//! Aggregated LAN discovery for the fleet:
//!   * one standard `LANLINE-BEACON` datagram per running child, so existing
//!     Android / CLI clients see N servers with no change;
//!   * one `LANLINE-FLEET-BEACON` datagram describing the whole fleet (the
//!     grouping + labels + hypervisor address the Android chooser wants);
//!   * one `_lanline._tcp` mDNS instance per child;
//!   * a small HTTP endpoint (`GET /api/v1/fleet`, `GET /`).

use crate::supervisor::{now_unix, Child};
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

    let fleet_url = (opts.fleet_port != 0)
        .then(|| format!("http://{}:{}", opts.advertise_host, opts.fleet_port));

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
            let payload = child_beacon(
                c,
                &hostname,
                opts.advertise_host,
                fleet_url.as_deref(),
                opts.devices_available,
                &now,
            );
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
    fleet_url: Option<&str>,
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
        "fleet_url": fleet_url,
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn radio_card(c: &Child, host: IpAddr) -> String {
    let s = &c.spec;
    let scheme = if s.tls { "https" } else { "http" };
    let url = format!("{scheme}://{host}:{}/", s.ports.c2);
    let (dotcls, statetxt) = if c.running() { ("ok", "running") } else { ("bad", "down") };
    let up = c.started_at().map(|t| fmt_uptime(now_unix().saturating_sub(t))).unwrap_or_default();
    let restarts = c.restarts();
    let restart_chip = if restarts > 0 {
        format!("<span class=\"chip warn\">↻ {restarts}</span>")
    } else {
        String::new()
    };
    format!(
        r#"<article class="card radio">
  <div class="rc-head">
    <span class="badge"><span class="dot {dotcls}"></span>{label}</span>
    <a class="btn" href="{url}">Open ↗</a>
  </div>
  <div class="rc-dev">{dev}</div>
  <div class="chips">
    <span class="chip">{scheme} · :{c2}</span>
    <span class="chip">{statetxt}{up_sep}{up}</span>
    {restart_chip}
  </div>
</article>"#,
        label = esc(&s.label),
        dev = esc(&s.device_label),
        c2 = s.ports.c2,
        up_sep = if up.is_empty() { "" } else { " · " },
    )
}

fn fmt_uptime(secs: u64) -> String {
    if secs < 90 {
        format!("{secs}s")
    } else if secs < 5400 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

async fn index(State(st): State<FleetState>) -> Html<String> {
    let cards: String = st.children.iter().map(|c| radio_card(c, st.advertise_host)).collect();
    let n = st.children.len();
    let plural = if n == 1 { "radio" } else { "radios" };
    let host = esc(&st.hostname);
    Html(format!(
        r##"<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>LANline fleet — {host}</title>
<style>
:root {{
  --bg:#f6f7f9; --card:#fff; --ink:#1a1c1f; --muted:#5c636e; --line:#e3e6ea;
  --accent:#2563eb; --ok:#15803d; --bad:#b91c1c; --warn:#b45309; --chip:#eef1f5;
}}
@media (prefers-color-scheme: dark) {{
  :root {{
    --bg:#0f1216; --card:#171b21; --ink:#e8eaed; --muted:#9aa3af; --line:#262c34;
    --accent:#60a5fa; --ok:#4ade80; --bad:#f87171; --warn:#fbbf24; --chip:#222831;
  }}
}}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--ink);
  font:14px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
.wrap {{ max-width:820px; margin:0 auto; padding:24px 16px 48px; }}
h1 {{ font-size:20px; margin:0 0 4px; letter-spacing:.2px; }}
h1 .sub {{ color:var(--muted); font-weight:400; font-size:14px; }}
.tagline {{ color:var(--muted); margin:0 0 20px; }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:10px;
  padding:16px; margin-bottom:14px; }}
.card.radio {{ transition:border-color .15s; }}
.card.radio:hover {{ border-color:var(--accent); }}
.rc-head {{ display:flex; align-items:center; justify-content:space-between; gap:12px; }}
.badge {{ display:inline-flex; align-items:center; gap:8px; font-weight:700; font-size:18px; }}
.dot {{ width:10px; height:10px; border-radius:50%; background:var(--muted); flex:none; }}
.dot.ok {{ background:var(--ok); }}
.dot.bad {{ background:var(--bad); }}
.rc-dev {{ color:var(--muted); margin:8px 0 10px; font-variant-numeric:tabular-nums;
  word-break:break-word; }}
.chips {{ display:flex; gap:6px; flex-wrap:wrap; }}
.chip {{ background:var(--chip); border-radius:999px; padding:2px 10px; font-size:12px;
  color:var(--muted); font-variant-numeric:tabular-nums; }}
.chip.warn {{ color:var(--warn); }}
a.btn {{ padding:9px 16px; border:1px solid transparent; border-radius:8px;
  background:var(--accent); color:#fff; font-weight:600; text-decoration:none;
  white-space:nowrap; }}
a.btn:hover {{ filter:brightness(1.06); }}
.foot {{ color:var(--muted); font-size:13px; margin-top:8px; }}
.foot a {{ color:var(--accent); }}
</style></head><body><div class="wrap">
<h1>LANline <span class="sub">fleet · {host}</span></h1>
<p class="tagline">{n} {plural} on this host — pick one to open its radio page.</p>
<div id="fleet-list">{cards}</div>
<p class="foot">JSON: <a href="/api/v1/fleet">/api/v1/fleet</a></p>
</div>
<script>
const FL = document.getElementById('fleet-list');
function fmtUp(s){{ if(s<90)return s+'s'; if(s<5400)return Math.floor(s/60)+'m';
  return Math.floor(s/3600)+'h'+Math.floor((s%3600)/60)+'m'; }}
function esc(s){{ return String(s).replace(/[&<>"]/g,c=>({{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}}[c])); }}
function card(r){{
  const dot = r.running ? 'ok' : 'bad';
  const st = r.running ? 'running' : 'down';
  const up = r.started_at ? ' · '+fmtUp(Math.max(0, Math.floor(Date.now()/1000)-r.started_at)) : '';
  const rs = r.restarts > 0 ? `<span class="chip warn">&#8635; ${{r.restarts}}</span>` : '';
  return `<article class="card radio"><div class="rc-head">`
    + `<span class="badge"><span class="dot ${{dot}}"></span>${{esc(r.label)}}</span>`
    + `<a class="btn" href="${{esc(r.c2_base_url)}}/">Open &#8599;</a></div>`
    + `<div class="rc-dev">${{esc(r.device||'')}}</div>`
    + `<div class="chips"><span class="chip">${{esc(r.scheme)}} · :${{r.ports.c2}}</span>`
    + `<span class="chip">${{st}}${{up}}</span>${{rs}}</div></article>`;
}}
async function tick(){{
  try {{
    const d = await (await fetch('/api/v1/fleet', {{cache:'no-store'}})).json();
    FL.innerHTML = d.radios.map(card).join('');
  }} catch (e) {{ /* keep the last render */ }}
}}
setInterval(tick, 3000);
</script>
</body></html>"##,
    ))
}
