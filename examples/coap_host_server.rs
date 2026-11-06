//! The firmware's CoAP resources on a host, to test `tools/coap.py` without a device.
//!
//! ```sh
//! uv run tools/coap.py keygen
//! cargo run --target x86_64-unknown-linux-gnu --example coap_host_server -- \
//!     127.0.0.1:5683 local/coap/host.ccs local/coap/client.ccs
//! uv run tools/coap.py --host 127.0.0.1 --device-ccs local/coap/host.ccs info
//! ```

use std::{
    cell::{Cell, RefCell},
    convert::Infallible,
    fs::File,
    io::Read,
    net::Ipv4Addr,
    rc::Rc,
};

use esp32_led_mqtt::{
    EffectId,
    coap_api::{
        ApiEndpoint, ApiRouter, ApiShared, Authenticated, Busy, ClientKeys, ClientList, Device,
        Info, POLICY, PendingManifest, Resources, UploadAccess, resource,
    },
    coap_keys,
    device_config::{CoapCredential, DeviceConfig},
    light_command::{LightState, LightUpdate},
    log_buffer::{LOG_BYTES, LogRead, LogRing},
    ota_protocol::{ImageTarget, TargetError},
};
use futures::{executor::block_on, future::select, pin_mut};
use heapless::String;
use mai_coap::{
    crypto::SoftwareCrypto, edhoc::endpoint::Edhoc, endpoint::Handle, time::StdClock,
    transport::linux::StdTransport,
};
use rand_core::{TryCryptoRng, TryRng};
use smart_leds::RGB8;

/// Fixed so that restarting the server keeps its credential. Never use it on a device.
const TEST_KEY: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const STATUS_SECONDS: u64 = 1;

struct OsRng(File);
impl TryRng for OsRng {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Infallible> {
        self.0.read_exact(output).expect("/dev/urandom");
        Ok(())
    }
}
impl TryCryptoRng for OsRng {}

fn os_rng() -> OsRng {
    OsRng(File::open("/dev/urandom").expect("/dev/urandom"))
}

#[derive(Default)]
struct Shared {
    status: RefCell<String<192>>,
    image: RefCell<Vec<u8>>,
    logs: RefCell<Option<LogRing<LOG_BYTES>>>,
    holding: Cell<bool>,
}

impl Shared {
    fn log(&self, line: &str) {
        println!("{line}");
        let mut logs = self.logs.borrow_mut();
        let logs = logs.get_or_insert_with(LogRing::new);
        logs.push(line.as_bytes());
        logs.push(b"\n");
    }
    fn set_status(&self, state: &str, detail: &str, percent: u8, build: &str) {
        *self.status.borrow_mut() = heapless::format!(
            r#"{{"state":"{state}","detail":"{detail}","progress":{percent},"running_build":"{build}"}}"#
        )
        .expect("bounded status");
    }
}

struct HostDevice {
    shared: Rc<Shared>,
    light: Cell<LightState>,
    config: DeviceConfig,
    keys: Rc<ClientKeys>,
}

impl Device for HostDevice {
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
        Some(21.5)
    }
    fn ota_status(&self) -> Option<String<192>> {
        Some(self.shared.status.borrow().clone())
    }
    fn info(&self) -> Info<'_> {
        Info {
            name: "esp32-led-host",
            uptime_s: 1,
            ip: Some(Ipv4Addr::LOCALHOST),
            free_heap: 0,
        }
    }
    fn read_logs(&self, from: Option<u64>, output: &mut [u8]) -> LogRead {
        let mut logs = self.shared.logs.borrow_mut();
        logs.get_or_insert_with(LogRing::new).read(from, output)
    }
    fn submit_ota(&self, command: &[u8]) -> Result<(), &'static str> {
        self.shared
            .log(&format!("HTTPS OTA command: {} bytes", command.len()));
        Ok(())
    }
    fn config(&self) -> &DeviceConfig {
        &self.config
    }
    fn device_credential(&self) -> CoapCredential {
        coap_keys::public_credential(TEST_KEY).expect("test key")
    }
    fn write_config(&self, config: DeviceConfig) -> Result<(), Busy> {
        self.shared
            .log(&format!("config write: ssid {}", config.wifi_ssid));
        Ok(())
    }
    fn save_clients(&self, clients: ClientList) -> Result<(), Busy> {
        self.shared.log(&format!("clients: {}", clients.len()));
        self.keys.load(&clients);
        Ok(())
    }
    fn reboot(&self) -> Result<(), Busy> {
        self.shared.log("reboot requested");
        Ok(())
    }
    fn upload_enabled(&self) -> bool {
        true
    }
    fn authenticated(&self) {}
}

struct HostTarget(Rc<Shared>);

impl Drop for HostTarget {
    fn drop(&mut self) {
        self.0.holding.set(false);
    }
}

impl ImageTarget for HostTarget {
    fn begin(&mut self, _: u32, _: u32) -> Result<(), TargetError> {
        self.0.image.borrow_mut().clear();
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), TargetError> {
        self.0.image.borrow_mut().extend_from_slice(bytes);
        Ok(())
    }
    fn activate(&mut self) -> Result<(), TargetError> {
        let crc = self.0.image.borrow().len();
        self.0.log(&format!("activated an image of {crc} bytes"));
        Ok(())
    }
}

struct HostAccess(Rc<Shared>);

impl UploadAccess for HostAccess {
    type Target = HostTarget;
    type Hash = sha2::Sha256;
    fn acquire(&mut self) -> Option<HostTarget> {
        if self.0.holding.replace(true) {
            return None;
        }
        Some(HostTarget(self.0.clone()))
    }
    fn status(&mut self, state: &str, detail: &str, percent: u8) {
        self.0.log(&format!("status {state} {detail} {percent}"));
        self.0
            .set_status(state, detail, percent, env!("FIRMWARE_BUILD_ID"));
    }
    fn progress(&mut self) {}
    fn finished(&mut self, activated: bool) {
        if activated {
            // Stands in for the reboot: report the build from the image's app descriptor.
            let image = self.0.image.borrow();
            let name = &image[48..80];
            let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
            let build = std::str::from_utf8(&name[..end]).unwrap_or("?");
            self.0.set_status("ready", "", 0, build);
        }
    }
}

async fn notify_logs(handle: Handle<'_, 1152, 0>) {
    loop {
        async_io::Timer::after(std::time::Duration::from_secs(STATUS_SECONDS)).await;
        let _ = handle.notify(resource::LOGS);
        let _ = handle.notify(resource::LIGHT);
        let _ = handle.notify(resource::SPEED);
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let [_, bind, device_ccs, client_ccs] = args.as_slice() else {
        panic!("usage: coap_host_server BIND DEVICE_CCS_OUT CLIENT_CCS");
    };
    std::fs::write(
        device_ccs,
        hex_decode(&coap_keys::public_credential(TEST_KEY).unwrap()),
    )
    .unwrap();
    let client = std::fs::read(client_ccs).unwrap();
    let client: String<256> = coap_keys::write_hex(&client).unwrap();
    let keys = Rc::new(ClientKeys::new());
    assert_eq!(keys.load(&[client]), 0, "unusable client credential");

    let shared = Rc::new(Shared::default());
    shared.set_status("ready", "", 0, env!("FIRMWARE_BUILD_ID"));
    let config = DeviceConfig::decode(
        br#"{"wifi_ssid":"host","wifi_password":"host-password","mqtt_broker":"mqtt.example"}"#,
    )
    .unwrap();
    let device = HostDevice {
        shared: shared.clone(),
        light: Cell::new(LightState {
            on: false,
            brightness: 8,
            effect: Some(EffectId::Rainbow),
            speed: 64,
            color: RGB8 { r: 1, g: 2, b: 3 },
        }),
        config,
        keys: keys.clone(),
    };

    block_on(async {
        let mut socket = StdTransport::bind(bind.parse().unwrap()).unwrap();
        let identity =
            coap_keys::identity(TEST_KEY, &mut SoftwareCrypto::new(os_rng())).expect("test key");
        let protection =
            Edhoc::with_store(SoftwareCrypto::new(os_rng()), identity, &*keys).unwrap();
        let api_shared = ApiShared::new();
        let mut endpoint = ApiEndpoint::new(&api_shared, protection).unwrap();
        let manifest = PendingManifest::new();
        let mut resources = Resources::new(&device, &keys, &manifest, HostAccess(shared.clone()));
        let mut router = ApiRouter::new(&POLICY);
        resources.register(&mut router).unwrap();
        let mut application = Authenticated {
            device: &device,
            router,
        };
        let clock = StdClock::default();
        println!("LISTEN {bind}");
        let server = endpoint.run(&mut socket, &clock, &mut application, &POLICY);
        let notifier = notify_logs(api_shared.handle());
        pin_mut!(server, notifier);
        let _ = select(server, notifier).await;
    });
}

fn hex_decode(hex: &str) -> Vec<u8> {
    let mut output = vec![0; hex.len() / 2];
    coap_keys::read_hex(hex, &mut output).unwrap();
    output
}
