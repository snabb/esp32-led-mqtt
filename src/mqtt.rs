use core::fmt::Write;

use embassy_futures::select::{Either4, select4};
use embassy_net::{IpEndpoint, Stack, tcp::TcpSocket};
use embassy_time::{Duration, Ticker, Timer, with_timeout};
use esp32_led_mqtt::{EFFECT_DISABLED_NAME, EFFECTS, EffectId};
use log::{info, warn};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Connection, Publication, Session, TopicFilter, Will,
};
use serde::Deserialize;
use smart_leds::RGB8;

use crate::{
    identity::{DEVICE_MODEL, DEVICE_NAME, DeviceIdentity},
    light_state::{self, LIGHT_STATE_CHANGED, MAX_EFFECT_SPEED},
};

const TEMPERATURE_INTERVAL: Duration = Duration::from_secs(60);

#[embassy_executor::task]
pub async fn task(
    stack: Stack<'static>,
    identity: &'static DeviceIdentity,
    settings: &'static esp32_led_mqtt::device_config::DeviceConfig,
    tls_context: crate::tls::Context,
    chip_temp: &'static crate::tsens::ChipTemp,
) {
    loop {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            info!("Network ready with IP {}", config.address);
        }

        let result = run_session(stack, identity, settings, tls_context, chip_temp).await;
        crate::ota::MQTT_READY.store(false, core::sync::atomic::Ordering::Release);
        match result {
            Ok(()) => warn!("MQTT session ended"),
            Err(err) => warn!("MQTT session failed: {:?}", err),
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

async fn run_session(
    stack: Stack<'static>,
    identity: &'static DeviceIdentity,
    settings: &'static esp32_led_mqtt::device_config::DeviceConfig,
    tls_context: crate::tls::Context,
    chip_temp: &crate::tsens::ChipTemp,
) -> Result<(), MqttRunError> {
    let tls = if settings.mqtt_tls {
        Some(tls_context.map_err(MqttRunError::Tls)?)
    } else {
        None
    };
    let address = crate::resolve_ipv4(stack, &settings.mqtt_broker)
        .await
        .map_err(MqttRunError::Resolve)?;
    let mut tcp_rx = [0_u8; 2048];
    let mut tcp_tx = [0_u8; 2048];
    let mut socket = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
    with_timeout(
        Duration::from_secs(15),
        socket.connect(IpEndpoint::new(address, settings.mqtt_port)),
    )
    .await
    .map_err(|_| MqttRunError::ConnectTimeout)?
    .map_err(MqttRunError::TcpConnect)?;
    if let Some(context) = tls {
        let mut tls =
            crate::tls::connect(context, crate::tls::Socket(socket), &settings.mqtt_broker)
                .await
                .map_err(MqttRunError::Tls)?;
        return bridge::run(&mut tls, identity, settings, chip_temp).await;
    }
    run_connected(socket, identity, settings, chip_temp).await
}

async fn run_connected<IO: minimq::Io>(
    socket: IO,
    identity: &DeviceIdentity,
    settings: &esp32_led_mqtt::device_config::DeviceConfig,
    chip_temp: &crate::tsens::ChipTemp,
) -> Result<(), MqttRunError> {
    let mut mqtt_rx = [0_u8; 1024];
    let mut mqtt_tx = [0_u8; 4096];
    let will = Will::new(identity.availability_topic.as_str(), b"offline", &[])
        .map_err(MqttRunError::Config)?
        .retained();
    let mut builder = ConfigBuilder::new(Buffers::new(&mut mqtt_rx, &mut mqtt_tx))
        .client_id(identity.slug.as_str())
        .map_err(MqttRunError::Config)?
        .keepalive_interval(30)
        .will(will)
        .map_err(MqttRunError::Config)?;
    if !settings.mqtt_username.as_str().is_empty() || !settings.mqtt_password.as_str().is_empty() {
        builder = builder
            .auth(
                settings.mqtt_username.as_str(),
                settings.mqtt_password.as_str().as_bytes(),
            )
            .map_err(MqttRunError::Config)?;
    }

    let mut mqtt_session = Session::new(builder);
    let mut session = mqtt_session.connect(socket).await.map_err(mqtt_error)?;
    let event = session.connect_event();
    info!("MQTT connected: {:?}", event);
    crate::memory::report("mqtt_connected");

    publish(&mut session, &identity.availability_topic, "online").await?;
    publish_light_discovery(&mut session, identity).await?;
    publish_speed_discovery(&mut session, identity).await?;
    publish_light_state(&mut session, identity).await?;
    publish_speed_state(&mut session, identity).await?;
    publish_temperature_discovery(&mut session, identity).await?;
    publish_firmware_version(&mut session, identity).await?;
    publish_temperature(&mut session, identity, chip_temp).await?;

    if matches!(event, ConnectEvent::Connected) {
        session
            .subscribe(
                &[
                    TopicFilter::new(identity.command_topic.as_str()),
                    TopicFilter::new(identity.speed_command_topic.as_str()),
                    TopicFilter::new(identity.ota_command_topic.as_str()),
                ],
                &[],
            )
            .await
            .map_err(mqtt_error)?;
    }

    crate::ota::MQTT_READY.store(true, core::sync::atomic::Ordering::Release);
    // The first tick fires one interval from now; the initial reading was published above.
    let mut temperature_ticker = Ticker::every(TEMPERATURE_INTERVAL);
    loop {
        match select4(
            session.recv(),
            LIGHT_STATE_CHANGED.wait(),
            crate::ota::STATUS.wait(),
            temperature_ticker.next(),
        )
        .await
        {
            Either4::First(Ok(message)) => {
                let topic = message.topic();
                if topic == identity.ota_command_topic.as_str() {
                    crate::ota::submit(message.payload(), message.retained());
                } else if topic == identity.command_topic.as_str() {
                    handle_command(message.payload());
                } else if topic == identity.speed_command_topic.as_str() {
                    handle_speed_command(message.payload());
                }
            }
            Either4::First(Err(err)) => {
                return Err(mqtt_error(err));
            }
            Either4::Second(()) => {
                publish_light_state(&mut session, identity).await?;
                publish_speed_state(&mut session, identity).await?;
            }
            Either4::Third(status) => {
                publish(&mut session, &identity.ota_status_topic, &status).await?
            }
            Either4::Fourth(()) => publish_temperature(&mut session, identity, chip_temp).await?,
        }
    }
}

async fn publish_light_state<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let light = light_state::get();
    let mut payload: heapless::String<192> = heapless::String::new();
    write!(
        payload,
        r#"{{"state":"{}","brightness":{},"color_mode":"rgb","color":{{"r":{},"g":{},"b":{}}},"effect":"{}"}}"#,
        if light.on { "ON" } else { "OFF" },
        light.brightness,
        light.color.r,
        light.color.g,
        light.color.b,
        light.effect.map_or(EFFECT_DISABLED_NAME, EffectId::name)
    )?;
    publish(session, &identity.state_topic, &payload).await
}

async fn publish_light_discovery<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<1400> = heapless::String::new();
    write!(
        payload,
        r#"{{"name":"LED Strip","unique_id":"{}","schema":"json","command_topic":"{}","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","brightness":true,"brightness_scale":255,"supported_color_modes":["rgb"],"effect":true,"effect_list":["{}""#,
        identity.slug,
        identity.command_topic,
        identity.state_topic,
        identity.availability_topic,
        EFFECT_DISABLED_NAME,
    )?;
    for (_, name) in EFFECTS {
        write!(payload, r#","{}""#, name)?;
    }
    write!(
        payload,
        r#"],"device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug, DEVICE_NAME, DEVICE_MODEL,
    )?;
    publish(session, &identity.discovery_topic, &payload).await
}

async fn publish_speed_discovery<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<700> = heapless::String::new();
    write!(
        payload,
        r#"{{"name":"Effect Speed","unique_id":"{}_effect_speed","command_topic":"{}","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","min":1,"max":{},"step":1,"mode":"slider","device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug,
        identity.speed_command_topic,
        identity.speed_state_topic,
        identity.availability_topic,
        MAX_EFFECT_SPEED,
        identity.slug,
        DEVICE_NAME,
        DEVICE_MODEL,
    )?;
    publish(session, &identity.speed_discovery_topic, &payload).await
}

async fn publish_speed_state<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<3> = heapless::String::new();
    write!(payload, "{}", light_state::get().speed)?;
    publish(session, &identity.speed_state_topic, &payload).await
}

async fn publish_temperature_discovery<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<700> = heapless::String::new();
    write!(
        payload,
        r#"{{"name":"Chip Temperature","unique_id":"{}_temperature","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","device_class":"temperature","unit_of_measurement":"°C","state_class":"measurement","entity_category":"diagnostic","suggested_display_precision":1,"device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug,
        identity.temperature_state_topic,
        identity.availability_topic,
        identity.slug,
        DEVICE_NAME,
        DEVICE_MODEL,
    )?;
    publish(session, &identity.temperature_discovery_topic, &payload).await
}

/// Discovery and state for the running build ID, the same string as in the image name.
async fn publish_firmware_version<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<700> = heapless::String::new();
    write!(
        payload,
        r#"{{"name":"Firmware Version","unique_id":"{}_firmware","state_topic":"{}","availability_topic":"{}","payload_available":"online","payload_not_available":"offline","entity_category":"diagnostic","device":{{"identifiers":["{}"],"name":"{}","manufacturer":"esp32-led-mqtt","model":"{}"}}}}"#,
        identity.slug,
        identity.firmware_state_topic,
        identity.availability_topic,
        identity.slug,
        DEVICE_NAME,
        DEVICE_MODEL,
    )?;
    publish(session, &identity.firmware_discovery_topic, &payload).await?;
    publish(
        session,
        &identity.firmware_state_topic,
        env!("FIRMWARE_BUILD_ID"),
    )
    .await
}

async fn publish_temperature<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    identity: &DeviceIdentity,
    chip_temp: &crate::tsens::ChipTemp,
) -> Result<(), MqttRunError> {
    let celsius = chip_temp.read().await;
    let mut payload: heapless::String<12> = heapless::String::new();
    write!(payload, "{:.1}", celsius)?;
    publish(session, &identity.temperature_state_topic, &payload).await
}

/// Every topic this device publishes is retained, so Home Assistant sees the
/// current state after a restart.
async fn publish<IO: minimq::Io>(
    session: &mut Connection<'_, '_, IO>,
    topic: &str,
    payload: &str,
) -> Result<(), MqttRunError> {
    session
        .publish(Publication::text(topic, payload).retain())
        .await
        .map_err(|err| match err {
            minimq::PubError::Session(err) => mqtt_error(err),
            minimq::PubError::Payload(_) => MqttRunError::PayloadTooLarge,
        })?;
    Ok(())
}

fn handle_command(payload: &[u8]) {
    let Ok((command, _)) = serde_json_core::from_slice::<LightCommand<'_>>(payload) else {
        warn!("Ignoring invalid light command JSON");
        return;
    };
    let on = match command.state {
        Some(state) if state.eq_ignore_ascii_case("ON") => Some(true),
        Some(state) if state.eq_ignore_ascii_case("OFF") => Some(false),
        _ => None,
    };
    let effect = command.effect.and_then(|name| {
        if name.eq_ignore_ascii_case(EFFECT_DISABLED_NAME) {
            Some(None)
        } else if let Some(id) = EffectId::from_name(name) {
            Some(Some(id))
        } else {
            warn!("Ignoring unknown light effect '{}'", name);
            None
        }
    });

    light_state::update(|state| {
        if let Some(on) = on {
            state.on = on;
        }
        if let Some(brightness) = command.brightness {
            state.brightness = brightness;
        }
        if let Some(LightColor { r, g, b }) = command.color {
            state.color = RGB8 { r, g, b };
        }
        if let Some(effect) = effect {
            state.effect = effect;
        }
    });
}

fn handle_speed_command(payload: &[u8]) {
    match parse_speed(payload) {
        Some(speed) => light_state::update(|state| state.speed = speed),
        None => warn!("Ignoring invalid effect speed command"),
    }
}

fn parse_speed(payload: &[u8]) -> Option<u8> {
    core::str::from_utf8(payload)
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
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

#[allow(dead_code, reason = "fields are only read by Debug when logging")]
#[derive(Debug)]
enum MqttRunError {
    Config(minimq::ConfigError),
    Transport,
    TlsIo(mbedtls_rs::SessionError),
    ConnectTimeout,
    Tls(&'static str),
    PayloadTooLarge,
    Resolve(&'static str),
    TcpConnect(embassy_net::tcp::ConnectError),
}

impl From<core::fmt::Error> for MqttRunError {
    fn from(_: core::fmt::Error) -> Self {
        Self::PayloadTooLarge
    }
}

mod bridge;

fn mqtt_error<E: core::fmt::Debug>(error: minimq::Error<E>) -> MqttRunError {
    warn!("MQTT protocol/transport error: {:?}", error);
    MqttRunError::Transport
}
