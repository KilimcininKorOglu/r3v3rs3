//! The registry of the admin routers that this process serves. The HTTP proxy hands a request
//! whose upstream is the address of a panel to its router in process, without the loopback TCP
//! hop that loses the client address, the TLS state and the `Host` of the request.

use axum::Router;
use dashmap::DashMap;
use http::uri::Authority;
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;

struct PanelHandle {
    addr: SocketAddr,
    router: Router,
}

/// The panels of this process by port. The tests start several panels in one process, each on
/// its own port.
static PANELS: LazyLock<DashMap<u16, PanelHandle>> = LazyLock::new(DashMap::new);

/// Registers the router of the panel that listens on `addr`.
pub fn install(addr: SocketAddr, router: Router) {
    PANELS.insert(addr.port(), PanelHandle { addr, router });
}

/// Removes the panel that listens on `addr`.
pub fn clear(addr: SocketAddr) {
    PANELS.remove_if(&addr.port(), |_, panel| panel.addr == addr);
}

/// The router of the panel at the upstream `authority`, if this process serves it.
pub fn matches(authority: &Authority) -> Option<Router> {
    let port = authority.port_u16()?;
    let panel = PANELS.get(&port)?;
    addr_matches(panel.addr, authority.host()).then(|| panel.router.clone())
}

/// Whether `host` names the panel bound to `bound`, that is whether a TCP connection to `host`
/// would reach it: the bound address itself, a loopback address of a panel on every interface,
/// or `localhost` for a panel on a loopback or an unspecified address.
fn addr_matches(bound: SocketAddr, host: &str) -> bool {
    let bound = bound.ip();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) => ip == bound || (ip.is_loopback() && bound.is_unspecified()),
        Err(_) => {
            host.eq_ignore_ascii_case("localhost")
                && (bound.is_loopback() || bound.is_unspecified())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(addr: &str) -> SocketAddr {
        addr.parse().unwrap()
    }

    #[test]
    fn a_loopback_host_reaches_a_loopback_panel() {
        let panel = bound("127.0.0.1:46492");
        assert!(addr_matches(panel, "127.0.0.1"));
        assert!(addr_matches(panel, "localhost"));
        assert!(addr_matches(panel, "LocalHost"));
        assert!(addr_matches(bound("[::1]:46492"), "[::1]"));
    }

    #[test]
    fn another_loopback_address_does_not_reach_a_loopback_panel() {
        let panel = bound("127.0.0.1:46492");
        assert!(!addr_matches(panel, "[::1]"));
        assert!(!addr_matches(panel, "127.0.0.2"));
    }

    #[test]
    fn a_loopback_host_reaches_a_panel_on_every_interface() {
        assert!(addr_matches(bound("0.0.0.0:46492"), "127.0.0.1"));
        assert!(addr_matches(bound("0.0.0.0:46492"), "127.0.0.2"));
        assert!(addr_matches(bound("[::]:46492"), "localhost"));
    }

    #[test]
    fn another_host_must_be_the_bound_address() {
        let panel = bound("10.0.0.5:46492");
        assert!(addr_matches(panel, "10.0.0.5"));
        assert!(!addr_matches(panel, "127.0.0.1"));
        assert!(!addr_matches(panel, "localhost"));
        assert!(!addr_matches(bound("127.0.0.1:46492"), "10.0.0.5"));
        assert!(!addr_matches(bound("0.0.0.0:46492"), "app.example.com"));
    }

    #[test]
    fn the_registry_matches_the_port_and_the_host() {
        let addr = bound("127.0.0.1:1");
        install(addr, Router::new());
        assert!(matches(&Authority::from_static("localhost:1")).is_some());
        assert!(matches(&Authority::from_static("127.0.0.1:2")).is_none());
        assert!(matches(&Authority::from_static("192.0.2.1:1")).is_none());
        assert!(matches(&Authority::from_static("localhost")).is_none());
        clear(bound("127.0.0.1:2"));
        clear(bound("0.0.0.0:1"));
        assert!(matches(&Authority::from_static("localhost:1")).is_some());
        clear(addr);
        assert!(matches(&Authority::from_static("localhost:1")).is_none());
    }
}
