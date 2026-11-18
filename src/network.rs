//! Whether the Wi-Fi interface has an address, for any number of waiting tasks.
//!
//! embassy-net keeps one waker per interface for these changes, so a second task waiting on
//! the interface itself would replace the first one's waker and leave it asleep. This task is
//! that one waiter.

use embassy_net::{iface::Iface, wire::IpCidr};
#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

/// One receiver per concurrent waiter: MQTT and mDNS.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
static CONFIG_UP: Watch<CriticalSectionRawMutex, bool, 2> = Watch::new();

#[cfg(any(feature = "mdns", feature = "mqtt"))]
#[embassy_executor::task]
pub async fn task(iface: Iface<'static>) {
    let sender = CONFIG_UP.sender();
    loop {
        let up = iface.is_config_up();
        sender.send_if_modified(|state| {
            let changed = *state != Some(up);
            *state = Some(up);
            changed
        });
        if up {
            iface.wait_config_down().await;
        } else {
            iface.wait_config_up().await;
        }
    }
}

/// Waits until the interface has an address (`true`) or has lost it (`false`).
#[cfg(any(feature = "mdns", feature = "mqtt"))]
pub async fn wait_config(up: bool) {
    let mut receiver = CONFIG_UP.receiver().expect("CONFIG_UP receiver capacity");
    receiver.get_and(|state| *state == up).await;
}

/// The interface's IPv4 address, once DHCP has assigned one.
pub fn ipv4_address(iface: Iface<'_>) -> Option<core::net::Ipv4Addr> {
    iface.ip_addrs().iter().find_map(|addr| match addr.cidr {
        IpCidr::V4(cidr) => Some(cidr.address()),
        IpCidr::V6(_) => None,
    })
}

/// The interface's routable IPv6 address once SLAAC has configured one, or else its
/// link-local one.
pub fn ipv6_address(iface: Iface<'_>) -> Option<core::net::Ipv6Addr> {
    let addrs = iface.ip_addrs();
    let v6 = || {
        addrs.iter().filter_map(|addr| match addr.cidr {
            IpCidr::V6(cidr) => Some(cidr.address()),
            IpCidr::V4(_) => None,
        })
    };
    v6().find(|addr| !addr.is_unicast_link_local())
        .or_else(|| v6().next())
}
