//! SDR command & control + streaming server.
//!
//! Phase 1b: REST API + discovery beacon + device enumeration + a synthesized
//! audio pipeline (440 Hz `debug_tone` / silence) → Opus → per-session WebRTC.
//! The real SoapySDR DSP source replaces the synth in phase 1c.

mod adsb;
mod api;
mod audio;
mod catalog;
mod config;
mod discovery;
mod error;
mod mdns;
mod media;
mod model;
mod net;
mod radio;
mod registry;
mod sessions;
mod state;
mod util;

use anyhow::Result;
use clap::Parser;
use config::Config;
use model::{Ports, RadioConfig};
use state::AppState;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,lanline_server=debug".into()),
        )
        .init();

    let config = Config::parse();

    // --- ports -----------------------------------------------------------
    let (tcp, c2_port) = net::bind_tcp(config.bind, config.c2_port)?;

    let advertised_host = config.advertise_host.unwrap_or_else(|| {
        net::primary_ipv4()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
    });

    // --- device --------------------------------------------------------
    let registry = Arc::new(registry::DeviceRegistry::new());
    registry.select_auto(config.device.as_deref());

    // --- radio config + pipeline -------------------------------------
    let radio_cfg = Arc::new(Mutex::new(RadioConfig::default_noaa()));
    {
        let mut radio = radio_cfg.lock().unwrap();
        if config.debug_tone {
            radio.mode = "debug_tone".to_string();
            radio.mode_params = catalog::default_mode_params("debug_tone");
            tracing::info!("starting in debug-tone mode (no SDR required)");
        }
        if let Some(dev) = registry.selected() {
            radio.tuner.device_id = Some(dev.id);
        }
    }
    let radio_mgr =
        radio::RadioManager::new(radio_cfg.clone(), registry.clone(), config.dump_wav.clone());

    // --- sessions + media ------------------------------------------
    let sessions = Arc::new(sessions::SessionStore::new(
        config.heartbeat_s,
        config.session_ttl_s(),
        config.max_sessions,
    ));
    let (webrtc, audio_out) = media::WebrtcEngine::new(
        config.bind,
        config.audio_out_port,
        advertised_host,
        radio_mgr.clone(),
        sessions.clone(),
    )
    .await?;
    let audio_in = net::pick_udp_port(config.bind, config.audio_in_port)?;
    let beast = adsb::beast::serve(config.bind, config.beast_port, radio_mgr.adsb().beast_tx.clone());
    let ports = Ports { c2: c2_port, audio_out, audio_in, beast };

    let state = AppState::new(
        config.clone(),
        ports,
        advertised_host,
        registry.clone(),
        sessions.clone(),
        radio_cfg,
        radio_mgr.clone(),
        webrtc.clone(),
    );

    if config.debug_tone {
        radio_mgr.start();
        state.radio.lock().unwrap().running = true;
    }

    // --- background tasks -------------------------------------------
    {
        let sessions = sessions.clone();
        let webrtc = webrtc.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                for id in sessions.reap() {
                    webrtc.close(id).await;
                    tracing::debug!("reaped session {id}");
                }
            }
        });
    }

    if !config.no_beacon {
        let beacon = discovery::Beacon::new(state.clone());
        tokio::spawn(beacon.run());
    }

    // mDNS: a browser can't hear the UDP beacon, so also advertise a
    // `<name>.local` address on the C2 port. Held until shutdown.
    let _mdns = match (config.no_mdns, advertised_host) {
        (false, IpAddr::V4(ip)) if !ip.is_loopback() => {
            mdns::spawn(&config.mdns_name, ip, ports.c2, state.server_id)
        }
        _ => None,
    };

    // --- serve -------------------------------------------------------
    let listener = tokio::net::TcpListener::from_std(tcp)?;
    tracing::info!(
        "listening: C2 http://{host}:{c2}  audio_out udp/{ao}  audio_in udp/{ai}  beast tcp/{be}  beacon udp/{bp}",
        host = advertised_host,
        c2 = ports.c2,
        ao = ports.audio_out,
        ai = ports.audio_in,
        be = ports.beast,
        bp = config.beacon_port,
    );
    let name_url = match (config.no_mdns, advertised_host) {
        (false, IpAddr::V4(ip)) if !ip.is_loopback() => {
            format!("  or  http://{}.local:{}/", config.mdns_name, ports.c2)
        }
        _ => String::new(),
    };
    tracing::info!(
        "web client: open http://{}:{}/{name_url}  — connects with nothing to type",
        advertised_host,
        ports.c2,
    );
    if !api::webui::bundled() {
        tracing::warn!(
            "web client: no bundle embedded — run `npm --prefix web run build` and rebuild"
        );
    }

    let app = api::router(state.clone());
    let shutdown = {
        let state = state.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown: stopping pipeline and closing peers");
            state.webrtc.close_all().await;
            state.radio_mgr.stop();
        }
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
