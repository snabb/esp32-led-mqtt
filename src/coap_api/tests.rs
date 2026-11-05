//! A real server and client talking EDHOC and OSCORE over loopback UDP.

use core::{cell::Cell, net::SocketAddr};
use std::format;
use std::vec::Vec as StdVec;

use futures::{
    executor::block_on,
    future::{Either, select},
    pin_mut,
};
use mai_coap::{
    codec::Code,
    crypto::SoftwareCrypto,
    edhoc::{CcsEncoding, Identity, TrustedPeers},
    endpoint::{DefaultEndpoint, DefaultShared, Handle},
    security::ProtectedOnly,
    time::StdClock,
    transport::linux::StdTransport,
};
use smart_leds::RGB8;

use super::*;
use crate::{
    EffectId,
    coap_crypto::{
        AcceleratedCrypto,
        tests::{FakeAccelerator, TestRng},
    },
    log_buffer::LogRing,
    ota_protocol::{IMAGE_PREFIX_SIZE, PROJECT, TargetError},
};

const DEVICE_KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const CLIENT_KEY: [u8; 32] = [9; 32];
const OTHER_KEY: [u8; 32] = [10; 32];
const DEVICE: PeerId = PeerId(1);

fn crypto(seed: u64) -> SoftwareCrypto<TestRng> {
    SoftwareCrypto::new(TestRng(seed))
}

struct FakeDevice {
    light: Cell<LightState>,
    authenticated: Cell<bool>,
    logs: RefCell<LogRing<4096>>,
    config: DeviceConfig,
    ota_result: Cell<Result<(), &'static str>>,
    ota_commands: RefCell<StdVec<StdVec<u8>>>,
    busy: Cell<bool>,
    written_config: RefCell<Option<DeviceConfig>>,
    saved_clients: RefCell<Option<ClientList>>,
    reboots: Cell<u32>,
    upload_enabled: Cell<bool>,
    flash: FakeFlash,
}

/// What a firmware upload did to the flash.
#[derive(Default)]
struct FlashLog {
    begun: Cell<u32>,
    bytes: RefCell<StdVec<u8>>,
    activated: Cell<bool>,
    finished: RefCell<StdVec<bool>>,
    statuses: RefCell<StdVec<(std::string::String, std::string::String)>>,
    held_by_upload: Cell<bool>,
    held_by_other: Cell<bool>,
    progress: Cell<u32>,
}

type FakeFlash = std::rc::Rc<FlashLog>;

struct FakeTarget(FakeFlash);

impl Drop for FakeTarget {
    fn drop(&mut self) {
        self.0.held_by_upload.set(false);
    }
}

impl ImageTarget for FakeTarget {
    fn begin(&mut self, _: u32, _: u32) -> Result<(), TargetError> {
        self.0.begun.set(self.0.begun.get() + 1);
        self.0.bytes.borrow_mut().clear();
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), TargetError> {
        self.0.bytes.borrow_mut().extend_from_slice(bytes);
        Ok(())
    }
    fn activate(&mut self) -> Result<(), TargetError> {
        self.0.activated.set(true);
        Ok(())
    }
}

struct FakeAccess(FakeFlash);

impl UploadAccess for FakeAccess {
    type Target = FakeTarget;
    type Hash = sha2::Sha256;
    fn acquire(&mut self) -> Option<FakeTarget> {
        if self.0.held_by_other.get() || self.0.held_by_upload.get() {
            return None;
        }
        self.0.held_by_upload.set(true);
        Some(FakeTarget(self.0.clone()))
    }
    fn status(&mut self, state: &str, detail: &str, _: u8) {
        self.0
            .statuses
            .borrow_mut()
            .push((state.into(), detail.into()));
    }
    fn progress(&mut self) {
        self.0.progress.set(self.0.progress.get() + 1);
    }
    fn finished(&mut self, activated: bool) {
        // The flash must be free before the job slot is.
        assert!(!self.0.held_by_upload.get());
        self.0.finished.borrow_mut().push(activated);
    }
}

const SETTINGS: &[u8] = br#"{"wifi_ssid":"home","wifi_password":"wifi-secret","mqtt_broker":"mqtt.example","mqtt_username":"me","mqtt_password":"mqtt-secret"}"#;

impl FakeDevice {
    fn new() -> Self {
        Self {
            light: Cell::new(LightState {
                on: false,
                brightness: 8,
                effect: Some(EffectId::Rainbow),
                speed: 64,
                color: RGB8 { r: 1, g: 2, b: 3 },
            }),
            authenticated: Cell::new(false),
            logs: RefCell::new(LogRing::new()),
            config: {
                let mut config = DeviceConfig::decode(SETTINGS).unwrap();
                config.coap_device_key = DEVICE_KEY.try_into().unwrap();
                config.wifi_ssid = "he said \"hi\"".try_into().unwrap();
                config
            },
            ota_result: Cell::new(Ok(())),
            ota_commands: RefCell::new(StdVec::new()),
            busy: Cell::new(false),
            written_config: RefCell::new(None),
            saved_clients: RefCell::new(None),
            reboots: Cell::new(0),
            upload_enabled: Cell::new(true),
            flash: FakeFlash::default(),
        }
    }
}

impl Device for FakeDevice {
    fn light(&self) -> LightState {
        self.light.get()
    }
    fn update_light(&self, update: &LightUpdate) {
        let mut light = self.light.get();
        update.apply(&mut light);
        self.light.set(light);
    }
    fn set_speed(&self, speed: u8) {
        let mut light = self.light.get();
        light.speed = speed;
        self.light.set(light);
    }
    fn temperature(&self) -> Option<f32> {
        Some(21.25)
    }
    fn ota_status(&self) -> Option<String<192>> {
        Some("{\"state\":\"ready\"}".try_into().unwrap())
    }
    fn info(&self) -> Info<'_> {
        Info {
            name: "esp32-led-000102",
            uptime_s: 7,
            ip: Some(Ipv4Addr::new(192, 0, 2, 1)),
            free_heap: 1234,
        }
    }
    fn read_logs(&self, from: Option<u64>, output: &mut [u8]) -> LogRead {
        self.logs.borrow().read(from, output)
    }
    fn submit_ota(&self, command: &[u8]) -> Result<(), &'static str> {
        self.ota_commands.borrow_mut().push(command.to_vec());
        self.ota_result.get()
    }
    fn config(&self) -> &DeviceConfig {
        &self.config
    }
    fn device_credential(&self) -> CoapCredential {
        "abcd".try_into().unwrap()
    }
    fn write_config(&self, config: DeviceConfig) -> Result<(), Busy> {
        if self.busy.get() {
            return Err(Busy);
        }
        *self.written_config.borrow_mut() = Some(config);
        Ok(())
    }
    fn save_clients(&self, clients: ClientList) -> Result<(), Busy> {
        if self.busy.get() {
            return Err(Busy);
        }
        *self.saved_clients.borrow_mut() = Some(clients);
        Ok(())
    }
    fn reboot(&self) -> Result<(), Busy> {
        if self.busy.get() {
            return Err(Busy);
        }
        self.reboots.set(self.reboots.get() + 1);
        Ok(())
    }
    fn upload_enabled(&self) -> bool {
        self.upload_enabled.get()
    }
    fn authenticated(&self) {
        self.authenticated.set(true);
    }
}

type Client<'a> = Handle<'a, DATAGRAM_BYTES, { mai_coap::DEFAULT_OUTBOUND }>;
type Notifier<'a> = Handle<'a, DATAGRAM_BYTES, 0>;

fn client_identity(key: [u8; 32], kid: &[u8]) -> Identity {
    Identity::from_private_key(key, kid, CcsEncoding::FullXY, &mut crypto(0)).unwrap()
}

fn hex(bytes: &[u8]) -> device_config::CoapCredential {
    let mut output = String::new();
    for byte in bytes {
        write!(output, "{byte:02x}").unwrap();
    }
    output
}

/// Runs a server and a client using `client_key`, until `check` finishes.
fn session_with_notifier<F: AsyncFnOnce(Client<'_>, SocketAddr, Notifier<'_>)>(
    device: &FakeDevice,
    client_key: [u8; 32],
    keys: &ClientKeys,
    check: F,
) {
    block_on(async {
        let mut server_socket = StdTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let server_address = server_socket.local_addr().unwrap();
        let mut client_socket = StdTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let clock = StdClock::default();

        // The device's provider, with a stand-in for the hardware: every session here runs
        // through the same code as on a device.
        let accelerator = FakeAccelerator::new();
        let accelerator_calls = accelerator.calls.clone();
        let mut device_crypto = AcceleratedCrypto::new(accelerator, TestRng(2));
        let device_identity = coap_keys::identity(DEVICE_KEY, &mut device_crypto).unwrap();
        let device_credential = device_identity.credential_bytes().to_vec();
        let protection = Edhoc::with_store(device_crypto, device_identity, keys).unwrap();
        let shared = ApiShared::new();
        let mut endpoint = ApiEndpoint::new(&shared, protection).unwrap();
        let manifest = PendingManifest::new();
        let mut resources =
            Resources::new(device, keys, &manifest, FakeAccess(device.flash.clone()));
        let mut router = ApiRouter::new(&POLICY);
        resources.register(&mut router).unwrap();
        let mut application = Authenticated { device, router };

        let mut client_protection = Edhoc::<_, 4, 4, TrustedPeers<1>, 0>::with_store(
            crypto(3),
            client_identity(client_key, &[0x01]),
            TrustedPeers::new(),
        )
        .unwrap();
        client_protection
            .trust(TrustedPeer::new(DEVICE, &device_credential).unwrap())
            .unwrap();
        let client_shared = DefaultShared::new();
        let mut client = DefaultEndpoint::new(&client_shared, client_protection).unwrap();
        let mut no_resources =
            |_: Request<'_>, _: &mut ResponseWriter<'_, DATAGRAM_BYTES>| Err(Code::NOT_FOUND);

        let server = endpoint.run(&mut server_socket, &clock, &mut application, &POLICY);
        let client_driver = client.run(
            &mut client_socket,
            &clock,
            &mut no_resources,
            &ProtectedOnly,
        );
        let test = check(client_shared.handle(), server_address, shared.handle());
        pin_mut!(server, client_driver, test);
        match select(test, select(server, client_driver)).await {
            Either::Left(_) => {}
            Either::Right(_) => panic!("a driver stopped"),
        }
        assert!(accelerator_calls.multiplications.get() >= 1);
    });
}

fn session<F: AsyncFnOnce(Client<'_>, SocketAddr)>(
    device: &FakeDevice,
    client_key: [u8; 32],
    keys: &ClientKeys,
    check: F,
) {
    session_with_notifier(device, client_key, keys, async |client, server, _| {
        check(client, server).await
    });
}

fn trusted_keys() -> ClientKeys {
    let keys = ClientKeys::new();
    let credential = hex(client_identity(CLIENT_KEY, &[0x01]).credential_bytes());
    assert_eq!(keys.load(&[credential]), 0);
    keys
}

async fn get(client: Client<'_>, server: SocketAddr, path: &str) -> (Code, StdVec<u8>) {
    let reply = client.client(server).peer(DEVICE).get(path).await.unwrap();
    let message = reply.message().unwrap();
    (message.code(), message.payload().to_vec())
}

async fn send(
    client: Client<'_>,
    server: SocketAddr,
    method: Code,
    path: &str,
    body: &[u8],
) -> (Code, StdVec<u8>) {
    let reply = client
        .client(server)
        .peer(DEVICE)
        .request(method, path)
        .unwrap()
        .payload(body)
        .unwrap()
        .send()
        .unwrap()
        .await
        .unwrap();
    let message = reply.message().unwrap();
    (message.code(), message.payload().to_vec())
}

async fn put(client: Client<'_>, server: SocketAddr, path: &str, body: &[u8]) -> Code {
    send(client, server, Code::PUT, path, body).await.0
}

#[test]
fn resources_read_and_write_through_an_authenticated_session() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let (code, body) = get(client, server, "/light").await;
        assert_eq!(code, Code::CONTENT);
        assert!(body.starts_with(br#"{"state":"OFF","brightness":8"#));

        let on = br#"{"state":"ON","brightness":99,"effect":"Pacifica"}"#;
        assert_eq!(put(client, server, "/light", on).await, Code::CHANGED);
        let (_, body) = get(client, server, "/light").await;
        assert!(body.starts_with(br#"{"state":"ON","brightness":99"#));

        assert_eq!(put(client, server, "/light", b"{").await, Code::BAD_REQUEST);

        assert_eq!(put(client, server, "/speed", b"20").await, Code::CHANGED);
        assert_eq!(get(client, server, "/speed").await.1, b"20");
        assert_eq!(put(client, server, "/speed", b"0").await, Code::BAD_REQUEST);

        assert_eq!(get(client, server, "/temperature").await.1, b"21.2");
        assert_eq!(
            get(client, server, "/firmware").await.1,
            br#"{"state":"ready"}"#
        );

        let info = get(client, server, "/info").await.1;
        let info = core::str::from_utf8(&info).unwrap();
        assert!(info.contains(r#""name":"esp32-led-000102""#), "{info}");
        assert!(info.ends_with(r#","ip":"192.0.2.1"}"#), "{info}");

        assert_eq!(get(client, server, "/nope").await.0, Code::NOT_FOUND);
        assert_eq!(
            put(client, server, "/temperature", b"1").await,
            Code::METHOD_NOT_ALLOWED
        );
    });
    assert!(device.authenticated.get());
    assert!(device.light().on);
    assert_eq!(device.light().speed, 20);
    assert_eq!(device.light().effect, Some(EffectId::Pacifica));
}

#[test]
fn effect_list_spans_blocks_and_stays_valid_json() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let mut body = [0; 2048];
        // 64-byte blocks, to exercise the block boundaries.
        let len = client
            .client(server)
            .peer(DEVICE)
            .get_blocks("/effects", 2, &mut body)
            .await
            .unwrap();
        let text = core::str::from_utf8(&body[..len]).unwrap();
        assert!(text.starts_with(r#"["None","Rainbow","#), "{text}");
        assert!(text.ends_with(r#""Finland Kaamos"]"#), "{text}");
        assert_eq!(text.matches(',').count(), EFFECTS.len());
    });
}

#[test]
fn observers_are_told_about_changes() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session_with_notifier(
        &device,
        CLIENT_KEY,
        &keys,
        async |client, server, notifier| {
            let mut observation = client
                .client(server)
                .peer(DEVICE)
                .observe("/speed")
                .unwrap();
            let first = observation.next().await.unwrap().unwrap();
            assert_eq!(first.message().unwrap().payload(), b"64");

            assert_eq!(put(client, server, "/speed", b"20").await, Code::CHANGED);
            notifier.notify(resource::SPEED).unwrap();
            let second = observation.next().await.unwrap().unwrap();
            assert_eq!(second.message().unwrap().payload(), b"20");
            observation.cancel();
        },
    );
}

/// Splits a `/logs` reply into its offsets and text.
fn parse_logs(body: &[u8]) -> (u64, u64, &[u8]) {
    let split = body.iter().position(|b| *b == b'\n').unwrap();
    let (start, next) = core::str::from_utf8(&body[..split])
        .unwrap()
        .split_once(' ')
        .unwrap();
    (
        start.parse().unwrap(),
        next.parse().unwrap(),
        &body[split + 1..],
    )
}

#[test]
fn logs_are_read_with_a_cursor_and_nothing_is_missed() {
    let device = FakeDevice::new();
    let text: StdVec<u8> = (0..3000u32).map(|i| b'a' + (i % 26) as u8).collect();
    device.logs.borrow_mut().push(&text);
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let mut collected = StdVec::new();
        let mut cursor = 0;
        for _ in 0..10 {
            let path = format!("/logs?from={cursor}");
            let (code, body) = get(client, server, &path).await;
            assert_eq!(code, Code::CONTENT);
            let (start, next, chunk) = parse_logs(&body);
            assert_eq!(start, cursor);
            assert_eq!(next - start, chunk.len() as u64);
            if chunk.is_empty() {
                break;
            }
            collected.extend_from_slice(chunk);
            cursor = next;
        }
        assert_eq!(collected, text);

        // Without a cursor, the newest text.
        let (_, body) = get(client, server, "/logs").await;
        let (start, next, chunk) = parse_logs(&body);
        assert_eq!(next, 3000);
        assert!(start > 0);
        assert!(text.ends_with(chunk));

        assert_eq!(
            get(client, server, "/logs?from=x").await.0,
            Code::BAD_REQUEST
        );
    });
}

#[test]
fn log_tailing_sees_new_text_and_any_gap() {
    let device = FakeDevice::new();
    device.logs.borrow_mut().push(b"one\n");
    let keys = trusted_keys();
    session_with_notifier(
        &device,
        CLIENT_KEY,
        &keys,
        async |client, server, notifier| {
            let mut observation = client.client(server).peer(DEVICE).observe("/logs").unwrap();
            let first = observation.next().await.unwrap().unwrap();
            let (start, next, chunk) = parse_logs(first.message().unwrap().payload());
            assert_eq!((start, next, chunk), (0, 4, &b"one\n"[..]));

            device.logs.borrow_mut().push(b"two\n");
            notifier.notify(resource::LOGS).unwrap();
            let second = observation.next().await.unwrap().unwrap();
            let (_, next, chunk) = parse_logs(second.message().unwrap().payload());
            assert_eq!(next, 8);
            assert!(chunk.ends_with(b"two\n"));
            observation.cancel();
        },
    );
}

#[test]
fn config_reads_without_secrets_and_escapes_text() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let (code, body) = get(client, server, "/config").await;
        assert_eq!(code, Code::CONTENT);
        let text = core::str::from_utf8(&body).unwrap();
        for secret in ["wifi-secret", "mqtt-secret", DEVICE_KEY] {
            assert!(!text.contains(secret), "{text}");
        }
        assert!(text.contains(r#""has_wifi_password":true"#), "{text}");
        assert!(text.contains(r#""has_mqtt_password":true"#), "{text}");
        assert!(text.contains(r#""mqtt_broker":"mqtt.example""#), "{text}");
        assert!(text.contains(r#""device_credential":"abcd""#), "{text}");
        assert!(text.contains(r#""wifi_ssid":"he said \"hi\"""#), "{text}");
    });
}

#[test]
fn config_writes_are_validated_and_queued() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        assert_eq!(
            put(client, server, "/config", b"{").await,
            Code::BAD_REQUEST
        );
        let short_password = br#"{"wifi_ssid":"n","wifi_password":"short"}"#;
        assert_eq!(
            put(client, server, "/config", short_password).await,
            Code::BAD_REQUEST
        );
        let with_key = format!(
            r#"{{"wifi_ssid":"n","wifi_password":"password1","coap_device_key":"{}"}}"#,
            "ab".repeat(32)
        );
        let (code, body) = send(client, server, Code::PUT, "/config", with_key.as_bytes()).await;
        assert_eq!(
            (code, body.as_slice()),
            (Code::BAD_REQUEST, &b"coap_fields_not_allowed"[..])
        );
        let with_clients =
            br#"{"wifi_ssid":"n","wifi_password":"password1","coap_clients":["a108"]}"#;
        assert_eq!(
            put(client, server, "/config", with_clients).await,
            Code::BAD_REQUEST
        );
        assert!(device.written_config.borrow().is_none());

        let good = br#"{"wifi_ssid":"new","wifi_password":"password1"}"#;
        device.busy.set(true);
        let (code, body) = send(client, server, Code::PUT, "/config", good).await;
        assert_eq!((code, body.as_slice()), (Code::UNAVAILABLE, &b"busy"[..]));
        device.busy.set(false);
        assert_eq!(put(client, server, "/config", good).await, Code::CHANGED);
    });
    assert_eq!(
        device.written_config.borrow().as_ref().unwrap().wifi_ssid,
        "new"
    );
}

#[test]
fn large_settings_arrive_in_blocks() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        // Whitespace padding makes it several 64-byte blocks without changing its meaning.
        let padded = format!(
            "{}{}",
            r#"{"wifi_ssid":"blocks","wifi_password":"password1"}"#,
            " ".repeat(400)
        );
        let reply = client
            .client(server)
            .peer(DEVICE)
            .upload(
                Code::PUT,
                "/config",
                ContentFormat::OCTET_STREAM,
                padded.as_bytes(),
                2,
            )
            .await
            .unwrap();
        assert_eq!(reply.message().unwrap().code(), Code::CHANGED);
    });
    assert_eq!(
        device.written_config.borrow().as_ref().unwrap().wifi_ssid,
        "blocks"
    );
}

fn stored_hex(identity_key: [u8; 32], kid: &[u8]) -> device_config::CoapCredential {
    hex(client_identity(identity_key, kid).credential_bytes())
}

#[test]
fn client_keys_are_added_and_removed_through_the_flash_job() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let first = stored_hex(CLIENT_KEY, &[0x01]);
    let second = stored_hex(OTHER_KEY, &[0x02]);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let clients =
            async |method, path: &str, body: &[u8]| send(client, server, method, path, body).await;
        assert_eq!(
            clients(Code::POST, "/coap/clients", b"zz").await,
            (Code::BAD_REQUEST, b"invalid_key".to_vec())
        );

        // An existing key adds nothing, and the store only changes once the job has saved.
        let (code, _) = clients(Code::POST, "/coap/clients", first.as_bytes()).await;
        assert_eq!(code, Code::CHANGED);
        assert_eq!(device.saved_clients.borrow().as_ref().unwrap().len(), 1);

        let colliding = stored_hex(OTHER_KEY, &[0x01]);
        assert_eq!(
            clients(Code::POST, "/coap/clients", colliding.as_bytes()).await,
            (Code::BAD_REQUEST, b"duplicate_key_id".to_vec())
        );

        let (code, _) = clients(Code::POST, "/coap/clients", second.as_bytes()).await;
        assert_eq!(code, Code::CHANGED);
        let saved = device.saved_clients.borrow().clone().unwrap();
        assert_eq!(saved.as_slice(), [first.clone(), second.clone()]);
        assert_eq!(keys.load(&saved), 0);

        let (code, list) = get(client, server, "/coap/clients").await;
        assert_eq!(code, Code::CONTENT);
        let list = std::string::String::from_utf8(list).unwrap();
        let fingerprint = |hex: &str| {
            let mut ccs = [0; 128];
            coap_keys::fingerprint(coap_keys::read_hex(hex, &mut ccs).unwrap()).unwrap()
        };
        assert_eq!(
            list,
            format!(r#"["{}","{}"]"#, fingerprint(&first), fingerprint(&second))
        );

        assert_eq!(
            clients(Code::DELETE, "/coap/clients?fp=00000000", b"").await,
            (Code::NOT_FOUND, b"unknown_key".to_vec())
        );
        assert_eq!(
            clients(Code::DELETE, "/coap/clients", b"").await.0,
            Code::BAD_REQUEST
        );
        let path = format!("/coap/clients?fp={}", fingerprint(&second));
        assert_eq!(clients(Code::DELETE, &path, b"").await.0, Code::DELETED);
        let saved = device.saved_clients.borrow().clone().unwrap();
        assert_eq!(saved.as_slice(), core::slice::from_ref(&first));
        assert_eq!(keys.load(&saved), 0);

        let path = format!("/coap/clients?fp={}", fingerprint(&first));
        assert_eq!(
            clients(Code::DELETE, &path, b"").await,
            (Code::BAD_REQUEST, b"last_key".to_vec())
        );

        device.busy.set(true);
        assert_eq!(
            clients(Code::POST, "/coap/clients", second.as_bytes()).await,
            (Code::UNAVAILABLE, b"busy".to_vec())
        );
        device.busy.set(false);
    });
}

#[test]
fn the_client_list_has_a_limit() {
    let device = FakeDevice::new();
    let keys = ClientKeys::new();
    let full: StdVec<_> = (0..4u8)
        .map(|n| stored_hex([20 + n; 32], &[n + 1]))
        .collect();
    assert_eq!(keys.load(&full), 0);
    let _ = device;
    assert_eq!(
        keys.with_added(&stored_hex(OTHER_KEY, &[9])).err(),
        Some("too_many_keys")
    );
}

#[test]
fn ota_trigger_passes_the_command_and_explains_refusals() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let path = "/firmware/update";
        let (code, _) = send(client, server, Code::POST, path, b"{}").await;
        assert_eq!(code, Code::CHANGED);
        for (reason, expected) in [
            ("busy", Code::UNAVAILABLE),
            ("unavailable", Code::UNAVAILABLE),
            ("invalid_command", Code::BAD_REQUEST),
            ("already_running", Code::BAD_REQUEST),
        ] {
            device.ota_result.set(Err(reason));
            let (code, body) = send(client, server, Code::POST, path, b"{}").await;
            assert_eq!((code, body.as_slice()), (expected, reason.as_bytes()));
        }
    });
    assert_eq!(device.ota_commands.borrow()[0], b"{}");
}

#[test]
fn reboot_is_queued_unless_flash_is_busy() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let (code, _) = send(client, server, Code::POST, "/reboot", b"").await;
        assert_eq!(code, Code::CHANGED);
        device.busy.set(true);
        let (code, _) = send(client, server, Code::POST, "/reboot", b"").await;
        assert_eq!(code, Code::UNAVAILABLE);
    });
    assert_eq!(device.reboots.get(), 1);
}

const BUILD: &str = "test-1";

fn test_image(size: usize) -> StdVec<u8> {
    let mut image: StdVec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    image[..IMAGE_PREFIX_SIZE].fill(0);
    image[0] = 0xe9;
    image[1] = 3;
    image[12] = 13;
    image[28..32].copy_from_slice(&256_u32.to_le_bytes());
    image[32..36].copy_from_slice(&0xabcd5432_u32.to_le_bytes());
    image[48..48 + BUILD.len()].copy_from_slice(BUILD.as_bytes());
    image[80..80 + PROJECT.len()].copy_from_slice(PROJECT.as_bytes());
    image
}

fn manifest_json(image: &[u8]) -> std::string::String {
    let digest: [u8; 32] = sha2::Sha256::digest(image).into();
    format!(
        r#"{{"size":{},"crc32":7,"sha256":"{}","target":"esp32c6","build":"{BUILD}"}}"#,
        image.len(),
        hex(&digest)
    )
}

async fn upload_image(
    client: Client<'_>,
    server: SocketAddr,
    image: &[u8],
) -> Result<Code, mai_coap::Error> {
    let reply = client
        .client(server)
        .peer(DEVICE)
        .upload(
            Code::PUT,
            "/firmware/image",
            ContentFormat::OCTET_STREAM,
            image,
            6,
        )
        .await?;
    Ok(reply.message()?.code())
}

#[test]
fn firmware_upload_writes_checks_and_activates_the_image() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(10_000);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let manifest = manifest_json(&image);
        assert_eq!(
            put(client, server, "/firmware/manifest", manifest.as_bytes()).await,
            Code::CHANGED
        );
        assert_eq!(
            upload_image(client, server, &image).await,
            Ok(Code::CHANGED)
        );
    });
    let flash = &device.flash;
    assert_eq!(*flash.bytes.borrow(), image);
    assert_eq!(flash.begun.get(), 1);
    assert!(flash.activated.get());
    assert_eq!(*flash.finished.borrow(), [true]);
    assert!(!flash.held_by_upload.get());
    assert!(flash.progress.get() >= 10);
    let statuses = flash.statuses.borrow();
    assert_eq!(statuses.first().unwrap().0, "uploading");
    assert_eq!(statuses.last().unwrap().0, "rebooting");
}

#[test]
fn firmware_upload_needs_a_manifest_for_each_image() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(5000);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let refused = upload_image(client, server, &image).await;
        assert_eq!(
            refused,
            Err(mai_coap::Error::Response(Code::PRECONDITION_FAILED))
        );
        assert!(!device.flash.held_by_upload.get());
    });
    assert_eq!(device.flash.begun.get(), 0);
    assert!(!device.flash.activated.get());
}

#[test]
fn firmware_with_the_wrong_digest_is_never_activated_and_frees_the_flash() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(10_000);
    let mut corrupted = image.clone();
    *corrupted.last_mut().unwrap() ^= 1;
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let manifest = manifest_json(&image);
        put(client, server, "/firmware/manifest", manifest.as_bytes()).await;
        assert_eq!(
            upload_image(client, server, &corrupted).await,
            Err(mai_coap::Error::Response(Code::BAD_REQUEST))
        );
        assert!(!device.flash.activated.get());
        assert!(!device.flash.held_by_upload.get());
        assert_eq!(*device.flash.finished.borrow(), [false]);
        assert_eq!(
            device.flash.statuses.borrow().last().unwrap(),
            &("failed".into(), "sha256".into())
        );

        // The flash is free, so a good retry works.
        put(client, server, "/firmware/manifest", manifest.as_bytes()).await;
        assert_eq!(
            upload_image(client, server, &image).await,
            Ok(Code::CHANGED)
        );
    });
    assert!(device.flash.activated.get());
    assert_eq!(*device.flash.bytes.borrow(), image);
}

#[test]
fn firmware_with_the_wrong_identity_never_reaches_flash() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let mut image = test_image(5000);
    image[0] = 0;
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        put(
            client,
            server,
            "/firmware/manifest",
            manifest_json(&image).as_bytes(),
        )
        .await;
        assert_eq!(
            upload_image(client, server, &image).await,
            Err(mai_coap::Error::Response(Code::BAD_REQUEST))
        );
    });
    assert_eq!(device.flash.begun.get(), 0);
    assert!(device.flash.bytes.borrow().is_empty());
    assert_eq!(*device.flash.finished.borrow(), [false]);
}

#[test]
fn firmware_upload_waits_for_busy_flash_and_keeps_its_manifest() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(5000);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        put(
            client,
            server,
            "/firmware/manifest",
            manifest_json(&image).as_bytes(),
        )
        .await;
        device.flash.held_by_other.set(true);
        assert_eq!(
            upload_image(client, server, &image).await,
            Err(mai_coap::Error::Response(Code::UNAVAILABLE))
        );
        device.flash.held_by_other.set(false);
        assert_eq!(
            upload_image(client, server, &image).await,
            Ok(Code::CHANGED)
        );
    });
    assert!(device.flash.activated.get());
}

#[test]
fn a_retransmitted_block_is_stored_once() {
    use mai_coap::block::Block;

    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(5000);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        put(
            client,
            server,
            "/firmware/manifest",
            manifest_json(&image).as_bytes(),
        )
        .await;
        let block = Block::new(0, true, 6).unwrap().encode().unwrap();
        for _ in 0..2 {
            let reply = client
                .client(server)
                .peer(DEVICE)
                .request(Code::PUT, "/firmware/image")
                .unwrap()
                .uint_option(OptionNumber::BLOCK1, block)
                .unwrap()
                .payload(&image[..1024])
                .unwrap()
                .send()
                .unwrap()
                .await
                .unwrap();
            assert_eq!(reply.message().unwrap().code(), Code::CONTINUE);
        }
    });
    assert_eq!(
        device.flash.begun.get(),
        0,
        "nothing flushed before 4096 bytes"
    );
    assert_eq!(device.flash.progress.get(), 1);
}

#[test]
fn an_abandoned_upload_frees_the_flash_without_activating() {
    let flash = FakeFlash::default();
    let manifest = PendingManifest::new();
    let image = test_image(5000);
    manifest.set(Some(
        Manifest::parse(manifest_json(&image).as_bytes()).unwrap(),
    ));
    let mut sink = FirmwareUpload {
        access: FakeAccess(flash.clone()),
        manifest: &manifest,
        upload: None,
        failure_reported: false,
    };
    let key = BodyKey::from_request(
        "192.0.2.1:5683".parse().unwrap(),
        0,
        None,
        mai_coap::codec::MessageView::parse(&[0x40, 0x03, 0, 1]).unwrap(),
    )
    .unwrap();
    sink.begin(&key, Some(image.len() as u64)).unwrap();
    assert!(flash.held_by_upload.get());
    sink.write(0, &image[..1024]).unwrap();

    sink.abort();
    assert!(!flash.held_by_upload.get());
    assert!(!flash.activated.get());
    assert_eq!(*flash.finished.borrow(), [false]);
    assert_eq!(
        flash.statuses.borrow().last().unwrap(),
        &("failed".into(), "abandoned".into())
    );

    // A second abort, or one with nothing running, has nothing to report.
    let statuses = flash.statuses.borrow().len();
    sink.abort();
    assert_eq!(*flash.finished.borrow(), [false]);
    assert_eq!(flash.statuses.borrow().len(), statuses);
}

#[test]
fn manifests_are_checked_up_front() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    let image = test_image(5000);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let path = "/firmware/manifest";
        let manifest = manifest_json(&image);
        assert_eq!(
            send(client, server, Code::PUT, path, b"{").await,
            (Code::BAD_REQUEST, b"invalid_command".to_vec())
        );
        let running = manifest.replace(BUILD, env!("FIRMWARE_BUILD_ID"));
        assert_eq!(
            send(client, server, Code::PUT, path, running.as_bytes()).await,
            (Code::BAD_REQUEST, b"already_running".to_vec())
        );
        device.upload_enabled.set(false);
        assert_eq!(
            send(client, server, Code::PUT, path, manifest.as_bytes()).await,
            (Code::UNAVAILABLE, b"unavailable".to_vec())
        );
    });
}

/// Long-lived observations keep their security contexts, which must not stop a client from
/// connecting: otherwise a couple of forgotten `logs -f` lock everyone out of the device.
#[test]
fn observers_leave_room_for_another_client() {
    let device = FakeDevice::new();
    let keys = ClientKeys::new();
    let clients: [([u8; 32], u8); device_config::MAX_COAP_CLIENTS] =
        core::array::from_fn(|n| ([20 + n as u8; 32], n as u8 + 1));
    let credentials: StdVec<_> = clients
        .iter()
        .map(|(key, kid)| hex(client_identity(*key, &[*kid]).credential_bytes()))
        .collect();
    assert_eq!(keys.load(&credentials), 0);

    block_on(async {
        let mut server_socket = StdTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let server = server_socket.local_addr().unwrap();
        let clock = StdClock::default();
        let mut device_crypto = crypto(2);
        let device_identity = coap_keys::identity(DEVICE_KEY, &mut device_crypto).unwrap();
        let device_credential = device_identity.credential_bytes().to_vec();
        let protection = Edhoc::with_store(device_crypto, device_identity, &keys).unwrap();
        let shared = ApiShared::new();
        let mut endpoint = ApiEndpoint::new(&shared, protection).unwrap();
        let manifest = PendingManifest::new();
        let mut resources =
            Resources::new(&device, &keys, &manifest, FakeAccess(device.flash.clone()));
        let mut router = ApiRouter::new(&POLICY);
        resources.register(&mut router).unwrap();
        let mut application = Authenticated {
            device: &device,
            router,
        };
        let server_driver = endpoint.run(&mut server_socket, &clock, &mut application, &POLICY);

        // As many clients as can be trusted: three holding sessions, and one connecting after.
        macro_rules! client {
            ($index:expr, $shared:ident, $endpoint:ident, $socket:ident) => {
                let $shared = DefaultShared::new();
                let (key, kid) = clients[$index];
                let mut protection = Edhoc::<_, 4, 4, TrustedPeers<1>, 0>::with_store(
                    crypto(10 + $index as u64),
                    client_identity(key, &[kid]),
                    TrustedPeers::new(),
                )
                .unwrap();
                protection
                    .trust(TrustedPeer::new(DEVICE, &device_credential).unwrap())
                    .unwrap();
                let mut $endpoint = DefaultEndpoint::new(&$shared, protection).unwrap();
                let mut $socket = StdTransport::bind("127.0.0.1:0".parse().unwrap()).unwrap();
            };
        }
        client!(0, shared_a, endpoint_a, socket_a);
        client!(1, shared_b, endpoint_b, socket_b);
        client!(2, shared_c, endpoint_c, socket_c);
        client!(3, shared_d, endpoint_d, socket_d);
        let reject =
            |_: Request<'_>, _: &mut ResponseWriter<'_, DATAGRAM_BYTES>| Err(Code::NOT_FOUND);
        let (mut no_a, mut no_b, mut no_c, mut no_d) = (reject, reject, reject, reject);
        let drivers = futures::future::join5(
            server_driver,
            endpoint_a.run(&mut socket_a, &clock, &mut no_a, &ProtectedOnly),
            endpoint_b.run(&mut socket_b, &clock, &mut no_b, &ProtectedOnly),
            endpoint_c.run(&mut socket_c, &clock, &mut no_c, &ProtectedOnly),
            endpoint_d.run(&mut socket_d, &clock, &mut no_d, &ProtectedOnly),
        );

        let test = async {
            let mut a = shared_a
                .handle()
                .client(server)
                .peer(DEVICE)
                .observe("/speed")
                .unwrap();
            a.next().await.unwrap().unwrap();
            let mut b = shared_b
                .handle()
                .client(server)
                .peer(DEVICE)
                .observe("/light")
                .unwrap();
            b.next().await.unwrap().unwrap();
            let mut c = shared_c
                .handle()
                .client(server)
                .peer(DEVICE)
                .observe("/logs")
                .unwrap();
            c.next().await.unwrap().unwrap();
            let reply = shared_d
                .handle()
                .client(server)
                .peer(DEVICE)
                .get("/speed")
                .await
                .expect("another client can still connect");
            assert_eq!(reply.message().unwrap().code(), Code::CONTENT);
            a.cancel();
            b.cancel();
            c.cancel();
        };
        pin_mut!(drivers, test);
        match select(test, drivers).await {
            Either::Left(_) => {}
            Either::Right(_) => panic!("a driver stopped"),
        }
    });
}

#[test]
fn an_unknown_client_key_gets_nothing() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, OTHER_KEY, &keys, async |client, server| {
        let result = client.client(server).peer(DEVICE).get("/light").await;
        assert!(result.is_err());
    });
    assert!(!device.authenticated.get());
}

#[test]
fn without_authentication_only_discovery_answers() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        let unprotected = client.client(server);
        for path in [
            "/light",
            "/speed",
            "/info",
            "/firmware",
            "/effects",
            "/nope",
        ] {
            let reply = unprotected.get(path).await.unwrap();
            assert_eq!(
                reply.message().unwrap().code(),
                Code::UNAUTHORIZED,
                "{path}"
            );
        }
        let reply = unprotected
            .request(Code::PUT, "/light")
            .unwrap()
            .payload(br#"{"state":"ON"}"#)
            .unwrap()
            .send()
            .unwrap()
            .await
            .unwrap();
        assert_eq!(reply.message().unwrap().code(), Code::UNAUTHORIZED);
        for (method, path) in [
            (Code::PUT, "/speed"),
            (Code::PUT, "/config"),
            (Code::POST, "/coap/clients"),
            (Code::DELETE, "/coap/clients"),
            (Code::POST, "/firmware/update"),
            (Code::POST, "/reboot"),
            (Code::PUT, "/firmware/manifest"),
            (Code::PUT, "/firmware/image"),
        ] {
            let reply = unprotected
                .request(method, path)
                .unwrap()
                .send()
                .unwrap()
                .await
                .unwrap();
            assert_eq!(
                reply.message().unwrap().code(),
                Code::UNAUTHORIZED,
                "{path}"
            );
        }

        // Discovery lists only what the caller could access: nothing.
        let reply = unprotected.get("/.well-known/core").await.unwrap();
        assert_eq!(reply.message().unwrap().code(), Code::CONTENT);
        assert_eq!(reply.message().unwrap().payload(), b"");
    });
    assert!(!device.light().on);
    assert!(!device.authenticated.get());
    assert!(device.written_config.borrow().is_none());
    assert!(device.saved_clients.borrow().is_none());
    assert!(device.ota_commands.borrow().is_empty());
    assert_eq!(device.reboots.get(), 0);
    assert_eq!(device.flash.begun.get(), 0);
}

#[test]
fn removed_client_keys_stop_new_handshakes() {
    let device = FakeDevice::new();
    let keys = trusted_keys();
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        assert_eq!(get(client, server, "/speed").await.0, Code::CONTENT);
    });
    keys.load(&[]);
    session(&device, CLIENT_KEY, &keys, async |client, server| {
        assert!(
            client
                .client(server)
                .peer(DEVICE)
                .get("/speed")
                .await
                .is_err()
        );
    });
}

#[test]
fn unusable_client_keys_are_skipped() {
    let keys = ClientKeys::new();
    let good = hex(client_identity(CLIENT_KEY, &[0x01]).credential_bytes());
    assert_eq!(keys.load(&["00ff".try_into().unwrap(), good]), 1);
    assert!(keys.get(PeerId(2)).is_some());
    assert!(keys.get(PeerId(1)).is_none());
}
