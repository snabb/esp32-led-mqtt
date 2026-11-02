//! Portable, bounded device settings. Flash storage and USB framing use this same schema.

use heapless::String;
use serde::{Deserialize, Serialize};

pub const SCHEMA: u16 = 1;
pub const PARTITION_OFFSET: u32 = 0x3f0000;
pub const PARTITION_SIZE: u32 = 0x10000;
pub const MAX_BYTES: usize = 4096;

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
    pub mqtt_broker: String<128>,
    #[serde(default = "mqtt_port")]
    pub mqtt_port: u16,
    #[serde(default)]
    pub mqtt_username: String<128>,
    #[serde(default)]
    pub mqtt_password: String<128>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WifiAuth {
    Open,
    #[default]
    Wpa2Personal,
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
            || self.mqtt_broker.is_empty()
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
        Ok(())
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
            valid.replace("{", "{\"schema\":2,"),
            valid.replace("private\"", &format!("{}\"", "x".repeat(33))),
            valid.replace("{", "{\"mqtt_port\":0,"),
        ] {
            assert!(DeviceConfig::decode(bad.as_bytes()).is_err());
        }
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
