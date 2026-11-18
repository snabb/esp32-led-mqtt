//! Multicast DNS responder, so a device can be found as `esp32-led-XXXXXX.local`, and its
//! CoAP service with `avahi-browse _coap._udp`.

use core::net::{Ipv4Addr, Ipv6Addr};

#[cfg(feature = "coap")]
use edge_mdns::host::{Service, ServiceAnswers};
use edge_mdns::{
    HostAnswer, HostAnswers, HostAnswersMdnsHandler, MdnsError, MdnsHandler, MdnsRequest,
    MdnsResponse, domain::base::Ttl, host::Host,
};
use embassy_futures::select::select;
use embassy_net::{
    iface::Iface,
    udp::UdpSocket,
    wire::{IpAddr, ListenSocketAddr, SocketAddr},
};
use embassy_time::Timer;
use esp_hal::rng::Rng;
use log::{info, warn};

use crate::{identity::DeviceIdentity, network};

/// RFC 6762's recommendation for records that name a host.
const TTL_SECS: u32 = 120;
/// An Ethernet payload less the IPv6 and UDP headers, so that a message fits either family.
const MAX_MESSAGE: usize = 1452;
const PORT: u16 = 5353;
const GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);

#[embassy_executor::task]
pub async fn task(iface: Iface<'static>, identity: &'static DeviceIdentity, advertise_coap: bool) {
    let answers = Answers {
        iface,
        hostname: &identity.slug,
        advertise_coap,
    };
    let mut handler = HostAnswersMdnsHandler::new(&answers);
    let Ok(mut socket) = UdpSocket::new(iface.stack()) else {
        warn!("mDNS disabled: no free UDP socket");
        return;
    };
    // RFC 6762 §11: older queriers drop responses with any other IP TTL.
    if socket.set_hop_limit(Some(255)).is_err()
        || iface.join_multicast_group(GROUP_V4).is_err()
        || iface.join_multicast_group(GROUP_V6).is_err()
        || socket.bind(PORT, ListenSocketAddr::UNSPECIFIED).is_err()
    {
        warn!("mDNS disabled: could not bind its socket");
        return;
    }
    let mut request = [0; MAX_MESSAGE];
    let mut response = [0; MAX_MESSAGE];

    loop {
        let config = network::wait_config(|state| state.v4.is_some() || state.v6.is_some()).await;
        info!("mDNS answering as {}.local", identity.slug);
        // Announce the addresses, which may be new. IPv6 announces go out even with only a
        // link-local address, which is what an IPv6 querier reaches.
        if let Ok(MdnsResponse::Reply { data, .. }) =
            handler.handle(MdnsRequest::None, &mut response)
        {
            if config.v4.is_some() {
                send(&socket, data, SocketAddr::new(GROUP_V4.into(), PORT)).await;
            }
            send(&socket, data, SocketAddr::new(GROUP_V6.into(), PORT)).await;
        }
        select(
            serve(&socket, &mut handler, &mut request, &mut response),
            network::wait_config(|state| state != config),
        )
        .await;
    }
}

async fn serve(
    socket: &UdpSocket<'_>,
    handler: &mut impl MdnsHandler,
    request: &mut [u8],
    response: &mut [u8],
) -> ! {
    loop {
        let Ok((len, meta)) = socket.recv_from(request).await else {
            continue;
        };
        // RFC 6762 §6.7: a query from another port is a one-shot query, answered directly.
        let legacy = meta.remote_addr.port != PORT;
        let request = MdnsRequest::Request {
            legacy,
            multicast: meta.local_addr.is_some_and(|addr| addr.is_multicast()),
            data: &request[..len],
        };
        let (data, delay) = match handler.handle(request, response) {
            Ok(MdnsResponse::Reply { data, delay }) => (data, delay),
            Ok(MdnsResponse::None) | Err(MdnsError::InvalidMessage) => continue,
            Err(err) => {
                warn!("mDNS could not answer: {:?}", err);
                continue;
            }
        };
        if legacy {
            send(socket, data, meta.remote_addr).await;
            continue;
        }
        if delay {
            // RFC 6762 §6: 20–120 ms, so that responders to a shared record don't collide.
            Timer::after_millis(20 + u64::from(Rng::new().random()) % 100).await;
        }
        // Answer in the family the query came in: a querier only listens on its own group.
        let group = match meta.remote_addr.addr {
            IpAddr::V4(_) => GROUP_V4.into(),
            IpAddr::V6(_) => GROUP_V6.into(),
        };
        send(socket, data, SocketAddr::new(group, PORT)).await;
    }
}

async fn send(socket: &UdpSocket<'_>, data: &[u8], to: SocketAddr) {
    if let Err(err) = socket.send_to(data, to).await {
        warn!("mDNS send failed: {:?}", err);
    }
}

/// Reads the addresses on each query, because DHCP and SLAAC can change them.
struct Answers<'a> {
    iface: Iface<'a>,
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
            ipv4: network::ipv4_address(self.iface).unwrap_or(Ipv4Addr::UNSPECIFIED),
            ipv6: network::ipv6_address(self.iface).unwrap_or(Ipv6Addr::UNSPECIFIED),
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
