//! A real server and client talking EDHOC and OSCORE over loopback UDP.

use core::{cell::Cell, convert::Infallible, net::SocketAddr};
use std::format;
use std::vec::Vec as StdVec;

use futures::{
    executor::block_on,
    future::{Either, select},
    pin_mut,
};
use mai_coap::{
    codec::Code,
    edhoc::{CcsEncoding, Identity, TrustedPeers},
    endpoint::{DefaultEndpoint, DefaultShared, Handle},
    security::ProtectedOnly,
    time::StdClock,
    transport::linux::StdTransport,
};
use rand_core::{TryCryptoRng, TryRng};
use smart_leds::RGB8;

use super::*;
use crate::{EffectId, log_buffer::LogRing};

const DEVICE_KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const CLIENT_KEY: [u8; 32] = [9; 32];
const OTHER_KEY: [u8; 32] = [10; 32];
const DEVICE: PeerId = PeerId(1);

/// Not for real use; tests only need the two sides to differ.
struct TestRng(u64);
impl TryRng for TestRng {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        Ok(self.try_next_u64()? as u32)
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        self.0 = self.0.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
        Ok(self.0 ^ (self.0 >> 29))
    }
    fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Infallible> {
        for chunk in output.chunks_mut(8) {
            let bytes = self.try_next_u64()?.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}
impl TryCryptoRng for TestRng {}

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

        let device_identity = coap_keys::identity(DEVICE_KEY, &mut crypto(1)).unwrap();
        let device_credential = device_identity.credential_bytes().to_vec();
        let protection = Edhoc::with_store(crypto(2), device_identity, keys).unwrap();
        let shared = ApiShared::new();
        let mut endpoint = ApiEndpoint::new(&shared, protection).unwrap();
        let mut resources = Resources::new(device, keys);
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
        assert_eq!(saved.as_slice(), [first.clone()]);
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
