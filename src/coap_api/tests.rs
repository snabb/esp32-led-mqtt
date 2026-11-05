//! A real server and client talking EDHOC and OSCORE over loopback UDP.

use core::{cell::Cell, convert::Infallible, net::SocketAddr};
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
use crate::{EffectId, coap_keys};

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
}

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
        let mut resources = Resources::new(device);
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

async fn put(client: Client<'_>, server: SocketAddr, path: &str, body: &[u8]) -> Code {
    let reply = client
        .client(server)
        .peer(DEVICE)
        .request(Code::PUT, path)
        .unwrap()
        .payload(body)
        .unwrap()
        .send()
        .unwrap()
        .await
        .unwrap();
    reply.message().unwrap().code()
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

        // Discovery lists only what the caller could access: nothing.
        let reply = unprotected.get("/.well-known/core").await.unwrap();
        assert_eq!(reply.message().unwrap().code(), Code::CONTENT);
        assert_eq!(reply.message().unwrap().payload(), b"");
    });
    assert!(!device.light().on);
    assert!(!device.authenticated.get());
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
