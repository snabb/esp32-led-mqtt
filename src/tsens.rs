//! On-chip temperature sensor (ESP32-C6 die temperature).

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Timer};
use esp_hal::{
    efuse,
    peripherals::{I2C_ANA_MST, MODEM_LPCON, TSENS},
    tsens::{Config, Temperature, TemperatureSensor},
};

/// Latest die temperature in °C; each control plane holds one receiver.
pub static TEMPERATURE: Watch<CriticalSectionRawMutex, f32, 2> = Watch::new();
const INTERVAL: Duration = Duration::from_secs(60);

#[embassy_executor::task]
pub async fn task(mut sensor: ChipTemp) {
    loop {
        TEMPERATURE.sender().send(sensor.read());
        Timer::after(INTERVAL).await;
    }
}

/// Stays powered for its lifetime.
pub struct ChipTemp(TemperatureSensor<'static>);

impl ChipTemp {
    pub fn new(tsens: TSENS<'static>) -> Self {
        // `ConfigError` has no variants yet.
        let sensor = TemperatureSensor::new(tsens, Config::default())
            .expect("temperature sensor configuration is infallible");
        Self(sensor)
    }

    /// Die temperature in °C.
    fn read(&mut self) -> f32 {
        // esp-hal applies no eFuse trim.
        self.read_in_current_range().to_celsius() - efuse_correction()
    }

    /// esp-hal converts with the range it selected in `new`, but the Wi-Fi PHY uses this
    /// sensor too and switches the range to suit the temperature (`tsens_dac_cal_new` in
    /// libphy). Convert with the range the reading was taken in instead.
    fn read_in_current_range(&mut self) -> Temperature {
        loop {
            // Keeps the PHY, which runs from interrupts, away until both range reads are done.
            let (before, reading, after) =
                critical_section::with(|_| (range_dac(), self.0.get_temperature(), range_dac()));
            if before == after {
                return match range_offset(before) {
                    Some(offset) => Temperature::new(reading.raw_value, offset),
                    None => reading,
                };
            }
            // The output needs this long to settle after a range change (ESP-IDF's figure).
            esp_hal::rom::ets_delay_us(300);
        }
    }
}

/// The conversion offset for a range DAC setting: ESP32-C6 TRM v1.2, Table 39.3-1, with the
/// DAC values from ESP-IDF's `temperature_sensor_periph.c`.
fn range_offset(dac: u8) -> Option<i8> {
    match dac {
        5 => Some(-2), // 50 °C to 125 °C
        7 => Some(-1), // 20 °C to 100 °C
        15 => Some(0), // -10 °C to 80 °C
        11 => Some(1), // -30 °C to 50 °C
        10 => Some(2), // -40 °C to 20 °C
        _ => None,
    }
}

/// The sensor's range DAC, `I2C_SARADC_TSENS_DAC`: bits 3..0 of register 6 in analog I2C
/// block 0x69. esp-hal's regi2c access is crate-private; this is its read sequence for the
/// ESP32-C6. Call it inside a critical section: the PHY drives the same I2C master.
fn range_dac() -> u8 {
    const BLOCK: u8 = 0x69;
    const REG: u8 = 6;

    MODEM_LPCON::regs()
        .clk_conf()
        .modify(|_, w| w.clk_i2c_mst_en().set_bit());
    MODEM_LPCON::regs()
        .i2c_mst_clk_conf()
        .modify(|_, w| w.clk_i2c_mst_sel_160m().set_bit());
    let master = usize::from(
        !I2C_ANA_MST::regs()
            .ana_conf2()
            .read()
            .sar_i2c_mst_sel()
            .bit_is_set(),
    );
    I2C_ANA_MST::regs().ana_conf1().write(|w| unsafe {
        w.bits(0x00FF_FFFF);
        w.sar_i2c_rd().clear_bit()
    });

    let ctrl = I2C_ANA_MST::regs().i2c_ctrl(master);
    while ctrl.read().busy().bit() {}
    ctrl.write(|w| unsafe {
        w.slave_addr().bits(BLOCK);
        w.slave_reg_addr().bits(REG)
    });
    while ctrl.read().busy().bit() {}
    ctrl.read().data().bits() & 0x0F
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
