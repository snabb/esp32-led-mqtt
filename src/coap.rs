//! The CoAP server: binds `esp32_led_mqtt::coap_api` to the hardware and the network.

use core::{mem::ManuallyDrop, sync::atomic::Ordering};

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_net::{
    Stack,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Instant, Timer};
use esp_hal::{ecc::EllipticCurve, rng::Trng, sha::Sha256Context};
use esp_hal_ota::Ota;
use esp_storage::FlashStorage;
use esp32_led_mqtt::{
    coap_api::{
        ApiEndpoint, ApiRouter, ApiShared, Authenticated, Busy, ClientKeys, ClientList,
        DATAGRAM_BYTES, Device, Info, POLICY, PORT, PendingManifest, Resources, UploadAccess,
        resource,
    },
    coap_crypto::{AcceleratedCrypto, Accelerator},
    coap_keys,
    device_config::{CoapCredential, DeviceConfig},
    light_command::{LightState, LightUpdate},
    log_buffer::LogRead,
    provisioning::Command,
};
use heapless::String;
use log::{error, info, warn};
use mai_coap::{
    Error, crypto::Sha256State, edhoc::endpoint::Edhoc, endpoint::RunError, time::EmbassyClock,
    transport::embassy::EmbassyTransport,
};
use zeroize::Zeroizing;

use crate::{
    flash::{self, Job, Lease, UploadEvent},
    identity::DeviceIdentity,
    light_state, log_sink,
    ota::{self, FlashAccess, UpdateSlot},
    tsens,
};

const SOCKET_RX_SLOTS: usize = 4;
const SOCKET_TX_SLOTS: usize = 2;

static CLIENT_KEYS: ClientKeys = ClientKeys::new();

/// SHA-256 on the accelerator. Holds its state in place rather than through `Digest`, whose
/// consuming `finalize` would leave a copy of the buffered input to erase.
struct HardwareSha256(ManuallyDrop<Sha256Context>);

impl Drop for HardwareSha256 {
    fn drop(&mut self) {
        // SAFETY: the context is valid for writes and has no destructor, which `ManuallyDrop`
        // keeps from running on the zeroed value, and nothing can use it after this. Its
        // handles borrowed it mutably and have all ended.
        unsafe { zeroize::zeroize_flat_type::<Sha256Context>(&mut *self.0) };
    }
}

impl Sha256State for HardwareSha256 {
    fn update(&mut self, input: &[u8]) -> Result<(), Error> {
        self.0.update(input).wait_blocking();
        Ok(())
    }
    fn finalize(mut self) -> Result<[u8; 32], Error> {
        let mut digest = [0; 32];
        self.0.finalize(&mut digest).wait_blocking();
        Ok(digest)
    }
}

/// The SHA and ECC work queues. `main` starts them in every build with CoAP, because waiting
/// for an operation with no queue running would hang.
struct Hardware;

impl Accelerator for Hardware {
    type Sha256 = HardwareSha256;

    fn sha256_start(&mut self) -> Result<HardwareSha256, Error> {
        Ok(HardwareSha256(ManuallyDrop::new(Sha256Context::new())))
    }

    fn multiply(
        &mut self,
        k: &[u8; 32],
        x: &[u8; 32],
        y: &[u8; 32],
    ) -> Result<([u8; 32], [u8; 32]), Error> {
        // The peripheral is little-endian, unlike the provider interface.
        let reversed = |value: &[u8; 32]| {
            let mut output = *value;
            output.reverse();
            output
        };
        let scalar = Zeroizing::new(reversed(k));
        let (px, py) = (reversed(x), reversed(y));
        let (mut result_x, mut result_y) = ([0; 32], [0; 32]);
        let on_curve = {
            let mut operation = EllipticCurve::P256
                .affine_point_verification_multiplication(&scalar[..], &px, &py)
                .and_then(|operation| {
                    operation.with_affine_point_result(&mut result_x, &mut result_y)
                })
                .map_err(|_| Error::Protection)?;
            operation.process().wait_blocking();
            operation.point_on_curve()
        };
        if !on_curve {
            return Err(Error::Protection);
        }
        result_x.reverse();
        result_y.reverse();
        Ok((result_x, result_y))
    }
}

struct HardwareDevice {
    stack: Stack<'static>,
    name: &'static str,
    settings: &'static DeviceConfig,
}

/// Claims the flash owner's single job slot for `job`.
fn queue(job: impl FnOnce() -> Job) -> Result<(), Busy> {
    if !flash::try_claim() {
        return Err(Busy);
    }
    flash::queue(job());
    Ok(())
}

/// Hands the flash to firmware uploads.
struct FlashUpload;

impl UploadAccess for FlashUpload {
    type Target = UpdateSlot<Lease>;
    // `main` starts its work queue in every build with CoAP.
    type Hash = Sha256Context;

    fn acquire(&mut self) -> Option<Self::Target> {
        let lease = flash::try_lease()?;
        match Ota::new(FlashAccess(lease)) {
            Ok(ota) => Some(UpdateSlot(ota)),
            Err(_) => {
                warn!("Could not read the partition table for an upload");
                flash::UPLOAD_EVENT.signal(UploadEvent::Finished { reboot: false });
                None
            }
        }
    }
    fn status(&mut self, state: &str, detail: &str, percent: u8) {
        ota::status(state, detail, percent);
    }
    fn progress(&mut self) {
        flash::UPLOAD_EVENT.signal(UploadEvent::Progress);
    }
    fn finished(&mut self, activated: bool) {
        flash::UPLOAD_EVENT.signal(UploadEvent::Finished { reboot: activated });
    }
}

/// Saves the client list and, if that worked, applies it to the next handshake.
pub async fn save_clients(flash: &mut FlashStorage<'static>, clients: ClientList) {
    if crate::config::save_clients(flash, &clients).await {
        CLIENT_KEYS.load(&clients);
    } else {
        warn!("Could not save the CoAP client keys");
    }
}

impl Device for HardwareDevice {
    fn light(&self) -> LightState {
        light_state::get()
    }
    fn update_light(&self, update: &LightUpdate) {
        light_state::update(|state| update.apply(state));
    }
    fn set_speed(&self, speed: u8) {
        light_state::update(|state| state.speed = speed);
    }
    fn temperature(&self) -> Option<f32> {
        tsens::TEMPERATURE.try_get()
    }
    fn ota_status(&self) -> Option<String<192>> {
        ota::STATUS.try_get()
    }
    fn info(&self) -> Info<'_> {
        Info {
            name: self.name,
            uptime_s: Instant::now().as_secs(),
            ip: self
                .stack
                .config_v4()
                .map(|config| config.address.address()),
            free_heap: esp_alloc::HEAP.free(),
        }
    }
    fn read_logs(&self, from: Option<u64>, output: &mut [u8]) -> LogRead {
        log_sink::read(from, output)
    }
    fn submit_ota(&self, command: &[u8]) -> Result<(), &'static str> {
        ota::submit(command, false)
    }
    fn config(&self) -> &DeviceConfig {
        self.settings
    }
    fn device_credential(&self) -> CoapCredential {
        crate::config::device_credential()
    }
    fn write_config(&self, config: DeviceConfig) -> Result<(), Busy> {
        queue(|| {
            Job::Config(Command::Configure {
                id: "coap".try_into().expect("short ID"),
                config,
            })
        })
    }
    fn save_clients(&self, clients: ClientList) -> Result<(), Busy> {
        queue(|| Job::CoapClients(clients))
    }
    fn reboot(&self) -> Result<(), Busy> {
        queue(|| Job::Reboot)
    }
    fn upload_enabled(&self) -> bool {
        ota::rollback_enabled()
    }
    fn authenticated(&self) {
        ota::COAP_AUTHENTICATED.store(true, Ordering::Release);
    }
}

/// Tells observers when a resource changes. Runs for as long as the server.
async fn notify_changes(handle: mai_coap::endpoint::Handle<'_, DATAGRAM_BYTES, 0>) -> ! {
    let mut light = light_state::LIGHT_STATE_CHANGED
        .dyn_receiver()
        .expect("receiver capacity");
    let mut ota_status = ota::STATUS.dyn_receiver().expect("receiver capacity");
    let mut temperature = tsens::TEMPERATURE
        .dyn_receiver()
        .expect("receiver capacity");
    loop {
        let changed: &[u32] = match select4(
            light.changed(),
            ota_status.changed(),
            temperature.changed(),
            log_sink::LOGGED.wait(),
        )
        .await
        {
            Either4::First(()) => &[resource::LIGHT, resource::SPEED],
            Either4::Second(_) => &[resource::FIRMWARE],
            Either4::Third(_) => &[resource::TEMPERATURE],
            Either4::Fourth(()) => &[resource::LOGS],
        };
        for &id in changed {
            // A full mailbox only happens with changes already waiting to be sent, and
            // a notification always carries the latest value.
            let _ = handle.notify(id);
        }
    }
}

#[embassy_executor::task]
pub async fn task(
    stack: Stack<'static>,
    identity: &'static DeviceIdentity,
    settings: &'static DeviceConfig,
) {
    let skipped = CLIENT_KEYS.load(&settings.coap_clients);
    if skipped > 0 {
        warn!("Ignoring {} unusable CoAP client keys", skipped);
    }
    let Ok(rng) = Trng::try_new() else {
        error!("CoAP disabled: no entropy source");
        return;
    };
    let mut crypto = AcceleratedCrypto::new(Hardware, rng);
    let Ok(identity_key) = coap_keys::identity(&settings.coap_device_key, &mut crypto) else {
        error!("CoAP disabled: unusable device key");
        return;
    };
    let Ok(protection) = Edhoc::with_store(crypto, identity_key, &CLIENT_KEYS) else {
        error!("CoAP disabled: invalid security capacity");
        return;
    };

    let mut rx_meta = [PacketMetadata::EMPTY; SOCKET_RX_SLOTS];
    let mut tx_meta = [PacketMetadata::EMPTY; SOCKET_TX_SLOTS];
    let mut rx = [0; 2 * DATAGRAM_BYTES];
    let mut tx = [0; 2 * DATAGRAM_BYTES];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    while socket.bind(PORT).is_err() {
        Timer::after_secs(1).await;
    }
    let mut transport = EmbassyTransport { socket };

    let device = HardwareDevice {
        stack,
        name: &identity.slug,
        settings,
    };
    let manifest = PendingManifest::new();
    let mut resources = Resources::new(&device, &CLIENT_KEYS, &manifest, FlashUpload);
    let mut router = ApiRouter::new(&POLICY);
    resources.register(&mut router).expect("router capacity");
    let mut application = Authenticated {
        device: &device,
        router,
    };

    let shared = ApiShared::new();
    let Ok(mut endpoint) = ApiEndpoint::new(&shared, protection) else {
        error!("CoAP disabled: invalid endpoint capacity");
        return;
    };
    info!("CoAP serving on port {}", PORT);

    let notifier = notify_changes(shared.handle());
    let mut notifier = core::pin::pin!(notifier);
    loop {
        let server = endpoint.run(&mut transport, &EmbassyClock, &mut application, &POLICY);
        match select(server, notifier.as_mut()).await {
            Either::First(Err(RunError::Transport(_))) => warn!("CoAP socket failed, restarting"),
            Either::First(_) => warn!("CoAP endpoint stopped, restarting"),
        }
        // Keeps its state, so running it again resumes where it stopped.
        Timer::after_secs(1).await;
    }
}
