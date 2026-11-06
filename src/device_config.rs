//! Portable, bounded device settings. Flash storage and USB framing use this same schema.

use core::ops::Range;
use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

pub const SCHEMA: u16 = 2;
/// The `device_config` entry in `partitions.csv`.
pub const PARTITION: Range<u32> = 0x3f0000..0x400000;
pub const MAX_BYTES: usize = 4096;
pub const MAX_COAP_CLIENTS: usize = 4;
/// Hex of one CCS credential. A P-256 key with both coordinates and a short key ID takes
/// about 85 bytes.
pub type CoapCredential = String<{ 2 * MAX_COAP_CREDENTIAL_BYTES }>;
pub const MAX_COAP_CREDENTIAL_BYTES: usize = 128;

/// Deliberately has no Debug implementation: this value contains credentials.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    #[serde(default = "schema")]
    pub schema: u16,
    #[serde(default)]
    pub generation: u32,
    pub wifi_ssid: String<32>,
    pub wifi_password: String<64>,
    #[serde(default)]
    pub wifi_auth: WifiAuth,
    /// Empty turns MQTT off at runtime. Left out of the snapshot when empty, so every
    /// snapshot with a broker stays readable by images that require this field.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub mqtt_broker: String<128>,
    #[serde(default = "mqtt_port")]
    pub mqtt_port: u16,
    #[serde(default)]
    pub mqtt_tls: bool,
    #[serde(default)]
    pub mqtt_username: String<128>,
    #[serde(default)]
    pub mqtt_password: String<128>,
    /// P-256 private key in hex, generated on the device and never accepted from outside.
    /// The CoAP fields exist in every build, so images built without `coap` keep them.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub coap_device_key: String<64>,
    /// Client public keys allowed to connect over CoAP.
    #[serde(default, skip_serializing_if = "no_clients")]
    pub coap_clients: Vec<CoapCredential, MAX_COAP_CLIENTS>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WifiAuth {
    Open,
    #[default]
    Wpa2Personal,
}

fn is_empty<const N: usize>(value: &String<N>) -> bool {
    value.is_empty()
}
fn no_clients(clients: &Vec<CoapCredential, MAX_COAP_CLIENTS>) -> bool {
    clients.is_empty()
}
fn is_hex(value: &str) -> bool {
    value.len().is_multiple_of(2) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
const fn schema() -> u16 {
    SCHEMA
}
const fn mqtt_port() -> u16 {
    1883
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Encoding,
    Schema,
    Wifi,
    Broker,
    Credentials,
    Coap,
}

impl DeviceConfig {
    /// Decode a complete JSON snapshot and validate its schema and field bounds.
    /// Returns an error category only, never a value copied from secret input.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (config, end): (Self, _) =
            serde_json_core::from_slice(bytes).map_err(|_| Error::Encoding)?;
        if !bytes[end..].iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Encoding);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.schema != SCHEMA {
            return Err(Error::Schema);
        }
        if self.wifi_ssid.is_empty()
            || self.wifi_ssid.contains('\0')
            || self.wifi_password.contains('\0')
        {
            return Err(Error::Wifi);
        }
        match self.wifi_auth {
            WifiAuth::Open if !self.wifi_password.is_empty() => return Err(Error::Wifi),
            WifiAuth::Wpa2Personal => {
                let password = self.wifi_password.as_bytes();
                if !(8..=63).contains(&password.len())
                    && !(password.len() == 64 && password.iter().all(u8::is_ascii_hexdigit))
                {
                    return Err(Error::Wifi);
                }
            }
            WifiAuth::Open => {}
        }
        if self.mqtt_port == 0
            || !self
                .mqtt_broker
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err(Error::Broker);
        }
        if self.mqtt_username.contains('\0') || self.mqtt_password.contains('\0') {
            return Err(Error::Credentials);
        }
        if (!self.coap_device_key.is_empty() && self.coap_device_key.len() != 64)
            || !is_hex(&self.coap_device_key)
            || self
                .coap_clients
                .iter()
                .any(|client| client.is_empty() || !is_hex(client))
        {
            return Err(Error::Coap);
        }
        Ok(())
    }

    /// Whether the CoAP server has what it needs to start.
    pub fn has_coap_keys(&self) -> bool {
        !self.coap_device_key.is_empty() && !self.coap_clients.is_empty()
    }

    /// Carries the CoAP keys over from the stored snapshot, so that writing settings
    /// can't silently lock clients out: a new snapshot only replaces the client list by
    /// naming one, and never the device key.
    pub fn keep_coap_keys(&mut self, stored: Option<&Self>) {
        let Some(stored) = stored else {
            return;
        };
        self.coap_device_key = stored.coap_device_key.clone();
        if self.coap_clients.is_empty() {
            self.coap_clients = stored.coap_clients.clone();
        }
    }

    /// Encode a validated snapshot. The caller owns and must protect the destination buffer.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, Error> {
        self.validate()?;
        serde_json_core::to_slice(self, output).map_err(|_| Error::Encoding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const VALID: &[u8] = br#"{"wifi_ssid":"private","wifi_password":"private-pass","mqtt_broker":"mqtt.example","mqtt_password":"credential"}"#;

    #[test]
    fn snapshot_round_trip_and_defaults() {
        let config = DeviceConfig::decode(VALID).unwrap();
        assert_eq!(config.schema, SCHEMA);
        assert_eq!(config.mqtt_port, 1883);
        assert!(!config.mqtt_tls);
        let mut output = [0; MAX_BYTES];
        let len = config.encode(&mut output).unwrap();
        let decoded = DeviceConfig::decode(&output[..len]).unwrap();
        assert_eq!(decoded.wifi_ssid, config.wifi_ssid);
        assert_eq!(decoded.mqtt_password, config.mqtt_password);
    }

    #[test]
    fn invalid_input_is_not_accepted_as_empty_defaults() {
        let valid = core::str::from_utf8(VALID).unwrap();
        for bad in [
            format!("{valid}x"),
            valid.replace("private-pass", "short"),
            valid.replace("mqtt.example", "http://mqtt.example"),
            valid.replace("\"mqtt_broker\"", "\"unknown\""),
            valid.replace("{", "{\"schema\":3,"),
            valid.replace("private\"", &format!("{}\"", "x".repeat(33))),
            valid.replace("{", "{\"mqtt_port\":0,"),
        ] {
            assert!(DeviceConfig::decode(bad.as_bytes()).is_err());
        }
    }

    #[test]
    fn broker_is_optional_and_only_written_when_set() {
        let config =
            DeviceConfig::decode(br#"{"wifi_ssid":"private","wifi_password":"private-pass"}"#)
                .unwrap();
        assert!(config.mqtt_broker.is_empty());
        let mut output = [0; MAX_BYTES];
        let len = config.encode(&mut output).unwrap();
        assert!(!output[..len].windows(11).any(|w| w == b"mqtt_broker"));

        let with_broker = DeviceConfig::decode(VALID).unwrap();
        let len = with_broker.encode(&mut output).unwrap();
        assert!(output[..len].windows(11).any(|w| w == b"mqtt_broker"));
    }

    const KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

    #[test]
    fn coap_fields_round_trip_and_are_only_written_when_set() {
        let mut config = DeviceConfig::decode(VALID).unwrap();
        let mut output = [0; MAX_BYTES];
        let len = config.encode(&mut output).unwrap();
        assert!(!output[..len].windows(4).any(|w| w == b"coap"));

        config.coap_device_key = KEY.try_into().unwrap();
        config
            .coap_clients
            .push("a108".try_into().unwrap())
            .unwrap();
        let len = config.encode(&mut output).unwrap();
        let decoded = DeviceConfig::decode(&output[..len]).unwrap();
        assert_eq!(decoded.coap_device_key, KEY);
        assert_eq!(decoded.coap_clients[0], "a108");
    }

    #[test]
    fn coap_fields_are_validated() {
        let valid = core::str::from_utf8(VALID).unwrap();
        let with = |field: &str| valid.replacen('{', &format!("{{{field},"), 1);
        for bad in [
            with(r#""coap_device_key":"abcd""#),
            with(&format!(r#""coap_device_key":"{}""#, "z".repeat(64))),
            with(r#""coap_clients":[""]"#),
            with(r#""coap_clients":["abc"]"#),
            with(r#""coap_clients":["zz"]"#),
            with(r#""coap_clients":["aa","aa","aa","aa","aa"]"#),
        ] {
            assert!(DeviceConfig::decode(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(DeviceConfig::decode(with(r#""coap_clients":["a108"]"#).as_bytes()).is_ok());
    }

    #[test]
    fn writing_settings_keeps_the_coap_keys() {
        let mut stored = DeviceConfig::decode(VALID).unwrap();
        stored.coap_device_key = KEY.try_into().unwrap();
        stored
            .coap_clients
            .push("a108".try_into().unwrap())
            .unwrap();

        let mut update = DeviceConfig::decode(VALID).unwrap();
        update.keep_coap_keys(Some(&stored));
        assert_eq!(update.coap_device_key, KEY);
        assert_eq!(update.coap_clients[0], "a108");

        let mut replacing = DeviceConfig::decode(VALID).unwrap();
        replacing
            .coap_clients
            .push("b208".try_into().unwrap())
            .unwrap();
        replacing.keep_coap_keys(Some(&stored));
        assert_eq!(replacing.coap_device_key, KEY);
        assert_eq!(replacing.coap_clients.len(), 1);
        assert_eq!(replacing.coap_clients[0], "b208");

        let mut first = DeviceConfig::decode(VALID).unwrap();
        first.keep_coap_keys(None);
        assert!(first.coap_device_key.is_empty());
    }

    #[test]
    fn tls_settings_round_trip() {
        let mut config = DeviceConfig::decode(VALID).unwrap();
        config.mqtt_tls = true;
        config.mqtt_port = 8883;
        let mut bytes = [0; MAX_BYTES];
        let n = config.encode(&mut bytes).unwrap();
        let decoded = DeviceConfig::decode(&bytes[..n]).unwrap();
        assert!(decoded.mqtt_tls);
        assert_eq!(decoded.mqtt_port, 8883);
    }

    #[test]
    fn open_wifi_is_explicit_and_cannot_keep_a_password() {
        let mut config = DeviceConfig::decode(VALID).unwrap();
        config.wifi_auth = WifiAuth::Open;
        assert_eq!(config.validate(), Err(Error::Wifi));
        config.wifi_password.clear();
        assert_eq!(config.validate(), Ok(()));
    }
}
