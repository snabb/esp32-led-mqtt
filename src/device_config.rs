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
    #[serde(default)]
    pub wifi_ssid: String<32>,
    #[serde(default)]
    pub wifi_password: String<64>,
    #[serde(default)]
    pub wifi_auth: WifiAuth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadProfile>,
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

/// Persistent credentials for joining an existing Thread network.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadProfile {
    pub dataset: String<508>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<core::net::Ipv6Addr>,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub seed: String<64>,
}

impl ThreadProfile {
    /// Decode and structurally validate an Active Operational Dataset.
    pub fn dataset_bytes(&self) -> Result<Vec<u8, 254>, Error> {
        let bytes = decode_hex(&self.dataset).ok_or(Error::Thread)?;
        let mut tlvs = bytes.as_slice();
        let mut seen = [false; 256];
        while !tlvs.is_empty() {
            let [kind, len, rest @ ..] = tlvs else {
                return Err(Error::Thread);
            };
            let len = usize::from(*len);
            let Some((value, remaining)) = rest.split_at_checked(len) else {
                return Err(Error::Thread);
            };
            if core::mem::replace(&mut seen[usize::from(*kind)], true) {
                return Err(Error::Thread);
            }
            // MeshCoP TLV types from OpenThread's meshcop_tlvs.hpp.
            let valid = match *kind {
                0 => matches!(value, [0, 0, 11..=26]),
                1 => len == 2,
                2 | 7 | 14 => len == 8,
                3 => {
                    (1..=16).contains(&len)
                        && core::str::from_utf8(value).is_ok()
                        && !value.contains(&0)
                }
                4 | 5 => len == 16,
                12 => matches!(len, 3 | 4),
                51 | 52 => false, // Pending Timestamp and Delay Timer.
                53 => len >= 3,
                _ => true,
            };
            if !valid {
                return Err(Error::Thread);
            }
            tlvs = remaining;
        }
        if ![0, 1, 2, 3, 4, 5, 7, 12, 14, 53]
            .into_iter()
            .all(|kind| seen[kind])
        {
            return Err(Error::Thread);
        }
        Ok(bytes)
    }

    /// The device-generated seed, if it has been committed to flash.
    pub fn seed_bytes(&self) -> Option<[u8; 32]> {
        decode_hex::<32>(&self.seed)?.as_slice().try_into().ok()
    }

    fn validate(&self) -> Result<(), Error> {
        self.dataset_bytes()?;
        if self.dns.is_some_and(|addr| {
            addr.is_unspecified()
                || addr.is_loopback()
                || addr.is_multicast()
                || addr.is_unicast_link_local()
                || addr.to_ipv4_mapped().is_some()
        }) || (!self.seed.is_empty() && self.seed_bytes().is_none())
        {
            return Err(Error::Thread);
        }
        Ok(())
    }

    /// Decode a remote profile request. Only the device can supply the seed.
    pub fn decode_request(bytes: &[u8]) -> Result<Self, Error> {
        let (profile, end): (Self, _) =
            serde_json_core::from_slice(bytes).map_err(|_| Error::Encoding)?;
        if !bytes[end..].iter().all(u8::is_ascii_whitespace) || !profile.seed.is_empty() {
            return Err(Error::Thread);
        }
        profile.validate()?;
        Ok(profile)
    }
}

fn decode_hex<const N: usize>(value: &str) -> Option<Vec<u8, N>> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    for &[high, low] in pairs {
        let high = char::from(high).to_digit(16)? as u8;
        let low = char::from(low).to_digit(16)? as u8;
        bytes.push((high << 4) | low).ok()?;
    }
    Some(bytes)
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
    Thread,
    Broker,
    Credentials,
    Coap,
}

impl DeviceConfig {
    /// Decode a complete JSON snapshot and validate its schema and field bounds.
    /// Returns an error category only, never a value copied from secret input.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let config = Self::parse(bytes)?;
        config.validate()?;
        Ok(config)
    }

    fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let (config, end): (Self, _) =
            serde_json_core::from_slice(bytes).map_err(|_| Error::Encoding)?;
        if !bytes[end..].iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Encoding);
        }
        Ok(config)
    }

    /// CoAP settings writes can omit the radio profile used by a Thread image.
    #[cfg(feature = "coap")]
    pub(crate) fn decode_coap_update(
        bytes: &[u8],
        current: &Self,
        uses_thread: bool,
    ) -> Result<Self, Error> {
        let mut config = Self::parse(bytes)?;
        if config.thread.is_some() {
            return Err(Error::Thread);
        }
        if uses_thread {
            config.copy_wifi(current);
        } else {
            config.validate_wifi()?;
        }
        config.keep_radio_profiles(Some(current));
        config.validate()?;
        // The flash owner preserves the committed profile, which may be newer.
        config.thread = None;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.schema != SCHEMA {
            return Err(Error::Schema);
        }
        if let Some(profile) = &self.thread {
            profile.validate()?;
        }
        if self.wifi_ssid.is_empty() {
            if self.thread.is_none()
                || !self.wifi_password.is_empty()
                || self.wifi_auth != WifiAuth::default()
            {
                return Err(Error::Wifi);
            }
        } else {
            self.validate_wifi()?;
        }
        if self.mqtt_port == 0
            || !(self
                .mqtt_broker
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
                || self.mqtt_broker.parse::<core::net::Ipv6Addr>().is_ok())
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

    fn validate_wifi(&self) -> Result<(), Error> {
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
        Ok(())
    }

    /// Whether the CoAP server has what it needs to start.
    pub fn has_coap_keys(&self) -> bool {
        !self.coap_device_key.is_empty() && !self.coap_clients.is_empty()
    }

    /// Carry forward any radio profile omitted by the provisioning request.
    pub fn keep_radio_profiles(&mut self, stored: Option<&Self>) {
        let Some(stored) = stored else {
            return;
        };
        if self.wifi_ssid.is_empty()
            && self.wifi_password.is_empty()
            && self.wifi_auth == WifiAuth::default()
        {
            self.copy_wifi(stored);
        }
        if self.thread.is_none() {
            self.thread = stored.thread.clone();
        }
    }

    fn copy_wifi(&mut self, stored: &Self) {
        self.wifi_ssid = stored.wifi_ssid.clone();
        self.wifi_password = stored.wifi_password.clone();
        self.wifi_auth = stored.wifi_auth;
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
pub(crate) const TEST_DATASET: &str = concat!(
    "000300000f0102123402080001020304050607030474657374",
    "041000112233445566778899aabbccddeeff0510ffeeddccbbaa99887766554433221100",
    "0708fd001122334455660c0402a0f7f80e08000000000001000035060004001fffe0",
);

#[cfg(test)]
mod tests {
    use super::*;
    const VALID: &[u8] = br#"{"wifi_ssid":"private","wifi_password":"private-pass","mqtt_broker":"mqtt.example","mqtt_password":"credential"}"#;

    #[test]
    fn thread_requires_a_complete_dataset_and_ipv6_unicast_dns() {
        let profile =
            |dataset: &str, dns: &str| format!(r#"{{"dataset":"{dataset}","dns":"{dns}"}}"#);
        let valid = profile(TEST_DATASET, "fd12::53");
        let parsed = ThreadProfile::decode_request(valid.as_bytes()).unwrap();
        assert_eq!(parsed.dns, Some("fd12::53".parse().unwrap()));
        assert_eq!(
            parsed.dataset_bytes().unwrap().len(),
            TEST_DATASET.len() / 2
        );
        for dns in [
            "127.0.0.1",
            "::",
            "::1",
            "ff02::1",
            "fe80::53",
            "::ffff:192.0.2.53",
        ] {
            assert!(ThreadProfile::decode_request(profile(TEST_DATASET, dns).as_bytes()).is_err());
        }
        for dataset in [
            "a",
            "zz",
            &TEST_DATASET[..TEST_DATASET.len() - 2],
            &TEST_DATASET[10..],
        ] {
            assert_eq!(
                ThreadProfile::decode_request(profile(dataset, "fd12::53").as_bytes()).err(),
                Some(Error::Thread),
            );
        }
        let duplicate = format!("{TEST_DATASET}000300000f");
        assert!(ThreadProfile::decode_request(profile(&duplicate, "fd12::53").as_bytes()).is_err());
        let injected = format!(
            r#"{{"dataset":"{TEST_DATASET}","seed":"{}"}}"#,
            "ab".repeat(32)
        );
        assert!(ThreadProfile::decode_request(injected.as_bytes()).is_err());
    }

    #[cfg(feature = "coap")]
    #[test]
    fn ordinary_thread_coap_settings_keep_the_wifi_rollback_profile() {
        let mut current = DeviceConfig::decode(VALID).unwrap();
        current.thread = Some(ThreadProfile {
            dataset: TEST_DATASET.try_into().unwrap(),
            dns: None,
            seed: "ab".repeat(32).as_str().try_into().unwrap(),
        });
        let input = br#"{"wifi_ssid":"other","wifi_password":"password","mqtt_broker":"fd12::1"}"#;
        let mut update = DeviceConfig::decode_coap_update(input, &current, true).unwrap();
        update.keep_radio_profiles(Some(&current));
        assert_eq!(update.wifi_ssid, current.wifi_ssid);
        assert_eq!(update.wifi_password, current.wifi_password);
        assert_eq!(update.thread.unwrap().seed_bytes(), Some([0xab; 32]));
        assert_eq!(update.mqtt_broker, "fd12::1");
    }

    #[test]
    fn thread_only_settings_and_a_wifi_update_preserve_the_other_profile() {
        let json = format!(r#"{{"thread":{{"dataset":"{TEST_DATASET}"}}}}"#);
        let mut thread = DeviceConfig::decode(json.as_bytes()).unwrap();
        assert!(thread.wifi_ssid.is_empty());
        thread.thread.as_mut().unwrap().seed = "ab".repeat(32).as_str().try_into().unwrap();
        let mut wifi = DeviceConfig::decode(VALID).unwrap();
        wifi.keep_radio_profiles(Some(&thread));
        thread.keep_radio_profiles(Some(&wifi));
        assert_eq!(thread.wifi_ssid, "private");
        assert_eq!(wifi.thread.unwrap().seed_bytes(), Some([0xab; 32]));
    }

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
            valid.replace("mqtt.example", "[2001:db8::1]"),
            valid.replace("mqtt.example", "mqtt.example:1883"),
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

        let valid = core::str::from_utf8(VALID).unwrap();
        assert!(
            DeviceConfig::decode(valid.replace("mqtt.example", "2001:db8::1").as_bytes()).is_ok()
        );

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
