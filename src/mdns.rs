//! Multicast DNS responder, so a device can be found as `esp32-led-XXXXXX.local`, and its
//! CoAP service with `avahi-browse _coap._udp`.

use core::net::{Ipv4Addr, Ipv6Addr};

#[cfg(feature = "coap")]
use edge_mdns::host::{Service, ServiceAnswers};
use edge_mdns::{
    HostAnswer, HostAnswers, HostAnswersMdnsHandler, MdnsError,
    buf::VecBufAccess,
    domain::base::Ttl,
    host::Host,
    io::{self, IPV4_DEFAULT_SOCKET},
};
use edge_nal::UdpSplit;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_futures::select::select;
use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::Timer;
use esp_hal::rng::Rng;
use log::{info, warn};

use crate::identity::DeviceIdentity;

/// RFC 6762's recommendation for records that name a host.
const TTL_SECS: u32 = 120;
/// An Ethernet payload less the IPv4 and UDP headers.
const MAX_MESSAGE: usize = 1472;

#[embassy_executor::task]
pub async fn task(stack: Stack<'static>, identity: &'static DeviceIdentity, advertise_coap: bool) {
    let answers = Answers {
        stack,
        hostname: &identity.slug,
        advertise_coap,
    };
    let buffers = UdpBuffers::<1, MAX_MESSAGE, MAX_MESSAGE, 2>::new();
    let udp = Udp::new(stack, &buffers);
    let recv_buf = VecBufAccess::<NoopRawMutex, MAX_MESSAGE>::new();
    let send_buf = VecBufAccess::<NoopRawMutex, MAX_MESSAGE>::new();
    let announce = Signal::<NoopRawMutex, ()>::new();

    loop {
        // The responder stops at the first failed send, and sends fail without an address.
        stack.wait_config_up().await;
        // edge-nal-embassy ignores the interface: a stack is one interface.
        let interface = Some(Ipv4Addr::UNSPECIFIED);
        match io::bind(&udp, IPV4_DEFAULT_SOCKET, interface, None).await {
            Ok(mut socket) => {
                let (recv, send) = socket.split();
                let mdns = io::Mdns::new(
                    interface,
                    None,
                    recv,
                    send,
                    &recv_buf,
                    &send_buf,
                    Rng::new(),
                    &announce,
                );
                info!("mDNS answering as {}.local", identity.slug);
                let handler = HostAnswersMdnsHandler::new(&answers);
                select(mdns.run(handler), stack.wait_config_down()).await;
            }
            Err(_) => warn!("mDNS could not bind its socket"),
        }
        Timer::after_secs(1).await;
    }
}

/// Reads the address on each query, because DHCP can change it.
struct Answers<'a> {
    stack: Stack<'a>,
    hostname: &'a str,
    advertise_coap: bool,
}

impl HostAnswers for Answers<'_> {
    fn visit<F, E>(&self, f: F) -> Result<(), E>
    where
        F: FnMut(HostAnswer) -> Result<(), E>,
        E: From<MdnsError>,
    {
        let host = Host {
            hostname: self.hostname,
            ipv4: self
                .stack
                .config_v4()
                .map_or(Ipv4Addr::UNSPECIFIED, |config| config.address.address()),
            ipv6: Ipv6Addr::UNSPECIFIED,
            ttl: Ttl::from_secs(TTL_SECS),
        };
        if self.advertise_coap {
            #[cfg(feature = "coap")]
            {
                let service = Service {
                    name: self.hostname,
                    priority: 0,
                    weight: 0,
                    service: "_coap",
                    protocol: "_udp",
                    port: esp32_led_mqtt::coap_api::PORT,
                    service_subtypes: &[],
                    txt_kvs: &[],
                };
                return ServiceAnswers::new(&host, &service).visit(f);
            }
        }
        host.visit(f)
    }
}
