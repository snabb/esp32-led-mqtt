use core::fmt::Write;
use core::str::FromStr;

use embassy_futures::select::{Either3, select3};
use embassy_net::{
    IpAddress, IpEndpoint, Stack,
    dns::{DnsQueryType, IpAddress as DnsIpAddress},
    tcp::TcpSocket,
};
use embassy_time::{Duration, Timer};
use esp32_led_mqtt::{EFFECT_DISABLED_NAME, EFFECTS, EffectId};
use heapless::Vec;
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

#[embassy_executor::task]
pub async fn task(
    stack: Stack<'static>,
    identity: &'static DeviceIdentity,
    settings: &'static esp32_led_mqtt::device_config::DeviceConfig,
) {
    loop {
        stack.wait_config_up().await;
        if let Some(config) = stack.config_v4() {
            info!("Network ready with IP {}", config.address);
        }

        let result = run_session(stack, identity, settings).await;
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
) -> Result<(), MqttRunError> {
    let endpoint = endpoint(stack, settings).await?;
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
    let mut session = mqtt_session
        .connect(socket)
        .await
        .map_err(MqttRunError::Mqtt)?;
    let event = session.connect_event();
    info!("MQTT connected: {:?}", event);

    publish(&mut session, &identity.availability_topic, "online").await?;
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
                    TopicFilter::new(identity.ota_command_topic.as_str()),
                ],
                &[],
            )
            .await
            .map_err(MqttRunError::Mqtt)?;
    }

    crate::ota::MQTT_READY.store(true, core::sync::atomic::Ordering::Release);
    loop {
        match select3(
            session.recv(),
            LIGHT_STATE_CHANGED.wait(),
            crate::ota::STATUS.wait(),
        )
        .await
        {
            Either3::First(Ok(message)) => {
                let topic = message.topic();
                if topic == identity.ota_command_topic.as_str() {
                    crate::ota::submit(message.payload(), message.retained());
                } else if topic == identity.command_topic.as_str() {
                    handle_command(message.payload());
                } else if topic == identity.speed_command_topic.as_str() {
                    handle_speed_command(message.payload());
                }
            }
            Either3::First(Err(err)) => return Err(MqttRunError::Mqtt(err)),
            Either3::Second(()) => {
                publish_light_state(&mut session, identity).await?;
                publish_speed_state(&mut session, identity).await?;
            }
            Either3::Third(status) => {
                publish(&mut session, &identity.ota_status_topic, &status).await?
            }
        }
    }
}

async fn endpoint(
    stack: Stack<'static>,
    settings: &esp32_led_mqtt::device_config::DeviceConfig,
) -> Result<IpEndpoint, MqttRunError> {
    if let Ok(IpAddress::Ipv4(address)) = IpAddress::from_str(settings.mqtt_broker.as_str()) {
        return Ok(IpEndpoint::new(
            IpAddress::Ipv4(address),
            settings.mqtt_port,
        ));
    }

    info!("Resolving MQTT broker hostname");
    let addrs: Vec<DnsIpAddress, 1> = stack
        .dns_query(settings.mqtt_broker.as_str(), DnsQueryType::A)
        .await
        .map_err(MqttRunError::Dns)?;
    let Some(DnsIpAddress::Ipv4(address)) = addrs.first().copied() else {
        return Err(MqttRunError::NoBrokerAddress);
    };

    Ok(IpEndpoint::new(
        IpAddress::Ipv4(address),
        settings.mqtt_port,
    ))
}

async fn publish_light_state(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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

async fn publish_light_discovery(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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

async fn publish_speed_discovery(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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

async fn publish_speed_state(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<3> = heapless::String::new();
    write!(payload, "{}", light_state::get().speed)?;
    publish(session, &identity.speed_state_topic, &payload).await
}

/// Every topic this device publishes is retained, so Home Assistant sees the
/// current state after a restart.
async fn publish(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
    topic: &str,
    payload: &str,
) -> Result<(), MqttRunError> {
    session
        .publish(Publication::text(topic, payload).retain())
        .await
        .map_err(|err| match err {
            minimq::PubError::Session(err) => MqttRunError::Mqtt(err),
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
    Dns(embassy_net::dns::Error),
    Mqtt(minimq::Error<embassy_net::tcp::Error>),
    NoBrokerAddress,
    PayloadTooLarge,
    TcpConnect(embassy_net::tcp::ConnectError),
}

impl From<core::fmt::Error> for MqttRunError {
    fn from(_: core::fmt::Error) -> Self {
        Self::PayloadTooLarge
    }
}
