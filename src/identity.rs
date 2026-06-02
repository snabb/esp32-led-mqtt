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
        let suffix: heapless::String<6> =
            heapless::format!("{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
                .expect("device identity suffix capacity too small");
        let slug: heapless::String<32> = heapless::format!("esp32_led_mqtt_{}", suffix)
            .expect("device identity slug capacity too small");
        let client_id = slug.clone();

        Self {
            discovery_topic: heapless::format!("homeassistant/light/{}/config", slug)
                .expect("device discovery topic capacity too small"),
            speed_discovery_topic: heapless::format!(
                "homeassistant/number/{}_effect_speed/config",
                slug
            )
            .expect("device speed discovery topic capacity too small"),
            command_topic: heapless::format!("{}/light/set", slug)
                .expect("device command topic capacity too small"),
            state_topic: heapless::format!("{}/light/state", slug)
                .expect("device state topic capacity too small"),
            speed_command_topic: heapless::format!("{}/effect_speed/set", slug)
                .expect("device speed command topic capacity too small"),
            speed_state_topic: heapless::format!("{}/effect_speed/state", slug)
                .expect("device speed state topic capacity too small"),
            availability_topic: heapless::format!("{}/status", slug)
                .expect("device availability topic capacity too small"),
            client_id,
            slug,
        }
    }
}
