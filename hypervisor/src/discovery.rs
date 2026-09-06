//! Aggregated LAN discovery for the fleet:
//!   * one standard `LANLINE-BEACON` datagram per running child, so existing
//!     Android / CLI clients see N servers with no change;
//!   * one `LANLINE-FLEET-BEACON` datagram describing the whole fleet (the
//!     grouping + labels + hypervisor address the Android chooser wants);
//!   * one `_lanline._tcp` mDNS instance per child;
//!   * a small HTTP endpoint (`GET /api/v1/fleet`, `GET /`).

use crate::supervisor::Child;
use axum::extract::State;
use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use uuid::Uuid;

use lanline_server::catalog::base_capabilities;
use lanline_server::model::{PROTOCOL_VERSION, SERVER_VERSION};

pub struct Opts {
    pub bind: IpAddr,
    pub advertise_host: IpAddr,
    pub beacon_port: u16,
    pub fleet_port: u16,
    pub no_beacon: bool,
    pub no_mdns: bool,
    pub devices_available: usize,
}

pub async fn run(
    fleet_id: Uuid,
    children: Arc<Vec<Arc<Child>>>,
    opts: Opts,
    mut shutdown: watch::Receiver<bool>,
) {
    let hostname = lanline_server::net::hostname();

    // mDNS — one instance per child, held for the lifetime of this task.
    let _mdns_guards: Vec<_> = if opts.no_mdns {
        Vec::new()
    } else if let IpAddr::V4(ip) = opts.advertise_host {
        children
            .iter()
            .filter_map(|c| {
                let scheme = if c.spec.tls { "https" } else { "http" };
                lanline_server::mdns::spawn(
                    &format!("lanline-{}", c.spec.idx),
                    ip,
                    c.spec.ports.c2,
                    c.spec.server_id,
                    scheme,
                )
            })
            .collect()
    } else {
        tracing::warn!("advertise host is not IPv4 — mDNS disabled");
        Vec::new()
    };

    // Fleet HTTP.
    if opts.fleet_port != 0 {
        let state = FleetState {
            fleet_id,
            hostname: hostname.clone(),
            advertise_host: opts.advertise_host,
            fleet_port: opts.fleet_port,
            children: children.clone(),
        };
        let bind = opts.bind;
        let port = opts.fleet_port;
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind((bind, port)).await {
                Ok(l) => {
                    tracing::info!("fleet: http://{}:{port}/  (GET /api/v1/fleet)", state.advertise_host);
                    let app = Router::new()
                        .route("/", get(index))
                        .route("/api/v1/fleet", get(fleet_json))
                        .with_state(state);
                    let _ = axum::serve(l, app).await;
                }
                Err(e) => tracing::error!("fleet: cannot bind {bind}:{port}: {e}"),
            }
        });
    }

    if opts.no_beacon {
        let _ = shutdown.changed().await;
        return;
    }

    let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("beacon: bind failed: {e}");
            let _ = shutdown.changed().await;
            return;
        }
    };
    if let Err(e) = sock.set_broadcast(true) {
        tracing::error!("beacon: SO_BROADCAST: {e}");
    }
    tracing::info!("beacon: udp/{} every 1000 ms ({} radios)", opts.beacon_port, children.len());

    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => return,
        }

        let now = time::OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default();
        let targets = lanline_server::net::broadcast_targets();

        // Per-child LANLINE-BEACON (only while the child is up).
        for c in children.iter() {
            if !c.running() {
                continue;
            }
            let payload = child_beacon(c, &hostname, opts.advertise_host, opts.devices_available, &now);
            send_to_all(&sock, &targets, opts.advertise_host, opts.beacon_port, &payload).await;
        }

        // One fleet beacon.
        let fleet = fleet_beacon(fleet_id, &children, &hostname, opts.advertise_host, opts.fleet_port, &now);
        send_to_all(&sock, &targets, opts.advertise_host, opts.beacon_port, &fleet).await;
    }
}

async fn send_to_all(
    sock: &UdpSocket,
    targets: &[(Ipv4Addr, Ipv4Addr)],
    fallback_host: IpAddr,
    port: u16,
    bytes: &[u8],
) {
    if targets.is_empty() {
        let _ = sock.send_to(bytes, SocketAddrV4::new(Ipv4Addr::BROADCAST, port)).await;
        let _ = fallback_host;
        return;
    }
    for (_iface, bcast) in targets {
        let _ = sock.send_to(bytes, SocketAddrV4::new(*bcast, port)).await;
    }
    let _ = sock.send_to(bytes, SocketAddrV4::new(Ipv4Addr::BROADCAST, port)).await;
}

fn child_beacon(
    c: &Child,
    hostname: &str,
    host: IpAddr,
    devices_available: usize,
    now: &str,
) -> Vec<u8> {
    let s = &c.spec;
    let scheme = if s.tls { "https" } else { "http" };
    let mut caps = base_capabilities();
    if s.enable_tx {
        caps.push("tx".to_string());
        caps.push("ptt".to_string());
    }
    let obj = json!({
        "magic": "LANLINE-BEACON",
        "protocol_version": PROTOCOL_VERSION,
        "server_id": s.server_id,
        "version": SERVER_VERSION,
        "hostname": hostname,
        "instance_label": s.label,
        "advertised_host": host.to_string(),
        "ports": {
            "c2": s.ports.c2,
            "audio_out": s.ports.audio_out,
            "audio_in": s.ports.audio_in,
            "beast": s.ports.beast,
            "ais_nmea": s.ports.ais_nmea,
        },
        "scheme": scheme,
        "c2_base_url": format!("{scheme}://{host}:{}", s.ports.c2),
        "device": {
            "driver": s.device_driver,
            "label": s.device_label,
            "serial": s.device_serial,
            "tx_capable": s.enable_tx,
        },
        "devices_available": devices_available,
        "capabilities": caps,
        "timestamp": now,
    });
    serde_json::to_vec(&obj).unwrap_or_default()
}

fn fleet_beacon(
    fleet_id: Uuid,
    children: &[Arc<Child>],
    hostname: &str,
    host: IpAddr,
    fleet_port: u16,
    now: &str,
) -> Vec<u8> {
    let radios: Vec<_> = children.iter().map(|c| radio_summary(c, host)).collect();
    let obj = json!({
        "magic": "LANLINE-FLEET-BEACON",
        "protocol_version": PROTOCOL_VERSION,
        "fleet_id": fleet_id,
        "hostname": hostname,
        "advertised_host": host.to_string(),
        "fleet_base_url": format!("http://{host}:{fleet_port}"),
        "version": SERVER_VERSION,
        "radios": radios,
        "timestamp": now,
    });
    serde_json::to_vec(&obj).unwrap_or_default()
}

fn radio_summary(c: &Child, host: IpAddr) -> serde_json::Value {
    let s = &c.spec;
    let scheme = if s.tls { "https" } else { "http" };
    json!({
        "idx": s.idx,
        "label": s.label,
        "server_id": s.server_id,
        "scheme": scheme,
        "c2_base_url": format!("{scheme}://{host}:{}", s.ports.c2),
        "device": s.device_label,
        "device_serial": s.device_serial,
        "running": c.running(),
        "restarts": c.restarts(),
        "pid": c.pid(),
        "started_at": c.started_at(),
        "ports": {
            "c2": s.ports.c2,
            "audio_out": s.ports.audio_out,
            "audio_in": s.ports.audio_in,
            "beast": s.ports.beast,
            "ais_nmea": s.ports.ais_nmea,
            "aprs": s.ports.aprs,
        },
    })
}

#[derive(Clone)]
struct FleetState {
    fleet_id: Uuid,
    hostname: String,
    advertise_host: IpAddr,
    fleet_port: u16,
    children: Arc<Vec<Arc<Child>>>,
}

async fn fleet_json(State(st): State<FleetState>) -> Json<serde_json::Value> {
    let radios: Vec<_> = st.children.iter().map(|c| radio_summary(c, st.advertise_host)).collect();
    Json(json!({
        "fleet_id": st.fleet_id,
        "hostname": st.hostname,
        "version": SERVER_VERSION,
        "fleet_base_url": format!("http://{}:{}", st.advertise_host, st.fleet_port),
        "radios": radios,
    }))
}

async fn index(State(st): State<FleetState>) -> Html<String> {
    let rows: String = st
        .children
        .iter()
        .map(|c| {
            let s = &c.spec;
            let scheme = if s.tls { "https" } else { "http" };
            let url = format!("{scheme}://{}:{}/", st.advertise_host, s.ports.c2);
            let dot = if c.running() { "🟢" } else { "🔴" };
            format!(
                "<li>{dot} <a href=\"{url}\">radio {} — {}</a> <small>{} · :{}</small></li>",
                s.idx, s.label, s.device_label, s.ports.c2
            )
        })
        .collect();
    Html(format!(
        "<!doctype html><meta charset=utf-8><title>LANline fleet</title>\
         <body style=\"font:16px system-ui;margin:3rem;max-width:40rem\">\
         <h1>LANline fleet — {}</h1><ul>{rows}</ul>\
         <p><small>JSON: <a href=\"/api/v1/fleet\">/api/v1/fleet</a></small></p>",
        st.hostname
    ))
}
