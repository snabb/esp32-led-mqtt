#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use embassy_executor::Spawner;
use embassy_net::{Runner, StackResources};
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse,
    gpio::{Input, Output, OutputConfig},
    interrupt::software::SoftwareInterruptControl,
    ram,
    rmt::Rmt,
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_radio::wifi::{
    self, Config, ControllerConfig, Interface, WifiController, sta::StationConfig,
};
use esp32_led_mqtt::{EffectId, EffectRuntime, RgbFrame, effect_id_from_code};
use log::{error, info, warn};
use smart_leds::{RGB8, SmartLedsWrite};
use static_cell::StaticCell;

mod secrets {
    include!(concat!(env!("OUT_DIR"), "/secrets.rs"));
}

mod button;
pub(crate) mod identity;
pub(crate) mod light_state;
mod mqtt;

use identity::DeviceIdentity;
use light_state::{effect_params, get as light_state};
const START_NETWORK: bool = true;

// LED strip hardware configuration.
//
// User configuration points:
// - LED_COUNT: set this to the number of addressable LEDs in your strip.
// - LedStrip: select the LED timing and RGB color order for your strip.
// - LED_POWER_ACTIVE_LEVEL: set to esp_hal::gpio::Level::Low if your
//   power-enable circuit is active-low.
// - In main, configure button_pin, led_power_pin, and the GPIO passed to
//   LedStrip::new_with_memsize.
//
// Other controller timings provided by esp-hal-smartled2 include
// Ws2812bTiming, Ws2812Timing, Ws2811Timing, and Ws2811LowSpeedTiming. Common
// RGB color orders are color_order::{Rgb, Rbg, Grb, Gbr, Brg, Bgr}.
//
// M5Stack NanoC6 internal LED preset:
// - Set LED_COUNT to 1.
// - In LedStrip, use esp_hal_smartled::Ws2812Timing.
// - In main, set led_power_pin to Some(peripherals.GPIO19.into()).
// - In main, pass peripherals.GPIO20 to LedStrip::new_with_memsize.
const LED_COUNT: usize = 60;
const LED_POWER_ACTIVE_LEVEL: esp_hal::gpio::Level = esp_hal::gpio::Level::High;
const LED_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
type LedStrip<'d> = esp_hal_smartled::RmtSmartLeds<
    'd,
    { esp_hal_smartled::buffer_size::<RGB8>(LED_COUNT) },
    esp_hal::Blocking,
    RGB8,
    esp_hal_smartled::color_order::Grb,
    esp_hal_smartled::Sk68xxTiming,
>;

// Wi-Fi activity can delay RMT refills; extra RMT memory gives the LED transfer
// more slack while the radio stack is running.
const RMT_MEMORY_BLOCKS: u8 = 4;
// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: StaticCell<$t> = StaticCell::new();
        STATIC_CELL.init($val)
    }};
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, software_interrupt.software_interrupt0);

    info!("Starting LED firmware");
    let identity = mk_static!(
        DeviceIdentity,
        DeviceIdentity::from_base_mac(efuse::base_mac_address())
    );
    info!("Device identity {}", identity.slug.as_str());

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("failed to initialize RMT");

    // LED data GPIO.
    //
    // Replace peripherals.GPIO2 with the GPIO connected to the DIN/data input
    // of your LED strip.
    let strip = LedStrip::new_with_memsize(rmt.channel0, peripherals.GPIO2, RMT_MEMORY_BLOCKS)
        .expect("failed to configure RMT LED channel");

    // Optional LED strip power control GPIO.
    //
    // Leave this as None if your strip is always powered, which is the default
    // M5Stack NanoC6 + RGB LED Strip setup. To control an active-high enable
    // pin, change this to Some(peripherals.GPIOx.into()) and replace GPIOx with
    // your board's power-enable pin.
    let led_power_pin: Option<esp_hal::gpio::AnyPin<'static>> = None;
    let led_power = led_power_pin.map(|pin| {
        let level = if light_state().on {
            LED_POWER_ACTIVE_LEVEL
        } else {
            !LED_POWER_ACTIVE_LEVEL
        };
        Output::new(pin, level, OutputConfig::default())
    });

    // Optional button input GPIO.
    //
    // The M5Stack NanoC6 button is on GPIO9 and pulls the pin low when pressed.
    // Leave this as None if your board has no button, or replace GPIO9 with
    // your board's active-low button pin.
    let button_pin: Option<esp_hal::gpio::AnyPin<'static>> = Some(peripherals.GPIO9.into());
    if let Some(pin) = button_pin {
        let button = Input::new(
            pin,
            esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
        );
        spawner.spawn(button::task(button).unwrap());
    }

    if START_NETWORK {
        let station_config = Config::Station(
            StationConfig::default()
                .with_ssid(secrets::WIFI_SSID)
                .with_password(secrets::WIFI_PASSWORD.into()),
        );
        let (mut controller, interfaces) = wifi::new(
            peripherals.WIFI,
            ControllerConfig::default().with_initial_config(station_config),
        )
        .expect("failed to initialize Wi-Fi controller");
        controller
            .set_max_tx_power(40)
            .expect("failed to limit Wi-Fi TX power");
        let wifi_interface = interfaces.station;

        let net_config = embassy_net::Config::dhcpv4(Default::default());
        let rng = Rng::new();
        let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            mk_static!(StackResources<4>, StackResources::<4>::new()),
            seed,
        );

        spawner.spawn(connection_task(controller).unwrap());
        spawner.spawn(net_task(runner).unwrap());
        spawner.spawn(mqtt::task(stack, identity).unwrap());
    }
    run_led_loop(strip, led_power).await
}

async fn run_led_loop(mut strip: LedStrip<'static>, mut led_power: Option<Output<'static>>) -> ! {
    let mut effect_runtime =
        EffectRuntime::<LED_COUNT>::new(effect_params(EffectId::Rainbow, light_state()));
    let mut solid_frame = RgbFrame::<LED_COUNT>::new();
    let mut output = [RGB8 { r: 0, g: 0, b: 0 }; LED_COUNT];
    let mut led_power_on = light_state().on;
    let mut ticker = Ticker::every(LED_FRAME_INTERVAL);

    loop {
        ticker.next().await;
        let now_ms = Instant::now().as_millis();
        let state = light_state();
        let light_on = state.on;
        if light_on && !led_power_on {
            if let Some(power) = led_power.as_mut() {
                power.set_level(LED_POWER_ACTIVE_LEVEL);
            }
            led_power_on = true;
        }

        if light_on {
            if let Some(effect_id) = effect_id_from_code(state.effect_code) {
                effect_runtime.set_effect(effect_params(effect_id, state));
                output = effect_runtime
                    .render(now_ms as u32)
                    .corrected(state.brightness);
            } else {
                solid_frame.set_all(state.color);
                output = solid_frame.corrected(state.brightness);
            }
        } else {
            output.fill(RGB8 { r: 0, g: 0, b: 0 });
        }

        write_frame(&mut strip, &output);
        if !light_on && led_power_on {
            if let Some(power) = led_power.as_mut() {
                power.set_level(!LED_POWER_ACTIVE_LEVEL);
            }
            led_power_on = false;
        }
    }
}

fn write_frame(strip: &mut LedStrip<'_>, output: &[RGB8; LED_COUNT]) {
    // Keep the short RMT refill window from being preempted by radio work.
    let result = critical_section::with(|_| strip.write(*output));
    if let Err(err) = result {
        error!("LED write failed: {:?}", err);
    }
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        info!("Connecting Wi-Fi");
        match controller.connect_async().await {
            Ok(info) => {
                info!("Wi-Fi connected: {:?}", info);
                let info = controller.wait_for_disconnect_async().await.ok();
                warn!("Wi-Fi disconnected: {:?}", info);
            }
            Err(err) => warn!("Wi-Fi connection failed: {:?}", err),
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await;
}
