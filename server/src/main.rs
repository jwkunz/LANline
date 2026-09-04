//! SDR command & control + streaming server.
//!
//! Phase 1b: REST API + discovery beacon + device enumeration + a synthesized
//! audio pipeline (440 Hz `debug_tone` / silence) → Opus → per-session WebRTC.
//! The real SoapySDR DSP source replaces the synth in phase 1c.

mod api;
mod audio;
mod catalog;
mod config;
mod discovery;
mod error;
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
                .unwrap_or_else(|_| "info,sdr_c2_server=debug".into()),
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
    let (webrtc, audio_out) =
        media::WebrtcEngine::new(config.bind, config.audio_out_port, radio_mgr.clone(), sessions.clone())
            .await?;
    let audio_in = net::pick_udp_port(config.bind, config.audio_in_port)?;
    let ports = Ports { c2: c2_port, audio_out, audio_in };

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

    // --- serve -------------------------------------------------------
    let listener = tokio::net::TcpListener::from_std(tcp)?;
    tracing::info!(
        "listening: C2 http://{host}:{c2}  audio_out udp/{ao}  audio_in udp/{ai}  beacon udp/{bp}",
        host = advertised_host,
        c2 = ports.c2,
        ao = ports.audio_out,
        ai = ports.audio_in,
        bp = config.beacon_port,
    );
    tracing::info!("web client: enter server host `{advertised_host}` (port {})", ports.c2);

    let app = api::router(state);
    axum::serve(listener, app).await?;
    Ok(())
}
