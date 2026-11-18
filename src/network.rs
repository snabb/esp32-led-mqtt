//! The Wi-Fi interface's addresses, for any number of waiting tasks.
//!
//! embassy-net keeps one waker per interface for these changes, so a second task waiting on
//! the interface itself would replace the first one's waker and leave it asleep. This task is
//! that one waiter.

use core::net::{Ipv4Addr, Ipv6Addr};

#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_futures::select::select3;
use embassy_net::{iface::Iface, wire::IpCidr};
#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
#[cfg(any(feature = "mdns", feature = "mqtt"))]
use embassy_time::{Duration, Timer};

/// The addresses services should use: IPv4 from DHCP and routable IPv6 from SLAAC. They arrive
/// seconds apart, in either order.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Config {
    pub v4: Option<Ipv4Addr>,
    pub v6: Option<Ipv6Addr>,
}

/// One receiver per concurrent waiter: MQTT and mDNS.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
static CONFIG: Watch<CriticalSectionRawMutex, Config, 2> = Watch::new();

/// How often to look for address changes that keep the family up, such as a new SLAAC prefix
/// or a deprecated one. embassy-net only lets this task wait for a family to come or go.
#[cfg(any(feature = "mdns", feature = "mqtt"))]
const RECHECK: Duration = Duration::from_secs(30);

#[cfg(any(feature = "mdns", feature = "mqtt"))]
#[embassy_executor::task]
pub async fn task(iface: Iface<'static>) {
    let sender = CONFIG.sender();
    loop {
        let config = Config {
            v4: ipv4_address(iface),
            v6: routable_ipv6_address(iface),
        };
        sender.send_if_modified(|state| {
            let changed = *state != Some(config);
            *state = Some(config);
            changed
        });
        // All waits come from this one task, so they share its waker.
        select3(
            async {
                if config.v4.is_some() {
                    iface.wait_config_v4_down().await
                } else {
                    iface.wait_config_v4_up().await
                }
            },
            async {
                if config.v6.is_some() {
                    iface.wait_config_v6_down().await
                } else {
                    iface.wait_config_v6_up().await
                }
            },
            Timer::after(RECHECK),
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
pub fn ipv4_address(iface: Iface<'_>) -> Option<Ipv4Addr> {
    iface.ip_addrs().iter().find_map(|addr| match addr.cidr {
        IpCidr::V4(cidr) => Some(cidr.address()),
        IpCidr::V6(_) => None,
    })
}

/// The interface's SLAAC address, preferring one whose prefix is not being retired.
pub fn routable_ipv6_address(iface: Iface<'_>) -> Option<Ipv6Addr> {
    let addrs = iface.ip_addrs();
    let routable = || {
        addrs.iter().filter_map(|addr| match addr.cidr {
            IpCidr::V6(cidr) if !cidr.address().is_unicast_link_local() => {
                Some((cidr.address(), addr.is_preferred()))
            }
            _ => None,
        })
    };
    routable()
        .find(|(_, preferred)| *preferred)
        .or_else(|| routable().next())
        .map(|(addr, _)| addr)
}

/// The interface's routable IPv6 address, or else its link-local one.
pub fn ipv6_address(iface: Iface<'_>) -> Option<Ipv6Addr> {
    routable_ipv6_address(iface).or_else(|| {
        iface.ip_addrs().iter().find_map(|addr| match addr.cidr {
            IpCidr::V6(cidr) => Some(cidr.address()),
            IpCidr::V4(_) => None,
        })
    })
}
