//! The CoAP resources, independent of the hardware so they can be tested on a host.
//!
//! Everything except discovery requires OSCORE keyed by EDHOC. Payloads match the MQTT
//! API where a counterpart exists. Handlers run synchronously on the endpoint driver, so
//! they must only do quick work and hand anything slower to another task.

use core::{cell::RefCell, fmt::Write, net::Ipv4Addr};

use critical_section::Mutex;
use heapless::{String, Vec};
use mai_coap::{
    Error,
    codec::{Code, ContentFormat, OptionNumber},
    crypto::SoftwareCrypto,
    delegate_application,
    edhoc::{CredentialId, CredentialStore, TrustedPeer, endpoint::Edhoc},
    endpoint::{Application, Disposition, Endpoint, Request, ResponseWriter, Shared},
    observe::Settings,
    security::{PeerId, PublicPaths, SecurityState},
    server::{BodyChunk, BodySource, BodyWindow, Router},
};

use crate::{
    EFFECT_DISABLED_NAME, EFFECTS, device_config,
    light_command::{LightState, LightUpdate, light_state_json, parse_light_command, parse_speed},
};

pub const PORT: u16 = 5683;
/// Leaves room for a 1024-byte block plus OSCORE's option and tag.
pub const DATAGRAM_BYTES: usize = 1152;
const ROUTES: usize = 6;
const UPLOADS: usize = 2;
const UPLOAD_BYTES: usize = 2048;
/// Keep one context per client, plus one while it runs EDHOC again.
const CONTEXTS: usize = 2;
const HANDSHAKES: usize = 2;
/// EDHOC derives no OSCORE ID Context.
const ID_CONTEXT_BYTES: usize = 0;
/// Lets observers get notifications, one at a time for each.
const OUTBOUND: usize = 4;
const DEDUP_ENTRIES: usize = 16;
const OBSERVERS: usize = 4;
const CACHED_REPLIES: usize = 4;
const SEND_QUEUE: usize = 4;
/// `Handle::notify` needs no command slots, and the device starts no requests.
const COMMANDS: usize = 0;
/// Replies are shared between the send queue and the reply cache.
const PACKETS: usize = 8;

pub const CONTENT_JSON: ContentFormat = ContentFormat(50);

/// Observable resources, in `Handle::notify`'s numbering.
pub mod resource {
    pub const LIGHT: u32 = 1;
    pub const SPEED: u32 = 2;
    pub const TEMPERATURE: u32 = 3;
    pub const FIRMWARE: u32 = 4;
}

pub type ApiShared = Shared<DATAGRAM_BYTES, COMMANDS>;
pub type Protection<'k, R> =
    Edhoc<SoftwareCrypto<R>, CONTEXTS, HANDSHAKES, &'k ClientKeys, ID_CONTEXT_BYTES>;
pub type ApiEndpoint<'a, 'k, R> = Endpoint<
    'a,
    Protection<'k, R>,
    DATAGRAM_BYTES,
    OUTBOUND,
    DEDUP_ENTRIES,
    OBSERVERS,
    CACHED_REPLIES,
    SEND_QUEUE,
    COMMANDS,
    0,
    0,
    PACKETS,
>;
pub type ApiRouter<'a> = Router<'a, DATAGRAM_BYTES, ROUTES, UPLOADS, UPLOAD_BYTES>;

/// Only discovery is public, and it lists only what the caller may access.
pub static POLICY: PublicPaths<'static> = PublicPaths(&[(Code::GET, "/.well-known/core")]);

/// The clients allowed to connect. Shared with handlers, so that changes apply to the next
/// handshake; contexts already established keep working until they are retired.
pub struct ClientKeys {
    peers: Mutex<RefCell<Vec<TrustedPeer, { device_config::MAX_COAP_CLIENTS }>>>,
}

impl ClientKeys {
    pub const fn new() -> Self {
        Self {
            peers: Mutex::new(RefCell::new(Vec::new())),
        }
    }

    /// Replaces the clients with these hex CCS credentials, skipping any that aren't usable.
    /// Returns how many were skipped.
    pub fn load(&self, credentials: &[device_config::CoapCredential]) -> usize {
        let mut peers = Vec::new();
        let mut skipped = 0;
        for (index, hex) in credentials.iter().enumerate() {
            let mut ccs = [0; device_config::MAX_COAP_CREDENTIAL_BYTES];
            let peer = crate::coap_keys::read_hex(hex, &mut ccs)
                .and_then(|ccs| TrustedPeer::new(PeerId(index as u32 + 1), ccs));
            let added = match peer {
                Ok(peer) => peers.push(peer).is_ok(),
                Err(_) => false,
            };
            if !added {
                skipped += 1;
            }
        }
        critical_section::with(|cs| *self.peers.borrow_ref_mut(cs) = peers);
        skipped
    }
}

impl Default for ClientKeys {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for ClientKeys {
    fn get(&self, peer: PeerId) -> Option<TrustedPeer> {
        critical_section::with(|cs| CredentialStore::get(&*self.peers.borrow_ref(cs), peer))
    }
    fn find(&self, id: CredentialId<'_>) -> Option<TrustedPeer> {
        critical_section::with(|cs| CredentialStore::find(&*self.peers.borrow_ref(cs), id))
    }
}

/// What the resources need from the rest of the firmware. Every method must return quickly.
pub trait Device {
    fn light(&self) -> LightState;
    fn update_light(&self, update: &LightUpdate);
    fn set_speed(&self, speed: u8);
    /// Latest die temperature in °C, if measured yet.
    fn temperature(&self) -> Option<f32>;
    /// Latest OTA status as JSON.
    fn ota_status(&self) -> Option<String<192>>;
    fn info(&self) -> Info<'_>;
    /// A request passed authentication, which an OTA trial counts as proof of health.
    fn authenticated(&self);
}

pub struct Info<'a> {
    pub name: &'a str,
    pub uptime_s: u64,
    pub ip: Option<Ipv4Addr>,
    pub free_heap: usize,
}

#[derive(Clone, Copy)]
enum Kind {
    Light,
    Speed,
    Temperature,
    Info,
    Firmware,
}

struct Handler<'d, D> {
    device: &'d D,
    kind: Kind,
}

fn reply(
    response: &mut ResponseWriter<'_, DATAGRAM_BYTES>,
    format: ContentFormat,
    payload: &[u8],
) -> Result<Disposition, Code> {
    response.set_code(Code::CONTENT);
    response.set_uint_option(OptionNumber::CONTENT_FORMAT, format.0.into())?;
    response.set_payload(payload)?;
    Ok(Disposition::Complete)
}

fn changed(response: &mut ResponseWriter<'_, DATAGRAM_BYTES>) -> Result<Disposition, Code> {
    response.set_code(Code::CHANGED);
    Ok(Disposition::Complete)
}

impl<D: Device> Application<DATAGRAM_BYTES> for Handler<'_, D> {
    fn handle(
        &mut self,
        request: Request<'_>,
        response: &mut ResponseWriter<'_, DATAGRAM_BYTES>,
    ) -> Result<Disposition, Code> {
        let write = request.message.code() != Code::GET;
        match self.kind {
            Kind::Light if write => {
                let update = parse_light_command(request.payload()).ok_or(Code::BAD_REQUEST)?;
                self.device.update_light(&update);
                changed(response)
            }
            Kind::Light => reply(
                response,
                CONTENT_JSON,
                light_state_json(&self.device.light()).as_bytes(),
            ),
            Kind::Speed if write => {
                let speed = parse_speed(request.payload()).ok_or(Code::BAD_REQUEST)?;
                self.device.set_speed(speed);
                changed(response)
            }
            Kind::Speed => {
                let speed: String<3> = heapless::format!("{}", self.device.light().speed)
                    .map_err(|_| Code::INTERNAL_ERROR)?;
                reply(response, ContentFormat::TEXT, speed.as_bytes())
            }
            Kind::Temperature => {
                let celsius = self.device.temperature().ok_or(Code::UNAVAILABLE)?;
                let text: String<12> =
                    heapless::format!("{celsius:.1}").map_err(|_| Code::INTERNAL_ERROR)?;
                reply(response, ContentFormat::TEXT, text.as_bytes())
            }
            Kind::Info => {
                let info = self.device.info();
                let mut json: String<256> = String::new();
                write!(
                    json,
                    r#"{{"name":"{}","build":"{}","hardware":"{}","uptime_s":{},"free_heap":{}"#,
                    info.name,
                    env!("FIRMWARE_BUILD_ID"),
                    env!("FIRMWARE_HARDWARE_ID"),
                    info.uptime_s,
                    info.free_heap,
                )
                .map_err(|_| Code::INTERNAL_ERROR)?;
                if let Some(ip) = info.ip {
                    write!(json, r#","ip":"{ip}""#).map_err(|_| Code::INTERNAL_ERROR)?;
                }
                json.push('}').map_err(|_| Code::INTERNAL_ERROR)?;
                reply(response, CONTENT_JSON, json.as_bytes())
            }
            Kind::Firmware => {
                let status = self.device.ota_status().ok_or(Code::UNAVAILABLE)?;
                reply(response, CONTENT_JSON, status.as_bytes())
            }
        }
    }

    fn recognizes(&self, _: OptionNumber) -> bool {
        false
    }
}

/// `/effects`: names as `["None","Rainbow",…]`, the same list as Home Assistant's.
struct EffectList;

impl BodySource for EffectList {
    fn read_at(
        &mut self,
        _: Request<'_>,
        offset: usize,
        output: &mut [u8],
    ) -> Result<BodyChunk, Code> {
        let mut window = BodyWindow::new(offset, output);
        window.write(b"[\"")?;
        window.write(EFFECT_DISABLED_NAME.as_bytes())?;
        for (_, name) in EFFECTS {
            window.write(b"\",\"")?;
            window.write(name.as_bytes())?;
        }
        window.write(b"\"]")?;
        window.finish(CONTENT_JSON, 3600)
    }
}

const READ: &[Code] = &[Code::GET];
const READ_WRITE: &[Code] = &[Code::GET, Code::PUT];
const JSON_OBSERVABLE: &[(&str, Option<&str>)] = &[("ct", Some("50")), ("obs", None)];
const TEXT_OBSERVABLE: &[(&str, Option<&str>)] = &[("ct", Some("0")), ("obs", None)];
const JSON: &[(&str, Option<&str>)] = &[("ct", Some("50"))];

/// Storage for the resources, which the router borrows.
pub struct Resources<'d, D> {
    light: Handler<'d, D>,
    speed: Handler<'d, D>,
    temperature: Handler<'d, D>,
    info: Handler<'d, D>,
    firmware: Handler<'d, D>,
    effects: EffectList,
}

impl<'d, D: Device> Resources<'d, D> {
    pub fn new(device: &'d D) -> Self {
        let handler = |kind| Handler { device, kind };
        Self {
            light: handler(Kind::Light),
            speed: handler(Kind::Speed),
            temperature: handler(Kind::Temperature),
            info: handler(Kind::Info),
            firmware: handler(Kind::Firmware),
            effects: EffectList,
        }
    }

    pub fn register<'r>(&'r mut self, router: &mut ApiRouter<'r>) -> Result<(), Error> {
        router.route("/light", READ_WRITE, &mut self.light)?;
        router.route("/speed", READ_WRITE, &mut self.speed)?;
        router.route("/temperature", READ, &mut self.temperature)?;
        router.route("/info", READ, &mut self.info)?;
        router.route("/firmware", READ, &mut self.firmware)?;
        router.representation("/effects", &mut self.effects)?;

        let notify = |id| {
            Settings::new(id)
                .with_minimum_ms(100)
                .with_maximum_ms(120_000)
                .with_confirmable_ms(60_000)
        };
        router.observable("/light", notify(resource::LIGHT))?;
        router.observable("/speed", notify(resource::SPEED))?;
        router.observable("/temperature", notify(resource::TEMPERATURE))?;
        router.observable("/firmware", notify(resource::FIRMWARE))?;

        router.describe("/light", JSON_OBSERVABLE, false)?;
        router.describe("/speed", TEXT_OBSERVABLE, false)?;
        router.describe("/temperature", TEXT_OBSERVABLE, false)?;
        router.describe("/firmware", JSON_OBSERVABLE, false)?;
        router.describe("/info", JSON, false)?;
        router.describe("/effects", JSON, false)?;
        Ok(())
    }
}

/// Wraps the router to tell the device about authenticated requests.
pub struct Authenticated<'d, 'r, D> {
    pub device: &'d D,
    pub router: ApiRouter<'r>,
}

impl<D: Device> Application<DATAGRAM_BYTES> for Authenticated<'_, '_, D> {
    delegate_application!(router, DATAGRAM_BYTES; except [handle]);

    fn handle(
        &mut self,
        request: Request<'_>,
        response: &mut ResponseWriter<'_, DATAGRAM_BYTES>,
    ) -> Result<Disposition, Code> {
        if matches!(request.security, SecurityState::Protected { .. }) {
            self.device.authenticated();
        }
        self.router.handle(request, response)
    }
}

#[cfg(test)]
mod tests;
