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
    ram,
    rmt::Rmt,
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_radio::wifi::{
    self, Config, ControllerConfig, Interface, WifiController, sta::StationConfig,
};
use esp32_led_mqtt::{EffectId, EffectRuntime, corrected};
use log::{error, info, warn};
use smart_leds::{RGB8, SmartLedsWrite};
use static_cell::StaticCell;

mod button;
#[cfg(feature = "_control")]
mod c_shims;
#[cfg(feature = "coap")]
mod coap;
mod config;
mod flash;
mod identity;
mod light_state;
#[cfg(feature = "mdns")]
mod mdns;
mod memory;
#[cfg(feature = "mqtt")]
mod mqtt;
mod ota;
#[cfg(feature = "_control")]
mod tls;
#[cfg(feature = "_control")]
mod tsens;

use identity::DeviceIdentity;
use light_state::{effect_params, get as light_state};

// LED count, timing, color order and pins come from hardware.json via build.rs.
include!(concat!(env!("OUT_DIR"), "/hardware.rs"));

// Wi-Fi activity can delay RMT refills; extra RMT memory gives the LED transfer
// more slack while the radio stack is running.
const RMT_MEMORY_BLOCKS: u8 = 4;
// The OTA manifest must match the version embedded in the app descriptor.
esp_bootloader_esp_idf::esp_app_desc!(
    env!("FIRMWARE_BUILD_ID"),
    env!("FIRMWARE_APP_PROJECT"),
    env!("FIRMWARE_BUILD_TIME"),
    env!("FIRMWARE_BUILD_DATE"),
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);

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
    // MQTT remains connected while OTA negotiates a second TLS session.
    esp_alloc::heap_allocator!(size: 100 * 1024);

    let mut timg0 = TimerGroup::new(peripherals.TIMG0);
    // Outlasts the two-minute OTA trial window; flash::task disables it afterwards.
    flash::arm_watchdog(&mut timg0.wdt, 125);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!(
        "Starting LED firmware {} hardware {}",
        env!("FIRMWARE_BUILD_ID"),
        env!("FIRMWARE_HARDWARE_ID")
    );
    let (usb_rx, _usb_tx) =
        esp_hal::usb::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE)
            .into_async()
            .split();
    spawner.spawn(config::usb_task(usb_rx).unwrap());
    let mut flash = esp_storage::FlashStorage::new(peripherals.FLASH);
    let layout = flash::check_partition_table(&mut flash);
    let loaded = match layout {
        Ok(()) => config::load(&mut flash).await,
        Err(reason) => Err(reason),
    };
    let device_config = match loaded {
        Ok(Some(config)) => Some(&*mk_static!(
            esp32_led_mqtt::device_config::DeviceConfig,
            config
        )),
        Ok(None) => {
            warn!("Device configuration required; provision over USB");
            None
        }
        Err(reason) => {
            warn!("Device configuration unavailable: {}", reason);
            None
        }
    };
    // Wi-Fi is the entropy source once configured. Before that, first-time provisioning
    // may generate the device's key, so the ADC has to provide it.
    #[cfg(feature = "coap")]
    if device_config.is_none() {
        mk_static!(
            esp_hal::rng::TrngSource<'static>,
            esp_hal::rng::TrngSource::new(peripherals.RNG, peripherals.ADC1)
        );
    }
    let identity = mk_static!(
        DeviceIdentity,
        DeviceIdentity::from_base_mac(efuse::base_mac_address())
    );
    info!("Device identity {}", identity.slug.as_str());

    let rmt_frequency = Rate::from_mhz(80);
    let rmt = Rmt::new(peripherals.RMT, rmt_frequency).expect("failed to initialize RMT");

    let strip = LedStrip::new_with_memsize(
        LED_TIMING,
        rmt.channel0,
        led_data_pin!(peripherals),
        RMT_MEMORY_BLOCKS,
        rmt_frequency,
    )
    .expect("failed to configure RMT LED channel");

    let led_power_pin: Option<esp_hal::gpio::AnyPin<'static>> = led_power_pin!(peripherals);
    let led_power = led_power_pin.map(|pin| {
        let level = if light_state().on {
            LED_POWER_ACTIVE_LEVEL
        } else {
            !LED_POWER_ACTIVE_LEVEL
        };
        Output::new(pin, level, OutputConfig::default())
    });

    // Buttons are active-low.
    let button_pin: Option<esp_hal::gpio::AnyPin<'static>> = button_pin!(peripherals);
    if let Some(pin) = button_pin {
        let button = Input::new(
            pin,
            esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
        );
        spawner.spawn(button::task(button).unwrap());
    }

    #[cfg(feature = "_control")]
    spawner.spawn(tsens::task(tsens::ChipTemp::new(peripherals.TSENS)).unwrap());

    let mut network = None;
    #[cfg(feature = "_control")]
    let mut download_context: ota::DownloadContext = Err("tls_disabled");
    #[cfg(not(feature = "_control"))]
    let download_context: ota::DownloadContext = ();
    let control_required = device_config.is_some_and(|settings| {
        (cfg!(feature = "mqtt") && !settings.mqtt_broker.is_empty())
            || (cfg!(feature = "coap")
                && !settings.coap_device_key.is_empty()
                && !settings.coap_clients.is_empty())
    });
    if let Some(settings) = device_config {
        let authentication = match settings.wifi_auth {
            esp32_led_mqtt::device_config::WifiAuth::Open => wifi::AuthenticationMethodConfig::Open,
            esp32_led_mqtt::device_config::WifiAuth::Wpa2Personal => {
                wifi::AuthenticationMethodConfig::Wpa2Personal(
                    settings
                        .wifi_password
                        .as_str()
                        .try_into()
                        .expect("validated password"),
                )
            }
        };
        let station_config = Config::Station(
            StationConfig::default()
                .with_ssid(
                    settings
                        .wifi_ssid
                        .as_str()
                        .try_into()
                        .expect("validated SSID"),
                )
                .with_authentication(authentication),
        );
        let mut controller = WifiController::new(
            peripherals.WIFI,
            ControllerConfig::default().with_initial_config(station_config),
        )
        .expect("failed to initialize Wi-Fi controller");
        controller
            .set_max_tx_power(40)
            .expect("failed to limit Wi-Fi TX power");
        let wifi_interface = Interface::station();

        let net_config = embassy_net::Config::dhcpv4(Default::default());
        let rng = Rng::new();
        let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
        let (stack, runner) = embassy_net::new(
            wifi_interface,
            net_config,
            // DHCP, DNS, MQTT and OTA, with room for the UDP services.
            mk_static!(StackResources<8>, StackResources::<8>::new()),
            seed,
        );

        #[cfg(feature = "_control")]
        if settings.mqtt_tls || ota::pull_enabled() {
            download_context = tls::initialize(
                mbedtls_rs::sys::hook::backend::esp::EspAccel::new()
                    .with_sha(peripherals.SHA)
                    .with_rsa(peripherals.RSA)
                    .with_aes(peripherals.AES)
                    .with_ecc(peripherals.ECC),
            );
            if let Err(reason) = download_context {
                warn!("TLS initialization failed: {}", reason);
            }
        }
        spawner.spawn(connection_task(controller).unwrap());
        spawner.spawn(net_task(runner).unwrap());
        #[cfg(feature = "mqtt")]
        if !settings.mqtt_broker.is_empty() {
            spawner.spawn(mqtt::task(stack, identity, settings, download_context).unwrap());
        }
        #[cfg(feature = "coap")]
        if !settings.coap_device_key.is_empty() && !settings.coap_clients.is_empty() {
            spawner.spawn(coap::task(stack, identity, settings).unwrap());
        }
        #[cfg(feature = "mdns")]
        spawner.spawn(mdns::task(stack, identity).unwrap());
        network = Some(stack);
    }
    spawner.spawn(
        flash::task(
            network,
            flash,
            timg0.wdt,
            layout,
            download_context,
            control_required,
        )
        .unwrap(),
    );
    memory::report("boot_ready");
    run_led_loop(strip, led_power).await
}

async fn run_led_loop(mut strip: LedStrip<'static>, mut led_power: Option<Output<'static>>) -> ! {
    let mut effect_runtime =
        EffectRuntime::<LED_COUNT>::new(effect_params(EffectId::Rainbow, light_state()));
    let mut led_power_on = light_state().on;
    let mut ticker = Ticker::every(LED_FRAME_INTERVAL);

    loop {
        ticker.next().await;
        let now_us = Instant::now().as_micros() as u32;
        let state = light_state();
        let light_on = state.on;
        if light_on && !led_power_on {
            if let Some(power) = led_power.as_mut() {
                power.set_level(LED_POWER_ACTIVE_LEVEL);
            }
            led_power_on = true;
        }

        let mut output = match (light_on, state.effect) {
            (false, _) => [RGB8 { r: 0, g: 0, b: 0 }; LED_COUNT],
            (true, Some(effect_id)) => {
                effect_runtime.set_effect(effect_params(effect_id, state));
                corrected(effect_runtime.render(now_us), state.brightness)
            }
            (true, None) => corrected(&[state.color; LED_COUNT], state.brightness),
        };

        if LED_REVERSED {
            output.reverse();
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
    match critical_section::with(|_| strip.write(*output)) {
        Ok(()) => ota::LED_READY.store(true, core::sync::atomic::Ordering::Relaxed),
        Err(err) => error!("LED write failed: {:?}", err),
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
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await;
}

/// Resolves an IPv4 literal, or else a DNS A record.
#[cfg(feature = "_control")]
async fn resolve_ipv4(
    stack: embassy_net::Stack<'static>,
    host: &str,
) -> Result<embassy_net::IpAddress, &'static str> {
    use embassy_net::{IpAddress, dns::DnsQueryType};
    use embassy_time::with_timeout;

    if let Ok(address) = host.parse() {
        return Ok(IpAddress::Ipv4(address));
    }
    let addresses = with_timeout(
        Duration::from_secs(15),
        stack.dns_query(host, DnsQueryType::A),
    )
    .await
    .map_err(|_| "dns_timeout")?
    .map_err(|_| "dns")?;
    match addresses.first() {
        Some(IpAddress::Ipv4(address)) => Ok(IpAddress::Ipv4(*address)),
        _ => Err("dns"),
    }
}
