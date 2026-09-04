//! Multicast-DNS advertisement.
//!
//! The UDP discovery beacon (see `discovery.rs`) is only reachable by the
//! Android wrapper's native listener — a browser has no way to receive it.
//! mDNS fills that gap: it publishes `<name>.local` (an A record for the
//! advertised LAN IPv4) plus a `_lanline._tcp` service on the C2 port, so a
//! plain browser can open `http://<name>.local:<c2-port>/` with nothing to
//! type. Best-effort: any failure is logged and ignored.

use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::{IpAddr, Ipv4Addr};
use uuid::Uuid;

const SERVICE_TYPE: &str = "_lanline._tcp.local.";

/// Register the service. The returned daemon must be kept alive for the
/// advertisement to persist; drop it (or call `.shutdown()`) to stop.
pub fn spawn(name: &str, ip: Ipv4Addr, c2_port: u16, server_id: Uuid) -> Option<ServiceDaemon> {
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("mdns: responder unavailable ({e}); browser name resolution disabled");
            return None;
        }
    };

    let host = format!("{name}.local.");
    let sid = server_id.to_string();
    let props = [("path", "/"), ("proto", "1"), ("id", sid.as_str())];

    let addr = IpAddr::V4(ip);
    let info = match ServiceInfo::new(SERVICE_TYPE, name, &host, addr, c2_port, &props[..]) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("mdns: bad service info ({e})");
            return None;
        }
    };

    match daemon.register(info) {
        Ok(()) => {
            tracing::info!("mdns: advertising http://{name}.local:{c2_port}/  ({SERVICE_TYPE})");
            Some(daemon)
        }
        Err(e) => {
            tracing::warn!("mdns: register failed ({e})");
            None
        }
    }
}
