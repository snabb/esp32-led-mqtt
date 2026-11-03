//! On-chip temperature sensor (ESP32-C6 die temperature).

use embassy_time::{Duration, Timer};
use esp_hal::{
    efuse,
    peripherals::TSENS,
    tsens::{Config, TemperatureSensor},
};

/// Time for the sensor to settle after power-up.
const SETTLE: Duration = Duration::from_millis(20);

pub struct ChipTemp(TemperatureSensor<'static>);

impl ChipTemp {
    pub fn new(tsens: TSENS<'static>) -> Self {
        // `ConfigError` has no variants yet.
        let sensor = TemperatureSensor::new(tsens, Config::default())
            .expect("temperature sensor configuration is infallible");
        // Only powered while measuring.
        sensor.power_down();
        Self(sensor)
    }

    /// Die temperature in °C.
    pub async fn read(&self) -> f32 {
        self.0.power_up();
        Timer::after(SETTLE).await;
        let raw = self.0.get_temperature().raw_value;
        self.0.power_down();

        // esp-hal's `to_celsius()` applies a fixed -1 range offset (+27.88 °C), but the
        // range DAC holds the -10..80 °C setting (offset 0) after reset. Writing the DAC
        // would need the regi2c bus shared with the Wi-Fi PHY, so rely on the reset value.
        0.4386 * f32::from(raw) - 20.52 - efuse_correction()
    }
}

/// Per-chip offset burned at final test: bit 8 is the sign, bits 7..0 the magnitude in
/// tenths of a degree. Subtracted from the reading, as ESP-IDF does. Unburned reads 0.
fn efuse_correction() -> f32 {
    let raw = efuse::read_field_le::<u16>(efuse::TEMP_CALIB);
    let magnitude = f32::from(raw & 0xFF) / 10.0;
    if raw & 0x100 != 0 {
        -magnitude
    } else {
        magnitude
    }
}
