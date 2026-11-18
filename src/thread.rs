//! OpenThread owns mesh membership and routing. Embassy owns application sockets.

use core::{
    cell::{Cell, RefCell},
    net::Ipv6Addr,
    task::{Context, Waker},
};

use embassy_executor::Spawner;
use embassy_futures::select::{select, select3};
use embassy_net::{
    Runner, Stack, StackStorage, StaticPool,
    config::IFACE_ADDR_COUNT,
    driver::{Capabilities, Driver, HardwareAddress, LinkState, Medium, NotSupported, PacketBuf},
    iface::Iface,
    route::Route,
    wire::{IpCidr, Ipv6Cidr},
};
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    channel::{Channel, TrySendError},
};
use esp_hal::{efuse, peripherals::IEEE802154, rng::Trng};
use esp_radio::ieee802154::Ieee802154;
use esp32_led_mqtt::device_config::DeviceConfig;
use log::{info, warn};
use openthread::{
    DeviceRole, OpenThread, OtResources, OtSrpResources, SimpleRamSettings, SrpConf, SrpService,
    esp::EspRadio, sys,
};
use sha2::{Digest, Sha256};
use static_cell::StaticCell;
use tinyrlibc as _;

use crate::identity::DeviceIdentity;

const MTU: usize = 1280;
static POOL: StaticPool<MTU, 16, 4> = StaticPool::new();
static STACK: StaticCell<StackStorage<'static>> = StaticCell::new();
static IO: StaticCell<PacketIo> = StaticCell::new();
static DRIVER: StaticCell<MeshDriver> = StaticCell::new();
static RNG: StaticCell<Trng> = StaticCell::new();
static RESOURCES: StaticCell<OtResources> = StaticCell::new();
static SRP: StaticCell<OtSrpResources<1, 128>> = StaticCell::new();
static SETTINGS_BUFFER: StaticCell<[u8; 2048]> = StaticCell::new();
static SETTINGS: StaticCell<SimpleRamSettings<'static>> = StaticCell::new();

type PacketSlot = Channel<NoopRawMutex, PacketBuf, 1>;

// All users run on the same executor. One pool buffer circulates through
// `empty_rx`, the radio task, and `rx`; TX has its own independent queue.
struct PacketIo {
    empty_rx: PacketSlot,
    rx: PacketSlot,
    tx: PacketSlot,
    attached: Cell<bool>,
    stack_waker: RefCell<Option<Waker>>,
}

struct MeshDriver {
    io: &'static PacketIo,
    rx_in_flight: bool,
}

impl Driver for MeshDriver {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ip
    }

    fn link_state(&mut self) -> LinkState {
        if self.io.attached.get() {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn register_waker(&mut self, waker: &Waker) -> Result<(), NotSupported> {
        let mut current = self.io.stack_waker.borrow_mut();
        if !current
            .as_ref()
            .is_some_and(|current| current.will_wake(waker))
        {
            *current = Some(waker.clone());
        }
        let _ = self
            .io
            .rx
            .poll_ready_to_receive(&mut Context::from_waker(waker));
        Ok(())
    }

    fn rx_wanted(&mut self) -> usize {
        usize::from(!self.rx_in_flight)
    }

    fn rx_give(&mut self, packet: PacketBuf) {
        if !self.rx_in_flight {
            self.rx_in_flight = self.io.empty_rx.try_send(packet).is_ok();
        }
    }

    fn receive(&mut self) -> Option<PacketBuf> {
        let packet = self.io.rx.try_receive().ok()?;
        self.rx_in_flight = false;
        Some(packet)
    }

    fn can_transmit(&mut self) -> bool {
        match self.io.stack_waker.borrow().as_ref() {
            Some(waker) => self
                .io
                .tx
                .poll_ready_to_send(&mut Context::from_waker(waker))
                .is_ready(),
            None => !self.io.tx.is_full(),
        }
    }

    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        self.io
            .tx
            .try_send(packet)
            .map_err(|TrySendError::Full(packet)| packet)
    }
}

pub fn start(
    spawner: Spawner,
    peripheral: IEEE802154<'static>,
    config: &DeviceConfig,
    identity: &DeviceIdentity,
) -> Option<Iface<'static>> {
    match start_mesh(spawner, peripheral, config, identity) {
        Ok(iface) => Some(iface),
        Err(reason) => {
            warn!("Thread startup: {reason}");
            None
        }
    }
}

fn start_mesh(
    spawner: Spawner,
    peripheral: IEEE802154<'static>,
    config: &DeviceConfig,
    identity: &DeviceIdentity,
) -> Result<Iface<'static>, &'static str> {
    let profile = config
        .thread
        .as_ref()
        .ok_or("provision a Thread dataset over USB")?;
    let dataset = profile.dataset_bytes().map_err(|_| "invalid dataset")?;
    let seed = profile.seed_bytes().ok_or("missing device seed")?;
    let rng = Trng::try_new().map_err(|_| "hardware entropy unavailable")?;
    let stack_seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let rng = RNG.init(rng);

    let settings = SETTINGS.init(SimpleRamSettings::new(SETTINGS_BUFFER.init([0; 2048])));
    let iid_secret = Sha256::new()
        .chain_update(b"esp32-led thread SLAAC v1")
        .chain_update(seed)
        .finalize();
    settings
        .set(
            sys::OT_SETTINGS_KEY_SLAAC_IID_SECRET_KEY as u16,
            &iid_secret,
        )
        .map_err(|_| "OpenThread settings full")?;
    if cfg!(feature = "coap") && config.has_coap_keys() {
        settings
            .set(sys::OT_SETTINGS_KEY_SRP_ECDSA_KEY as u16, &srp_key(seed))
            .map_err(|_| "OpenThread settings full")?;
    }

    let mac = efuse::base_mac_address();
    let mac = mac.as_bytes();
    let eui64 = [
        mac[0] ^ 2,
        mac[1],
        mac[2],
        0xff,
        0xfe,
        mac[3],
        mac[4],
        mac[5],
    ];
    let thread = OpenThread::new_with_srp(
        eui64,
        rng,
        settings,
        RESOURCES.init(OtResources::new()),
        SRP.init(OtSrpResources::new()),
    )
    .map_err(|_| "OpenThread instance")?;
    thread
        .set_active_dataset_tlv(&dataset)
        .map_err(|_| "invalid dataset")?;
    thread
        .set_link_mode(true, false, true)
        .map_err(|_| "Thread link mode")?;
    thread.enable_ipv6(true).map_err(|_| "enable IPv6")?;
    thread.enable_ipv6_rx(true);
    // Embassy answers echo requests for addresses installed on the application interface.
    thread.with_instance(|instance| unsafe {
        sys::otIcmp6SetEchoMode(
            instance,
            sys::otIcmp6EchoMode_OT_ICMP6_ECHO_HANDLER_DISABLED,
        );
    });

    let io = &*IO.init(PacketIo {
        empty_rx: Channel::new(),
        rx: Channel::new(),
        tx: Channel::new(),
        attached: Cell::new(false),
        stack_waker: RefCell::new(None),
    });
    let (stack, runner) = Stack::new(STACK.init(StackStorage::new()), &POOL, stack_seed);
    let iface = stack
        .add_iface_borrowed(DRIVER.init(MeshDriver {
            io,
            rx_in_flight: false,
        }))
        .map_err(|_| "Thread interface slot")?;
    // This IP medium forwards the whole packet to OpenThread, which selects the actual next hop.
    stack
        .routes()
        .add(Route::new_ipv6_gateway(
            "fe80::1".parse().unwrap(),
            iface.handle(),
        ))
        .map_err(|_| "Thread route slot")?;
    if let Some(dns) = profile.dns {
        stack.set_dns_servers(&[dns.into()]);
    }

    let radio = EspRadio::new(Ieee802154::new(peripheral));
    thread.enable_thread(true).map_err(|_| "enable Thread")?;
    if cfg!(feature = "coap")
        && config.has_coap_keys()
        && let Err(error) = register_service(&thread, &identity.slug)
    {
        warn!("Thread SRP setup: {error:?}");
    }
    spawner.spawn(stack_task(runner).unwrap());
    spawner.spawn(mesh_task(thread, radio, io, iface).unwrap());
    Ok(iface)
}

fn register_service(thread: &OpenThread<'_>, host: &str) -> Result<(), openthread::OtError> {
    thread.srp_set_conf(&SrpConf {
        host_name: host,
        ..SrpConf::new()
    })?;
    thread.srp_add_service(&SrpService {
        name: "_coap._udp",
        instance_name: host,
        subtype_labels: core::iter::empty::<&str>(),
        txt_entries: core::iter::empty::<(&str, &[u8])>(),
        port: 5683,
        priority: 0,
        weight: 0,
        lease_secs: 0,
        key_lease_secs: 0,
    })?;
    thread.srp_autostart()
}

/// OpenThread stores a P-256 key as a 125-byte DER buffer followed by its length.
fn srp_key(seed: [u8; 32]) -> [u8; 126] {
    const ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63,
        0x25, 0x51,
    ];
    let scalar = (0u64..)
        .find_map(|index| {
            let candidate: [u8; 32] = Sha256::new()
                .chain_update(b"esp32-led thread SRP v1")
                .chain_update(seed)
                .chain_update(index.to_be_bytes())
                .finalize()
                .into();
            (candidate != [0; 32] && candidate < ORDER).then_some(candidate)
        })
        .expect("valid P-256 scalar");
    let mut key = [0; 126];
    key[..7].copy_from_slice(&[0x30, 49, 0x02, 1, 1, 0x04, 32]);
    key[7..39].copy_from_slice(&scalar);
    key[39..51].copy_from_slice(&[0xa0, 10, 0x06, 8, 0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7]);
    key[125] = 51;
    key
}

#[embassy_executor::task]
async fn stack_task(mut runner: Runner<'static>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn mesh_task(
    thread: OpenThread<'static>,
    radio: EspRadio<'static>,
    io: &'static PacketIo,
    iface: Iface<'static>,
) {
    let packets = select(receive_packets(&thread, io), send_packets(&thread, io));
    select3(
        thread.run(radio),
        follow_network(&thread, io, iface),
        packets,
    )
    .await;
}

async fn receive_packets(thread: &OpenThread<'_>, io: &PacketIo) {
    loop {
        let mut packet = io.empty_rx.receive().await;
        packet.set_len(MTU);
        loop {
            match thread.rx(&mut packet).await {
                Ok(len) => {
                    packet.set_len(len);
                    io.rx.send(packet).await;
                    break;
                }
                Err(error) => warn!("Thread receive: {error:?}"),
            }
        }
    }
}

async fn send_packets(thread: &OpenThread<'_>, io: &PacketIo) {
    loop {
        let packet = io.tx.receive().await;
        if let Err(error) = thread.tx(&packet) {
            warn!("Thread send: {error:?}");
        }
    }
}

async fn follow_network(thread: &OpenThread<'_>, io: &PacketIo, iface: Iface<'_>) {
    let mut installed = heapless::Vec::new();
    loop {
        let attached = matches!(
            thread.device_role(),
            DeviceRole::Child | DeviceRole::Router | DeviceRole::Leader
        );
        let addresses = if attached {
            mesh_addresses(thread)
        } else {
            heapless::Vec::new()
        };
        let changed = io.attached.replace(attached) != attached;
        if changed && let Some(waker) = io.stack_waker.borrow().as_ref() {
            waker.wake_by_ref();
        }
        if installed != addresses || changed {
            if iface
                .set_ip_addrs(addresses.iter().copied().map(IpCidr::V6))
                .is_ok()
            {
                for cidr in &addresses {
                    info!("Thread IPv6 {}", cidr.address());
                }
                if !attached {
                    info!("Thread detached");
                }
                installed = addresses;
            } else {
                warn!("Cannot install Thread addresses");
            }
        }
        thread.wait_changed().await;
    }
}

fn mesh_addresses(thread: &OpenThread<'_>) -> heapless::Vec<Ipv6Cidr, IFACE_ADDR_COUNT> {
    let mesh_prefix = thread.mesh_local_prefix();
    thread.with_instance(|instance| {
        let mut addresses = heapless::Vec::new();
        // OpenThread's safe iterator omits the validity and preference flags.
        // These pointers remain valid inside this activation, which does not mutate the netif.
        unsafe {
            let mut current = sys::otIp6GetUnicastAddresses(instance);
            while let Some(entry) = current.as_ref() {
                let bytes = entry.mAddress.mFields.m8;
                let addr = Ipv6Addr::from(bytes);
                if entry.mValid()
                    && entry.mPreferred()
                    && !addr.is_unicast_link_local()
                    && bytes[..8] != mesh_prefix
                {
                    let _ = addresses.push(Ipv6Cidr::new(addr, entry.mPrefixLength));
                }
                current = entry.mNext;
            }
            if addresses.is_empty()
                && let Some(eid) = sys::otThreadGetMeshLocalEid(instance).as_ref()
            {
                let _ = addresses.push(Ipv6Cidr::new(eid.mFields.m8.into(), 64));
            }
        }
        addresses
    })
}
