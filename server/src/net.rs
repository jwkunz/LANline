//! Small networking helpers: port allocation and primary-address detection.

use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};

/// Bind a TCP listener. `port` 0 means "pick an ephemeral port". Returns the
/// listener (non-blocking, ready for `tokio::net::TcpListener::from_std`) and
/// the resolved port.
pub fn bind_tcp(bind: IpAddr, port: u16) -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(SocketAddr::new(bind, port))
        .with_context(|| format!("binding TCP {bind}:{port}"))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Pick a free UDP port by briefly binding it. There is a small race between
/// drop and the eventual re-bind by the WebRTC stack (phase 1b); acceptable on
/// a single dev host. A fixed non-zero `port` is returned as-is after a bind
/// check.
pub fn pick_udp_port(bind: IpAddr, port: u16) -> Result<u16> {
    let sock = UdpSocket::bind(SocketAddr::new(bind, port))
        .with_context(|| format!("reserving UDP {bind}:{port}"))?;
    Ok(sock.local_addr()?.port())
}

/// Best guess at the primary outward-facing IPv4 address. Uses a connected but
/// unsent UDP socket to let the OS pick the right source address, then falls
/// back to scanning interfaces for a private IPv4.
pub fn primary_ipv4() -> Option<Ipv4Addr> {
    if let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        if sock.connect((Ipv4Addr::new(1, 1, 1, 1), 80)).is_ok() {
            if let Ok(SocketAddr::V4(v4)) = sock.local_addr() {
                let ip = *v4.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    return Some(ip);
                }
            }
        }
    }

    if_addrs::get_if_addrs().ok()?.into_iter().find_map(|iface| match iface.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    })
}

/// Directed broadcast addresses for every non-loopback IPv4 interface, paired
/// with that interface's own address (used as `advertised_host` in per-iface
/// beacon datagrams).
pub fn broadcast_targets() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    let mut out = Vec::new();
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return out;
    };
    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            let ip = u32::from(v4.ip);
            let mask = u32::from(v4.netmask);
            if mask == 0 {
                continue;
            }
            let bcast = Ipv4Addr::from(ip | !mask);
            out.push((v4.ip, bcast));
        }
    }
    out
}

/// The machine hostname (Linux). Falls back to `"unknown"`.
pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
