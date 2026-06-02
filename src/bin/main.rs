#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::{
    cell::RefCell,
    fmt::{self, Write},
};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{
    IpAddress, IpEndpoint, Runner, Stack, StackResources,
    dns::{DnsQueryType, IpAddress as DnsIpAddress},
    tcp::TcpSocket,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
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
use esp32_led_mqtt::{
    DEFAULT_BRIGHTNESS, EFFECT_DEFINITIONS, EFFECT_DISABLED_NAME, EFFECT_MAX_CODE,
    EFFECT_NONE_CODE, EffectId, EffectParams, EffectRuntime, RgbFrame, effect_code_from_id,
    effect_id_from_code, random_color,
};
use heapless::Vec;
use log::{error, info, warn};
use minimq::{Buffers, ConfigBuilder, ConnectEvent, Publication, Session, TopicFilter, Will};
use serde::Deserialize;
use smart_leds::{RGB8, SmartLedsWrite};
use static_cell::StaticCell;

mod secrets {
    include!(concat!(env!("OUT_DIR"), "/secrets.rs"));
}

const DEVICE_NAME: &str = "ESP32 LED MQTT";
const DEVICE_MODEL: &str = "ESP32-C6";

static LIGHT_STATE: critical_section::Mutex<RefCell<LightState>> =
    critical_section::Mutex::new(RefCell::new(DEFAULT_LIGHT_STATE));
static LIGHT_STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
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
const BUTTON_DEBOUNCE_MS: u64 = 50;
const BUTTON_DOUBLE_CLICK_MS: u64 = 350;
const BUTTON_LONG_PRESS_MS: u64 = 700;
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
const MAX_EFFECT_SPEED: u8 = 128;
const DEFAULT_EFFECT_SPEED: u8 = 64;
const DEFAULT_LIGHT_STATE: LightState = LightState {
    on: true,
    brightness: DEFAULT_BRIGHTNESS,
    effect_code: EFFECT_DEFINITIONS[0].code,
    speed: DEFAULT_EFFECT_SPEED,
    color: RGB8 {
        r: 255,
        g: 96,
        b: 24,
    },
};

#[derive(Clone, Copy)]
struct LightState {
    on: bool,
    brightness: u8,
    effect_code: u8,
    speed: u8,
    color: RGB8,
}

struct DeviceIdentity {
    slug: heapless::String<32>,
    client_id: heapless::String<32>,
    discovery_topic: heapless::String<80>,
    speed_discovery_topic: heapless::String<80>,
    command_topic: heapless::String<64>,
    state_topic: heapless::String<64>,
    speed_command_topic: heapless::String<80>,
    speed_state_topic: heapless::String<80>,
    availability_topic: heapless::String<64>,
}

impl DeviceIdentity {
    fn from_base_mac(mac: efuse::MacAddress) -> Self {
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
        Output::new(
            pin,
            led_power_level(light_state().on),
            OutputConfig::default(),
        )
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
        spawner.spawn(button_task(button).unwrap());
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
        spawner.spawn(mqtt_task(stack, identity).unwrap());
    }
    run_led_loop(strip, led_power).await
}

async fn run_led_loop(mut strip: LedStrip<'static>, mut led_power: Option<Output<'static>>) -> ! {
    let mut effect_runtime =
        EffectRuntime::<LED_COUNT>::new(current_effect_params(EffectId::Rainbow));
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
                power.set_level(led_power_inactive_level());
            }
            led_power_on = false;
        }
    }
}

#[derive(Clone, Copy)]
enum ButtonAction {
    CycleEffect,
    RandomColor,
    TogglePower,
}

enum ButtonPress {
    Short,
    Long,
}

#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        wait_debounced_press(&mut button).await;
        match wait_release_or_long_press(&mut button).await {
            ButtonPress::Long => handle_button_action(ButtonAction::TogglePower, now_ms()),
            ButtonPress::Short => {
                let second_press = select(
                    Timer::after(Duration::from_millis(BUTTON_DOUBLE_CLICK_MS)),
                    async {
                        wait_debounced_press(&mut button).await;
                        wait_release_or_long_press(&mut button).await
                    },
                )
                .await;

                match second_press {
                    Either::First(()) => handle_button_action(ButtonAction::CycleEffect, now_ms()),
                    Either::Second(ButtonPress::Short) => {
                        handle_button_action(ButtonAction::RandomColor, now_ms());
                    }
                    Either::Second(ButtonPress::Long) => {
                        handle_button_action(ButtonAction::TogglePower, now_ms());
                    }
                }
            }
        }
    }
}

async fn wait_debounced_press(button: &mut Input<'_>) {
    loop {
        button.wait_for_falling_edge().await;
        Timer::after(Duration::from_millis(BUTTON_DEBOUNCE_MS)).await;
        if button.is_low() {
            return;
        }
    }
}

async fn wait_debounced_release(button: &mut Input<'_>) {
    loop {
        button.wait_for_rising_edge().await;
        Timer::after(Duration::from_millis(BUTTON_DEBOUNCE_MS)).await;
        if button.is_high() {
            return;
        }
    }
}

async fn wait_release_or_long_press(button: &mut Input<'_>) -> ButtonPress {
    match select(
        Timer::after(Duration::from_millis(BUTTON_LONG_PRESS_MS)),
        wait_debounced_release(button),
    )
    .await
    {
        Either::First(()) => {
            wait_debounced_release(button).await;
            ButtonPress::Long
        }
        Either::Second(()) => ButtonPress::Short,
    }
}

fn handle_button_action(action: ButtonAction, now_ms: u64) {
    match action {
        ButtonAction::CycleEffect => cycle_effect(),
        ButtonAction::RandomColor => set_random_color(now_ms),
        ButtonAction::TogglePower => {
            update_light_state(|state| state.on = !state.on);
            mark_light_state_dirty();
        }
    }
}

fn cycle_effect() {
    let current = light_state().effect_code;
    let next = if current >= EFFECT_MAX_CODE {
        EFFECT_NONE_CODE
    } else {
        current + 1
    };
    update_light_state(|state| state.effect_code = next);
    mark_light_state_dirty();
}

fn set_random_color(now_ms: u64) {
    let previous = light_state().color;
    let mut rng = (now_ms as u32)
        ^ (u32::from(previous.r) << 16)
        ^ (u32::from(previous.g) << 8)
        ^ u32::from(previous.b)
        ^ 0x9e37_79b9;
    let color = random_color::from_seed(previous, next_random_u32(&mut rng));

    update_light_state(|state| state.color = color);
    mark_light_state_dirty();
}

fn next_random_u32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn mark_light_state_dirty() {
    LIGHT_STATE_CHANGED.signal(());
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

fn led_power_level(light_on: bool) -> esp_hal::gpio::Level {
    if light_on {
        LED_POWER_ACTIVE_LEVEL
    } else {
        led_power_inactive_level()
    }
}

fn led_power_inactive_level() -> esp_hal::gpio::Level {
    match LED_POWER_ACTIVE_LEVEL {
        esp_hal::gpio::Level::High => esp_hal::gpio::Level::Low,
        esp_hal::gpio::Level::Low => esp_hal::gpio::Level::High,
    }
}

fn light_state() -> LightState {
    critical_section::with(|cs| *LIGHT_STATE.borrow(cs).borrow())
}

fn update_light_state(update: impl FnOnce(&mut LightState)) {
    critical_section::with(|cs| update(&mut LIGHT_STATE.borrow(cs).borrow_mut()));
}

fn current_effect_params(id: EffectId) -> EffectParams {
    effect_params(id, light_state())
}

fn effect_params(id: EffectId, state: LightState) -> EffectParams {
    EffectParams {
        id,
        primary: state.color,
        speed: effect_speed_value(state.speed),
        intensity: 128,
    }
}

fn effect_speed_value(slider: u8) -> u8 {
    let slider = slider.clamp(1, MAX_EFFECT_SPEED);
    let offset = u64::from(slider.saturating_sub(1));
    let max_offset = u64::from(MAX_EFFECT_SPEED - 1);
    1 + ((offset * offset * offset * offset * offset)
        / (max_offset * max_offset * max_offset * max_offset)) as u8
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

#[embassy_executor::task]
async fn mqtt_task(stack: Stack<'static>, identity: &'static DeviceIdentity) {
    loop {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            info!("Network ready with IP {}", config.address);
        }

        match run_mqtt_session(stack, identity).await {
            Ok(()) => warn!("MQTT session ended"),
            Err(err) => warn!("MQTT session failed: {:?}", err),
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

async fn run_mqtt_session(
    stack: Stack<'static>,
    identity: &'static DeviceIdentity,
) -> Result<(), MqttRunError> {
    let endpoint = mqtt_endpoint(stack).await?;
    let mut tcp_rx = [0_u8; 2048];
    let mut tcp_tx = [0_u8; 2048];
    let mut socket = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
    socket
        .connect(endpoint)
        .await
        .map_err(MqttRunError::TcpConnect)?;

    let mut mqtt_rx = [0_u8; 1024];
    let mut mqtt_tx = [0_u8; 4096];
    let will = Will::new(identity.availability_topic.as_str(), b"offline", &[])
        .map_err(MqttRunError::Config)?
        .retained();
    let mut builder = ConfigBuilder::new(Buffers::new(&mut mqtt_rx, &mut mqtt_tx))
        .client_id(identity.client_id.as_str())
        .map_err(MqttRunError::Config)?
        .keepalive_interval(30)
        .will(will)
        .map_err(MqttRunError::Config)?;
    if !secrets::MQTT_USERNAME.is_empty() || !secrets::MQTT_PASSWORD.is_empty() {
        builder = builder
            .auth(secrets::MQTT_USERNAME, secrets::MQTT_PASSWORD.as_bytes())
            .map_err(MqttRunError::Config)?;
    }

    let mut session = Session::new(builder);
    let event = session.connect(socket).await.map_err(MqttRunError::Mqtt)?;
    info!("MQTT connected: {:?}", event);

    publish_text(
        &mut session,
        identity.availability_topic.as_str(),
        "online",
        true,
    )
    .await?;
    publish_light_discovery(&mut session, identity).await?;
    publish_speed_discovery(&mut session, identity).await?;
    publish_light_state(&mut session, identity).await?;
    publish_speed_state(&mut session, identity).await?;

    if matches!(event, ConnectEvent::Connected) {
        session
            .subscribe(
                &[
                    TopicFilter::new(identity.command_topic.as_str()),
                    TopicFilter::new(identity.speed_command_topic.as_str()),
                ],
                &[],
            )
            .await
            .map_err(MqttRunError::Mqtt)?;
    }

    loop {
        match select(session.recv(), LIGHT_STATE_CHANGED.wait()).await {
            Either::First(Ok(message)) => {
                if message.topic() == identity.command_topic.as_str()
                    && handle_command(message.payload())
                {
                    publish_light_state(&mut session, identity).await?;
                } else if message.topic() == identity.speed_command_topic.as_str()
                    && handle_speed_command(message.payload())
                {
                    publish_speed_state(&mut session, identity).await?;
                }
            }
            Either::First(Err(err)) => return Err(MqttRunError::Mqtt(err)),
            Either::Second(()) => publish_light_state(&mut session, identity).await?,
        }
    }
}

async fn mqtt_endpoint(stack: Stack<'static>) -> Result<IpEndpoint, MqttRunError> {
    if let Some(address) = parse_ipv4(secrets::MQTT_BROKER) {
        return Ok(IpEndpoint::new(
            IpAddress::Ipv4(address),
            secrets::MQTT_PORT,
        ));
    }

    info!("Resolving MQTT broker hostname");
    let addrs: Vec<DnsIpAddress, 1> = stack
        .dns_query(secrets::MQTT_BROKER, DnsQueryType::A)
        .await
        .map_err(MqttRunError::Dns)?;
    let Some(DnsIpAddress::Ipv4(address)) = addrs.first().copied() else {
        return Err(MqttRunError::NoBrokerAddress);
    };

    Ok(IpEndpoint::new(
        IpAddress::Ipv4(address),
        secrets::MQTT_PORT,
    ))
}

fn parse_ipv4(value: &str) -> Option<embassy_net::Ipv4Address> {
    let mut parts = value.split('.');
    let a = parse_octet(parts.next()?)?;
    let b = parse_octet(parts.next()?)?;
    let c = parse_octet(parts.next()?)?;
    let d = parse_octet(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }

    Some(embassy_net::Ipv4Address::new(a, b, c, d))
}

fn parse_octet(value: &str) -> Option<u8> {
    if value.is_empty() || value.len() > 3 {
        return None;
    }

    let mut parsed: u16 = 0;
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        parsed = parsed * 10 + u16::from(byte - b'0');
    }

    u8::try_from(parsed).ok()
}

async fn publish_light_state(
    session: &mut Session<'_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let light = light_state();
    let state = if light.on { "ON" } else { "OFF" };
    let mut payload: heapless::String<192> = heapless::String::new();
    write!(
        payload,
        r#"{{"state":"{}","brightness":{},"color_mode":"rgb","color":{{"r":{},"g":{},"b":{}}},"effect":"{}"}}"#,
        state,
        light.brightness,
        light.color.r,
        light.color.g,
        light.color.b,
        effect_id_from_code(light.effect_code).map_or(EFFECT_DISABLED_NAME, EffectId::name)
    )
    .map_err(|_| MqttRunError::StatePayloadTooLarge)?;
    publish_text(
        session,
        identity.state_topic.as_str(),
        payload.as_str(),
        true,
    )
    .await
}

async fn publish_light_discovery(
    session: &mut Session<'_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<1400> = heapless::String::new();
    write_light_discovery_payload(&mut payload, identity)?;
    publish_text(
        session,
        identity.discovery_topic.as_str(),
        payload.as_str(),
        true,
    )
    .await
}

fn write_light_discovery_payload(
    payload: &mut heapless::String<1400>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    write!(
        payload,
        r#"{{"name":"LED Strip","unique_id":"{}","schema":"json","command_topic":"{}","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","brightness":true,"brightness_scale":255,"supported_color_modes":["rgb"],"effect":true,"effect_list":["{}""#,
        identity.slug.as_str(),
        identity.command_topic.as_str(),
        identity.state_topic.as_str(),
        identity.availability_topic.as_str(),
        EFFECT_DISABLED_NAME,
    )
    .map_err(|_| MqttRunError::StatePayloadTooLarge)?;

    for definition in EFFECT_DEFINITIONS {
        write!(payload, r#","{}""#, definition.name)
            .map_err(|_| MqttRunError::StatePayloadTooLarge)?;
    }

    write!(
        payload,
        r#"],"device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug.as_str(),
        DEVICE_NAME,
        DEVICE_MODEL,
    )
    .map_err(|_| MqttRunError::StatePayloadTooLarge)
}

async fn publish_speed_discovery(
    session: &mut Session<'_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<700> = heapless::String::new();
    write_speed_discovery_payload(&mut payload, identity)?;
    publish_text(
        session,
        identity.speed_discovery_topic.as_str(),
        payload.as_str(),
        true,
    )
    .await
}

fn write_speed_discovery_payload(
    payload: &mut heapless::String<700>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    write!(
        payload,
        r#"{{"name":"Effect Speed","unique_id":"{}_effect_speed","command_topic":"{}","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","min":1,"max":128,"step":1,"mode":"slider","device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug.as_str(),
        identity.speed_command_topic.as_str(),
        identity.speed_state_topic.as_str(),
        identity.availability_topic.as_str(),
        identity.slug.as_str(),
        DEVICE_NAME,
        DEVICE_MODEL,
    )
    .map_err(|_| MqttRunError::StatePayloadTooLarge)
}

async fn publish_speed_state(
    session: &mut Session<'_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<3> = heapless::String::new();
    write!(payload, "{}", light_state().speed).map_err(|_| MqttRunError::StatePayloadTooLarge)?;
    publish_text(
        session,
        identity.speed_state_topic.as_str(),
        payload.as_str(),
        true,
    )
    .await
}

async fn publish_text(
    session: &mut Session<'_, TcpSocket<'_>>,
    topic: &str,
    payload: &str,
    retain: bool,
) -> Result<(), MqttRunError> {
    let mut publication = Publication::text(topic, payload);
    if retain {
        publication = publication.retain();
    }
    session
        .publish(publication)
        .await
        .map_err(|err| MqttRunError::Publish(format_pub_error(err)))?;
    Ok(())
}

fn format_pub_error<E, T>(err: minimq::PubError<E, T>) -> PublishError<T> {
    match err {
        minimq::PubError::Session(err) => PublishError::Session(err),
        minimq::PubError::Payload(_) => PublishError::Payload,
    }
}

fn handle_command(payload: &[u8]) -> bool {
    let Ok((command, _)) = serde_json_core::from_slice::<LightCommand<'_>>(payload) else {
        warn!("Ignoring invalid light command JSON");
        return false;
    };

    let mut on_update = None;
    let mut effect_update = None;

    if let Some(state) = command.state {
        if state.eq_ignore_ascii_case("ON") {
            on_update = Some(true);
        } else if state.eq_ignore_ascii_case("OFF") {
            on_update = Some(false);
        }
    }
    if let Some(effect) = command.effect {
        if effect.eq_ignore_ascii_case(EFFECT_DISABLED_NAME) {
            effect_update = Some(EFFECT_NONE_CODE);
        } else if let Some(effect_id) = EffectId::from_name(effect) {
            effect_update = Some(effect_code_from_id(effect_id));
        } else {
            warn!("Ignoring unknown light effect '{}'", effect);
        }
    }

    let changed = on_update.is_some()
        || command.brightness.is_some()
        || command.color.is_some()
        || effect_update.is_some();
    if changed {
        update_light_state(|state| {
            if let Some(on) = on_update {
                state.on = on;
            }
            if let Some(brightness) = command.brightness {
                state.brightness = brightness;
            }
            if let Some(color) = command.color {
                state.color = RGB8 {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                };
            }
            if let Some(effect_code) = effect_update {
                state.effect_code = effect_code;
            }
        });
    }

    changed
}

fn handle_speed_command(payload: &[u8]) -> bool {
    let Some(speed) = parse_speed(payload) else {
        warn!("Ignoring invalid effect speed command");
        return false;
    };

    update_light_state(|state| state.speed = speed);
    true
}

fn parse_speed(payload: &[u8]) -> Option<u8> {
    let text = core::str::from_utf8(payload).ok()?.trim();
    if text.is_empty() {
        return None;
    }

    let mut parsed: u16 = 0;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        parsed = parsed.saturating_mul(10) + u16::from(byte - b'0');
        if parsed > u16::from(u8::MAX) {
            return None;
        }
    }

    u8::try_from(parsed)
        .ok()
        .filter(|speed| (1..=MAX_EFFECT_SPEED).contains(speed))
}

#[derive(Deserialize)]
struct LightCommand<'a> {
    #[serde(default)]
    state: Option<&'a str>,
    #[serde(default)]
    brightness: Option<u8>,
    #[serde(default)]
    color: Option<LightColor>,
    #[serde(default)]
    effect: Option<&'a str>,
}

#[derive(Clone, Copy, Deserialize)]
struct LightColor {
    r: u8,
    g: u8,
    b: u8,
}

#[allow(dead_code)]
#[derive(Debug)]
enum PublishError<T> {
    Session(minimq::Error<T>),
    Payload,
}

#[allow(dead_code)]
#[derive(Debug)]
enum MqttRunError {
    Config(minimq::ConfigError),
    Dns(embassy_net::dns::Error),
    Mqtt(minimq::Error<embassy_net::tcp::Error>),
    NoBrokerAddress,
    Publish(PublishError<embassy_net::tcp::Error>),
    StatePayloadTooLarge,
    TcpConnect(embassy_net::tcp::ConnectError),
}
