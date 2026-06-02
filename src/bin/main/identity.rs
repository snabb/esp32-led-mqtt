use core::fmt::{self, Write};

use esp_hal::efuse;

pub const DEVICE_NAME: &str = "ESP32 LED MQTT";
pub const DEVICE_MODEL: &str = "ESP32-C6";

pub struct DeviceIdentity {
    pub slug: heapless::String<32>,
    pub client_id: heapless::String<32>,
    pub discovery_topic: heapless::String<80>,
    pub speed_discovery_topic: heapless::String<80>,
    pub command_topic: heapless::String<64>,
    pub state_topic: heapless::String<64>,
    pub speed_command_topic: heapless::String<80>,
    pub speed_state_topic: heapless::String<80>,
    pub availability_topic: heapless::String<64>,
}

impl DeviceIdentity {
    pub fn from_base_mac(mac: efuse::MacAddress) -> Self {
        let mac = mac.as_bytes();
        let suffix =
            identity_string::<6>(format_args!("{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5]));
        let slug = identity_string::<32>(format_args!("esp32_led_mqtt_{}", suffix));
        let client_id = slug.clone();

        Self {
            discovery_topic: identity_string::<80>(format_args!(
                "homeassistant/light/{}/config",
                slug
            )),
            speed_discovery_topic: identity_string::<80>(format_args!(
                "homeassistant/number/{}_effect_speed/config",
                slug
            )),
            command_topic: identity_string::<64>(format_args!("{}/light/set", slug)),
            state_topic: identity_string::<64>(format_args!("{}/light/state", slug)),
            speed_command_topic: identity_string::<80>(format_args!("{}/effect_speed/set", slug)),
            speed_state_topic: identity_string::<80>(format_args!("{}/effect_speed/state", slug)),
            availability_topic: identity_string::<64>(format_args!("{}/status", slug)),
            client_id,
            slug,
        }
    }
}

fn identity_string<const N: usize>(args: fmt::Arguments<'_>) -> heapless::String<N> {
    let mut value = heapless::String::new();
    value
        .write_fmt(args)
        .expect("device identity string capacity too small");
    value
}
