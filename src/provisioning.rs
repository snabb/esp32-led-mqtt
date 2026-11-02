//! USB configuration commands contain secrets; responses carry metadata only.

use crate::device_config::{DeviceConfig, Error};
use heapless::String;
use serde::Deserialize;

pub const PREFIX: &[u8] = b"@CONFIG ";
pub const MAX_FRAME: usize = crate::device_config::MAX_BYTES + 128;

/// No Debug implementation because Configure contains private credentials.
#[expect(
    clippy::large_enum_variant,
    reason = "one bounded in-flight command owns credentials without heap allocation"
)]
pub enum Command {
    Status {
        id: String<32>,
    },
    Configure {
        id: String<32>,
        config: DeviceConfig,
    },
    Erase {
        id: String<32>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    op: Operation,
    id: String<32>,
    config: Option<DeviceConfig>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Status,
    Configure,
    Erase,
}

impl Command {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let json = bytes.strip_prefix(PREFIX).ok_or(Error::Encoding)?;
        let (envelope, end): (Envelope, _) =
            serde_json_core::from_slice(json).map_err(|_| Error::Encoding)?;
        if !json[end..].iter().all(u8::is_ascii_whitespace) {
            return Err(Error::Encoding);
        }
        let command = match (envelope.op, envelope.config) {
            (Operation::Status, None) => Self::Status { id: envelope.id },
            (Operation::Erase, None) => Self::Erase { id: envelope.id },
            (Operation::Configure, Some(config)) => Self::Configure {
                id: envelope.id,
                config,
            },
            _ => return Err(Error::Encoding),
        };
        let id = command.id();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Encoding);
        }
        if let Self::Configure { config, .. } = &command {
            config.validate()?;
        }
        Ok(command)
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Status { id } | Self::Configure { id, .. } | Self::Erase { id } => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_has_bounded_correlation_and_no_implicit_erase() {
        assert!(matches!(
            Command::decode(b"@CONFIG {\"op\":\"status\",\"id\":\"abc123\"}"),
            Ok(Command::Status { .. })
        ));
        for bad in [
            b"@CONFIG {\"op\":\"erase\",\"id\":\"\"}".as_slice(),
            b"@CONFIG {\"op\":\"status\",\"id\":\"abc\",\"config\":{}}",
            b"@CONFIG {\"op\":\"erase\",\"id\":\"abc\"}trailing",
            b"{\"op\":\"erase\",\"id\":\"abc\"}",
        ] {
            assert!(Command::decode(bad).is_err());
        }
    }
}
