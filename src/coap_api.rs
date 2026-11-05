//! The CoAP resources, independent of the hardware so they can be tested on a host.
//!
//! Everything except discovery requires OSCORE keyed by EDHOC. Payloads match the MQTT
//! API where a counterpart exists. Handlers run synchronously on the endpoint driver, so
//! they must only do quick work and hand anything slower to another task.

use core::{cell::RefCell, fmt::Write, net::Ipv4Addr};

use critical_section::Mutex;
use digest::Digest;
use heapless::{String, Vec};
use mai_coap::{
    Error,
    block::{BodyKey, UploadSink},
    codec::{Code, ContentFormat, OptionNumber},
    delegate_application,
    edhoc::{CredentialId, CredentialStore, TrustedPeer, endpoint::Edhoc},
    endpoint::{Application, Disposition, Endpoint, Request, ResponseWriter, Shared},
    observe::Settings,
    security::{PeerId, PublicPaths, SecurityState},
    server::{BodyChunk, BodySource, BodyWindow, Router},
};
use serde::Serialize;

use crate::{
    EFFECT_DISABLED_NAME, EFFECTS, coap_keys,
    device_config::{self, CoapCredential, DeviceConfig},
    light_command::{LightState, LightUpdate, light_state_json, parse_light_command, parse_speed},
    log_buffer::LogRead,
    ota_protocol::{ImageTarget, ImageWriter, Manifest, SLOT_SIZE},
};

pub const PORT: u16 = 5683;
/// Leaves room for a 1024-byte block plus OSCORE's option and tag.
pub const DATAGRAM_BYTES: usize = 1152;
const ROUTES: usize = 13;
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
/// A protected Block1 upload holds its security context, so each upload needs a lease.
const BODY_CONTEXTS: usize = UPLOADS;
/// An upload stalled this long ends, so that the flash becomes available again.
const UPLOAD_TIMEOUT_MS: u64 = 30_000;
/// Replies are shared between the send queue and the reply cache.
const PACKETS: usize = 8;

pub const CONTENT_JSON: ContentFormat = ContentFormat(50);

/// Observable resources, in `Handle::notify`'s numbering.
pub mod resource {
    pub const LIGHT: u32 = 1;
    pub const SPEED: u32 = 2;
    pub const TEMPERATURE: u32 = 3;
    pub const FIRMWARE: u32 = 4;
    pub const LOGS: u32 = 5;
}

pub type ApiShared = Shared<DATAGRAM_BYTES, COMMANDS>;
pub type Protection<'k, C> = Edhoc<C, CONTEXTS, HANDSHAKES, &'k ClientKeys, ID_CONTEXT_BYTES>;
pub type ApiEndpoint<'a, 'k, C> = Endpoint<
    'a,
    Protection<'k, C>,
    DATAGRAM_BYTES,
    OUTBOUND,
    DEDUP_ENTRIES,
    OBSERVERS,
    CACHED_REPLIES,
    SEND_QUEUE,
    COMMANDS,
    0,
    BODY_CONTEXTS,
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

    /// The stored form of the current list.
    fn current(&self) -> Result<ClientList, Error> {
        critical_section::with(|cs| {
            let mut list = ClientList::new();
            for peer in self.peers.borrow_ref(cs).iter() {
                list.push(coap_keys::write_hex(peer.credential_bytes())?)
                    .map_err(|_| Error::Capacity)?;
            }
            Ok(list)
        })
    }

    /// The list with one more client, or why it can't be added. Adding a key that's already
    /// there changes nothing.
    fn with_added(&self, hex: &str) -> Result<ClientList, &'static str> {
        let mut new = [0; device_config::MAX_COAP_CREDENTIAL_BYTES];
        let new = coap_keys::read_hex(hex, &mut new).map_err(|_| "invalid_key")?;
        TrustedPeer::new(PeerId(0), new).map_err(|_| "invalid_key")?;
        let new_id = coap_keys::credential_key_id(new).ok_or("invalid_key")?;

        let mut list = self.current().map_err(|_| "invalid_key")?;
        for existing in &list {
            let mut other = [0; device_config::MAX_COAP_CREDENTIAL_BYTES];
            let other = coap_keys::read_hex(existing, &mut other).map_err(|_| "invalid_key")?;
            if other == new {
                return Ok(list);
            }
            // Such a pair can't be told apart in a handshake, locking out both.
            if coap_keys::credential_key_id(other) == Some(new_id) {
                return Err("duplicate_key_id");
            }
        }
        let hex = hex.try_into().map_err(|_| "invalid_key")?;
        list.push(hex).map_err(|_| "too_many_keys")?;
        Ok(list)
    }

    /// The list without this client, or why it can't be removed. The last key stays, because
    /// losing it could only be fixed over USB.
    fn without(&self, fingerprint: &str) -> Result<ClientList, &'static str> {
        let list = self.current().map_err(|_| "unknown_key")?;
        let mut remaining = ClientList::new();
        for hex in &list {
            let mut ccs = [0; device_config::MAX_COAP_CREDENTIAL_BYTES];
            let ccs = coap_keys::read_hex(hex, &mut ccs).map_err(|_| "unknown_key")?;
            if coap_keys::fingerprint(ccs).map_err(|_| "unknown_key")? != fingerprint {
                remaining.push(hex.clone()).map_err(|_| "unknown_key")?;
            }
        }
        if remaining.len() == list.len() {
            return Err("unknown_key");
        }
        if remaining.is_empty() {
            return Err("last_key");
        }
        Ok(remaining)
    }

    /// `GET /coap/clients`: a JSON list of fingerprints.
    fn fingerprints(&self) -> Result<String<64>, Error> {
        let mut json = String::new();
        json.push('[').map_err(|_| Error::Capacity)?;
        critical_section::with(|cs| {
            for (index, peer) in self.peers.borrow_ref(cs).iter().enumerate() {
                let separator = if index == 0 { "" } else { "," };
                let fingerprint = coap_keys::fingerprint(peer.credential_bytes())?;
                write!(json, r#"{separator}"{fingerprint}""#).map_err(|_| Error::Capacity)?;
            }
            Ok::<_, Error>(())
        })?;
        json.push(']').map_err(|_| Error::Capacity)?;
        Ok(json)
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
    /// See [`LogRing::read`](crate::log_buffer::LogRing::read).
    fn read_logs(&self, from: Option<u64>, output: &mut [u8]) -> LogRead;
    /// Starts an HTTPS OTA update from the command JSON, or says why it won't.
    fn submit_ota(&self, command: &[u8]) -> Result<(), &'static str>;
    /// The settings this boot runs with.
    fn config(&self) -> &DeviceConfig;
    /// The device's own public credential in hex.
    fn device_credential(&self) -> CoapCredential;
    /// Queues a write of the settings and a reboot, after any reply has had time to leave.
    fn write_config(&self, config: DeviceConfig) -> Result<(), Busy>;
    /// Queues saving this client list. `ClientKeys` should only change once it's saved.
    fn save_clients(&self, clients: ClientList) -> Result<(), Busy>;
    /// Queues a reboot, after any reply has had time to leave.
    fn reboot(&self) -> Result<(), Busy>;
    /// Whether an uploaded image could be activated: a rollback bootloader must be there
    /// to catch one that doesn't work.
    fn upload_enabled(&self) -> bool;
    /// A request passed authentication, which an OTA trial counts as proof of health.
    fn authenticated(&self);
}

/// What a firmware upload needs from the firmware.
pub trait UploadAccess {
    type Target: ImageTarget;
    /// SHA-256, called from the endpoint driver.
    type Hash: Digest;
    /// Exclusive use of the flash, or `None` if something else is using it.
    fn acquire(&mut self) -> Option<Self::Target>;
    /// Publishes an OTA status.
    fn status(&mut self, state: &str, detail: &str, percent: u8);
    /// A block was stored, so the upload hasn't hung.
    fn progress(&mut self);
    /// The upload ended and any `Target` has been dropped. `activated` means a reboot into
    /// the new image is due.
    fn finished(&mut self, activated: bool);
}

/// The manifest of the upload that comes next.
pub struct PendingManifest(Mutex<RefCell<Option<Manifest>>>);

impl PendingManifest {
    pub const fn new() -> Self {
        Self(Mutex::new(RefCell::new(None)))
    }
    fn set(&self, manifest: Option<Manifest>) {
        critical_section::with(|cs| *self.0.borrow_ref_mut(cs) = manifest);
    }
    fn take(&self) -> Option<Manifest> {
        critical_section::with(|cs| self.0.borrow_ref_mut(cs).take())
    }
}

impl Default for PendingManifest {
    fn default() -> Self {
        Self::new()
    }
}

struct Upload<T, H> {
    writer: ImageWriter<H>,
    target: T,
    last_percent: u8,
}

/// `/firmware/image`. Runs on the endpoint driver, but flash calls would stall everything
/// sharing the executor wherever they ran, and this keeps replies piggybacked.
struct FirmwareUpload<'d, A: UploadAccess> {
    access: A,
    manifest: &'d PendingManifest,
    upload: Option<Upload<A::Target, A::Hash>>,
    /// A failed write has said why, which the abort following it shouldn't overwrite.
    failure_reported: bool,
}

impl<A: UploadAccess> FirmwareUpload<'_, A> {
    fn end(&mut self, failure: Option<&str>) {
        if let Some(upload) = self.upload.take() {
            // The flash must be free before the job slot is.
            drop(upload);
            if let Some(reason) = failure {
                self.access.status("failed", reason, 0);
            }
            self.access.finished(false);
        }
    }
}

/// The client caused this, so it's told; the status says exactly what happened.
fn rejected_image() -> Error {
    Error::Response(Code::BAD_REQUEST)
}

impl<A: UploadAccess> UploadSink for FirmwareUpload<'_, A> {
    fn begin(&mut self, _: &BodyKey, size: Option<u64>) -> Result<(), Error> {
        self.end(Some("restarted"));
        self.failure_reported = false;
        let manifest = self
            .manifest
            .take()
            .ok_or(Error::Response(Code::PRECONDITION_FAILED))?;
        if size.is_some_and(|size| size != u64::from(manifest.size)) {
            return Err(rejected_image());
        }
        let Some(target) = self.access.acquire() else {
            // It stays valid for when the flash is free.
            self.manifest.set(Some(manifest));
            return Err(Error::Capacity);
        };
        self.upload = Some(Upload {
            writer: ImageWriter::new(&manifest),
            target,
            last_percent: 0,
        });
        self.access.status("uploading", "", 0);
        Ok(())
    }

    fn write(&mut self, _: usize, bytes: &[u8]) -> Result<(), Error> {
        let upload = self.upload.as_mut().ok_or(Error::Malformed)?;
        if let Err(error) = upload.writer.push(&mut upload.target, bytes) {
            self.access.status("failed", error.reason(), 0);
            self.failure_reported = true;
            return Err(rejected_image());
        }
        let percent = upload.writer.percent();
        if percent / 10 != upload.last_percent / 10 {
            upload.last_percent = percent;
            self.access.status("uploading", "", percent);
        }
        self.access.progress();
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        let Upload {
            writer, mut target, ..
        } = self.upload.take().ok_or(Error::Malformed)?;
        self.access.status("verifying", "flash_readback", 100);
        let result = writer.finish(&mut target);
        drop(target);
        match result {
            Ok(()) => {
                self.access.status("rebooting", "verified", 100);
                self.access.finished(true);
                Ok(())
            }
            Err(error) => {
                self.access.status("failed", error.reason(), 0);
                self.access.finished(false);
                Err(rejected_image())
            }
        }
    }

    fn abort(&mut self) {
        let failure = (!self.failure_reported).then_some("abandoned");
        self.end(failure);
    }
}

/// Flash is taken by another job, such as an update in progress.
#[derive(Debug)]
pub struct Busy;

pub type ClientList = Vec<CoapCredential, { device_config::MAX_COAP_CLIENTS }>;

/// The settings as `GET /config` shows them. Passwords and the device key stay out.
#[derive(Serialize)]
struct ConfigSummary<'a> {
    schema: u16,
    generation: u32,
    wifi_ssid: &'a str,
    has_wifi_password: bool,
    mqtt_broker: &'a str,
    mqtt_port: u16,
    mqtt_tls: bool,
    mqtt_username: &'a str,
    has_mqtt_password: bool,
    device_credential: &'a str,
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
    Logs,
    FirmwareUpdate,
    Config,
    Clients,
    Reboot,
    FirmwareManifest,
}

struct Handler<'d, D> {
    device: &'d D,
    keys: &'d ClientKeys,
    manifest: &'d PendingManifest,
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

/// An error with a reason for the client. Returning an `Err` would drop the payload.
fn refuse(
    response: &mut ResponseWriter<'_, DATAGRAM_BYTES>,
    code: Code,
    reason: &str,
) -> Result<Disposition, Code> {
    response.set_code(code);
    response.set_uint_option(OptionNumber::CONTENT_FORMAT, ContentFormat::TEXT.0.into())?;
    response.set_payload(reason.as_bytes())?;
    Ok(Disposition::Complete)
}

fn busy(response: &mut ResponseWriter<'_, DATAGRAM_BYTES>) -> Result<Disposition, Code> {
    response.set_uint_option(OptionNumber::MAX_AGE, 5)?;
    refuse(response, Code::UNAVAILABLE, "busy")
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
            Kind::Logs => {
                // Not Block2: its ETag would change as new lines arrive during a transfer.
                let mut payload = [0; LOG_PAYLOAD_BYTES];
                let (header, text) = payload.split_at_mut(LOG_HEADER_BYTES);
                let read = self.device.read_logs(log_cursor(request)?, text);
                let mut first_line: String<LOG_HEADER_BYTES> = String::new();
                writeln!(first_line, "{} {}", read.start, read.next)
                    .map_err(|_| Code::INTERNAL_ERROR)?;
                let count = (read.next - read.start) as usize;
                // Right-align the first line against the text, so neither has to move.
                let at = LOG_HEADER_BYTES - first_line.len();
                header[at..].copy_from_slice(first_line.as_bytes());
                reply(
                    response,
                    ContentFormat::TEXT,
                    &payload[at..LOG_HEADER_BYTES + count],
                )
            }
            Kind::FirmwareUpdate => match self.device.submit_ota(request.payload()) {
                Ok(()) => changed(response),
                Err(reason @ ("busy" | "unavailable")) => {
                    refuse(response, Code::UNAVAILABLE, reason)
                }
                Err(reason) => refuse(response, Code::BAD_REQUEST, reason),
            },
            Kind::Config if write => {
                let Ok(config) = DeviceConfig::decode(request.payload()) else {
                    return refuse(response, Code::BAD_REQUEST, "invalid_settings");
                };
                // Client keys have their own resource, and the device key stays put.
                if !config.coap_device_key.is_empty() || !config.coap_clients.is_empty() {
                    return refuse(response, Code::BAD_REQUEST, "coap_fields_not_allowed");
                }
                match self.device.write_config(config) {
                    Ok(()) => changed(response),
                    Err(Busy) => busy(response),
                }
            }
            Kind::Config => {
                let config = self.device.config();
                let credential = self.device.device_credential();
                let summary = ConfigSummary {
                    schema: config.schema,
                    generation: config.generation,
                    wifi_ssid: &config.wifi_ssid,
                    has_wifi_password: !config.wifi_password.is_empty(),
                    mqtt_broker: &config.mqtt_broker,
                    mqtt_port: config.mqtt_port,
                    mqtt_tls: config.mqtt_tls,
                    mqtt_username: &config.mqtt_username,
                    has_mqtt_password: !config.mqtt_password.is_empty(),
                    device_credential: &credential,
                };
                let mut json = [0; CONFIG_JSON_BYTES];
                let length = serde_json_core::to_slice(&summary, &mut json)
                    .map_err(|_| Code::INTERNAL_ERROR)?;
                reply(response, CONTENT_JSON, &json[..length])
            }
            Kind::Clients => match request.message.code() {
                Code::POST => {
                    let hex =
                        core::str::from_utf8(request.payload()).map_err(|_| Code::BAD_REQUEST)?;
                    match self.keys.with_added(hex.trim()) {
                        Ok(clients) => match self.device.save_clients(clients) {
                            Ok(()) => changed(response),
                            Err(Busy) => busy(response),
                        },
                        Err(reason) => refuse(response, Code::BAD_REQUEST, reason),
                    }
                }
                Code::DELETE => {
                    let fingerprint = query_value(request, "fp")?.ok_or(Code::BAD_REQUEST)?;
                    match self.keys.without(fingerprint) {
                        Ok(clients) => match self.device.save_clients(clients) {
                            Ok(()) => {
                                response.set_code(Code::DELETED);
                                Ok(Disposition::Complete)
                            }
                            Err(Busy) => busy(response),
                        },
                        Err("unknown_key") => refuse(response, Code::NOT_FOUND, "unknown_key"),
                        Err(reason) => refuse(response, Code::BAD_REQUEST, reason),
                    }
                }
                _ => {
                    let list = self.keys.fingerprints().map_err(|_| Code::INTERNAL_ERROR)?;
                    reply(response, CONTENT_JSON, list.as_bytes())
                }
            },
            Kind::FirmwareManifest => {
                if !self.device.upload_enabled() {
                    return refuse(response, Code::UNAVAILABLE, "unavailable");
                }
                let Ok(manifest) = Manifest::parse(request.payload()) else {
                    return refuse(response, Code::BAD_REQUEST, "invalid_command");
                };
                if manifest.build == env!("FIRMWARE_BUILD_ID") {
                    return refuse(response, Code::BAD_REQUEST, "already_running");
                }
                self.manifest.set(Some(manifest));
                changed(response)
            }
            Kind::Reboot => match self.device.reboot() {
                Ok(()) => changed(response),
                Err(Busy) => busy(response),
            },
        }
    }

    fn recognizes(&self, _: OptionNumber) -> bool {
        false
    }
}

/// The value of a `name=value` query, if present. Any other query is an error, because a
/// client that misspelled one would otherwise get something it didn't ask for.
fn query_value<'r>(request: Request<'r>, name: &str) -> Result<Option<&'r str>, Code> {
    let mut found = None;
    for query in request.message.values(OptionNumber::URI_QUERY) {
        let value = core::str::from_utf8(query)
            .ok()
            .and_then(|query| query.strip_prefix(name)?.strip_prefix('='))
            .ok_or(Code::BAD_REQUEST)?;
        found = Some(value);
    }
    Ok(found)
}

/// The `from=N` query of `/logs`, if any.
fn log_cursor(request: Request<'_>) -> Result<Option<u64>, Code> {
    query_value(request, "from")?
        .map(|value| value.parse().map_err(|_| Code::BAD_REQUEST))
        .transpose()
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

/// A first line of two `u64` offsets, and text up to the 1024 bytes a block would hold.
const LOG_HEADER_BYTES: usize = 42;
const LOG_PAYLOAD_BYTES: usize = 1024;

const CONFIG_JSON_BYTES: usize = 1000;

const READ: &[Code] = &[Code::GET];
const WRITE: &[Code] = &[Code::POST];
const PUT: &[Code] = &[Code::PUT];
const CLIENTS: &[Code] = &[Code::GET, Code::POST, Code::DELETE];
const READ_WRITE: &[Code] = &[Code::GET, Code::PUT];
const JSON_OBSERVABLE: &[(&str, Option<&str>)] = &[("ct", Some("50")), ("obs", None)];
const TEXT_OBSERVABLE: &[(&str, Option<&str>)] = &[("ct", Some("0")), ("obs", None)];
const JSON: &[(&str, Option<&str>)] = &[("ct", Some("50"))];

/// Storage for the resources, which the router borrows.
pub struct Resources<'d, D, A: UploadAccess> {
    light: Handler<'d, D>,
    speed: Handler<'d, D>,
    temperature: Handler<'d, D>,
    info: Handler<'d, D>,
    firmware: Handler<'d, D>,
    logs: Handler<'d, D>,
    firmware_update: Handler<'d, D>,
    config: Handler<'d, D>,
    clients: Handler<'d, D>,
    reboot: Handler<'d, D>,
    firmware_manifest: Handler<'d, D>,
    firmware_image: FirmwareUpload<'d, A>,
    effects: EffectList,
}

impl<'d, D: Device, A: UploadAccess> Resources<'d, D, A> {
    pub fn new(
        device: &'d D,
        keys: &'d ClientKeys,
        manifest: &'d PendingManifest,
        access: A,
    ) -> Self {
        let handler = |kind| Handler {
            device,
            keys,
            manifest,
            kind,
        };
        Self {
            light: handler(Kind::Light),
            speed: handler(Kind::Speed),
            temperature: handler(Kind::Temperature),
            info: handler(Kind::Info),
            firmware: handler(Kind::Firmware),
            logs: handler(Kind::Logs),
            firmware_update: handler(Kind::FirmwareUpdate),
            config: handler(Kind::Config),
            clients: handler(Kind::Clients),
            reboot: handler(Kind::Reboot),
            firmware_manifest: handler(Kind::FirmwareManifest),
            firmware_image: FirmwareUpload {
                access,
                manifest,
                upload: None,
                failure_reported: false,
            },
            effects: EffectList,
        }
    }

    pub fn register<'r>(&'r mut self, router: &mut ApiRouter<'r>) -> Result<(), Error> {
        router.route("/light", READ_WRITE, &mut self.light)?;
        router.route("/speed", READ_WRITE, &mut self.speed)?;
        router.route("/temperature", READ, &mut self.temperature)?;
        router.route("/info", READ, &mut self.info)?;
        router.route("/firmware", READ, &mut self.firmware)?;
        router.route("/logs", READ, &mut self.logs)?;
        router.route("/firmware/update", WRITE, &mut self.firmware_update)?;
        router.buffered_upload("/config", READ_WRITE, &mut self.config)?;
        router.route("/coap/clients", CLIENTS, &mut self.clients)?;
        router.route("/reboot", WRITE, &mut self.reboot)?;
        router.route("/firmware/manifest", PUT, &mut self.firmware_manifest)?;
        router.streaming_upload(
            "/firmware/image",
            PUT,
            SLOT_SIZE as usize,
            &mut self.firmware_image,
        )?;
        router.set_upload_timeout(UPLOAD_TIMEOUT_MS)?;
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
        // Each notification carries a whole block, so there's no need for more than this.
        router.observable("/logs", notify(resource::LOGS).with_minimum_ms(1000))?;

        router.describe("/light", JSON_OBSERVABLE, false)?;
        router.describe("/speed", TEXT_OBSERVABLE, false)?;
        router.describe("/temperature", TEXT_OBSERVABLE, false)?;
        router.describe("/firmware", JSON_OBSERVABLE, false)?;
        router.describe("/logs", TEXT_OBSERVABLE, false)?;
        router.describe("/info", JSON, false)?;
        router.describe("/config", JSON, false)?;
        router.describe("/coap/clients", JSON, false)?;
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
