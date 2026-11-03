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
        .client_id(identity.client_id.as_str())
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
                if message.topic() == identity.ota_command_topic.as_str() {
                    crate::ota::submit(message.payload(), message.retained());
                } else if message.topic() == identity.command_topic.as_str()
                    && handle_command(message.payload())
                {
                    publish_light_state(&mut session, identity).await?;
                } else if message.topic() == identity.speed_command_topic.as_str()
                    && handle_speed_command(message.payload())
                {
                    publish_speed_state(&mut session, identity).await?;
                }
            }
            Either3::First(Err(err)) => return Err(MqttRunError::Mqtt(err)),
            Either3::Second(()) => publish_light_state(&mut session, identity).await?,
            Either3::Third(status) => {
                publish_text(&mut session, &identity.ota_status_topic, &status, true).await?
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
        light.effect.map_or(EFFECT_DISABLED_NAME, EffectId::name)
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
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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

    for (_, name) in EFFECTS {
        write!(payload, r#","{}""#, name).map_err(|_| MqttRunError::StatePayloadTooLarge)?;
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
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
    identity: &DeviceIdentity,
) -> Result<(), MqttRunError> {
    let mut payload: heapless::String<3> = heapless::String::new();
    write!(payload, "{}", light_state::get().speed)
        .map_err(|_| MqttRunError::StatePayloadTooLarge)?;
    publish_text(
        session,
        identity.speed_state_topic.as_str(),
        payload.as_str(),
        true,
    )
    .await
}

async fn publish_text(
    session: &mut Connection<'_, '_, TcpSocket<'_>>,
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
            effect_update = Some(None);
        } else if let Some(effect_id) = EffectId::from_name(effect) {
            effect_update = Some(Some(effect_id));
        } else {
            warn!("Ignoring unknown light effect '{}'", effect);
        }
    }

    let changed = on_update.is_some()
        || command.brightness.is_some()
        || command.color.is_some()
        || effect_update.is_some();
    if changed {
        light_state::update(|state| {
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
            if let Some(effect) = effect_update {
                state.effect = effect;
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

    light_state::update(|state| state.speed = speed);
    true
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
