//! Which address families the Wi-Fi interface is configured for, for any number of waiting
//! tasks.
//!
//! embassy-net keeps one waker per interface for these changes, so a second task waiting on
//! the interface itself would replace the first one's waker and leave it asleep. This task is
//! that one waiter.

#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_futures::select::select;
use embassy_net::{iface::Iface, wire::IpCidr};
#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

/// IPv4 from DHCP and routable IPv6 from SLAAC. They arrive seconds apart, in either order.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    pub v4: bool,
    pub v6: bool,
}

/// One receiver per concurrent waiter: MQTT and mDNS.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
static CONFIG: Watch<CriticalSectionRawMutex, Config, 2> = Watch::new();

#[cfg(any(feature = "mdns", feature = "mqtt"))]
#[embassy_executor::task]
pub async fn task(iface: Iface<'static>) {
    let sender = CONFIG.sender();
    loop {
        let config = Config {
            v4: iface.is_config_v4_up(),
            v6: iface.is_config_v6_up(),
        };
        sender.send_if_modified(|state| {
            let changed = *state != Some(config);
            *state = Some(config);
            changed
        });
        // Both waits come from this one task, so they share its waker.
        select(
            async {
                if config.v4 {
                    iface.wait_config_v4_down().await
                } else {
                    iface.wait_config_v4_up().await
                }
            },
            async {
                if config.v6 {
                    iface.wait_config_v6_down().await
                } else {
                    iface.wait_config_v6_up().await
                }
            },
        )
        .await;
    }
}

/// Waits until the configuration satisfies `ready`, and returns it.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
pub async fn wait_config(ready: impl Fn(Config) -> bool) -> Config {
    let mut receiver = CONFIG.receiver().expect("CONFIG receiver capacity");
    receiver.get_and(|state| ready(*state)).await
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
