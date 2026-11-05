//! The CoAP server: binds `esp32_led_mqtt::coap_api` to the hardware and the network.

use core::sync::atomic::Ordering;

use embassy_futures::select::{Either, select};
use embassy_net::{
    Stack,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Instant, Timer};
use esp_hal::rng::Trng;
use esp32_led_mqtt::{
    coap_api::{
        ApiEndpoint, ApiRouter, ApiShared, Authenticated, ClientKeys, DATAGRAM_BYTES, Device, Info,
        POLICY, PORT, Resources, resource,
    },
    coap_keys,
    device_config::DeviceConfig,
    light_command::{LightState, LightUpdate},
};
use heapless::String;
use log::{error, info, warn};
use mai_coap::{
    crypto::SoftwareCrypto, edhoc::endpoint::Edhoc, endpoint::RunError, time::EmbassyClock,
    transport::embassy::EmbassyTransport,
};

use crate::{identity::DeviceIdentity, light_state, ota, tsens};

const SOCKET_RX_SLOTS: usize = 4;
const SOCKET_TX_SLOTS: usize = 2;

static CLIENT_KEYS: ClientKeys = ClientKeys::new();

struct Hardware {
    stack: Stack<'static>,
    name: &'static str,
}

impl Device for Hardware {
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
        use embassy_futures::select::{Either3, select3};
        let changed: &[u32] =
            match select3(light.changed(), ota_status.changed(), temperature.changed()).await {
                Either3::First(()) => &[resource::LIGHT, resource::SPEED],
                Either3::Second(_) => &[resource::FIRMWARE],
                Either3::Third(_) => &[resource::TEMPERATURE],
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
    let Ok(rng_for_identity) = Trng::try_new() else {
        error!("CoAP disabled: no entropy source");
        return;
    };
    let Ok(identity_key) = coap_keys::identity(
        &settings.coap_device_key,
        &mut SoftwareCrypto::new(rng_for_identity),
    ) else {
        error!("CoAP disabled: unusable device key");
        return;
    };
    let Ok(protection) = Edhoc::with_store(SoftwareCrypto::new(rng), identity_key, &CLIENT_KEYS)
    else {
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

    let device = Hardware {
        stack,
        name: &identity.slug,
    };
    let mut resources = Resources::new(&device);
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
