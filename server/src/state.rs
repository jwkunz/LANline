//! Shared application state handed to every axum handler.

use crate::config::Config;
use crate::media::WebrtcEngine;
use crate::model::{Ports, RadioConfig};
use crate::radio::RadioManager;
use crate::registry::DeviceRegistry;
use crate::sessions::SessionStore;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub server_id: Uuid,
    pub hostname: String,
    pub started: Instant,
    pub ports: Ports,
    pub advertised_host: IpAddr,
    pub registry: Arc<DeviceRegistry>,
    pub sessions: Arc<SessionStore>,
    pub radio: Arc<Mutex<RadioConfig>>,
    pub radio_mgr: Arc<RadioManager>,
    pub webrtc: Arc<WebrtcEngine>,
    /// ADS-B flight enrichment via adsbdb.com (the one internet-facing
    /// feature). Built from `config`; disabled when `--flight-lookup false`.
    pub flight: Arc<crate::flight::FlightLookup>,
    /// AIS vessel enrichment via vesselfinder.com — same `--flight-lookup`
    /// toggle as `flight`.
    pub vessel: Arc<crate::vessel::VesselLookup>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)] // wiring constructor — a params struct would just move the noise
    pub fn new(
        config: Config,
        ports: Ports,
        advertised_host: IpAddr,
        registry: Arc<DeviceRegistry>,
        sessions: Arc<SessionStore>,
        radio: Arc<Mutex<RadioConfig>>,
        radio_mgr: Arc<RadioManager>,
        webrtc: Arc<WebrtcEngine>,
    ) -> Self {
        AppState(Arc::new(Inner {
            server_id: config.server_id.unwrap_or_else(Uuid::new_v4),
            hostname: crate::net::hostname(),
            started: Instant::now(),
            ports,
            advertised_host,
            registry,
            sessions,
            radio,
            radio_mgr,
            webrtc,
            flight: Arc::new(crate::flight::FlightLookup::new(&config)),
            vessel: Arc::new(crate::vessel::VesselLookup::new(&config)),
            config,
        }))
    }

    pub fn uptime_s(&self) -> f64 {
        self.0.started.elapsed().as_secs_f64()
    }

    /// `"https"` when the C2 port is served over TLS (`--tls`), else `"http"`.
    /// Used to build the URLs advertised in the beacon and mDNS.
    pub fn scheme(&self) -> &'static str {
        crate::tls::scheme(&self.0.config)
    }

    /// `["rx", "webrtc", "nbfm", "debug_tone"]` plus `"tx"` when the selected
    /// device can transmit, plus `"ptt"` when transmit is *also* actually
    /// enabled (`--enable-tx`) — a client uses this, not `"tx"` alone, to
    /// decide whether to show a push-to-talk control at all.
    pub fn capabilities(&self) -> Vec<String> {
        let mut caps = crate::catalog::base_capabilities();
        let device_tx_capable = self.0.registry.selected().map(|d| d.tx_capable).unwrap_or(false);
        if device_tx_capable {
            caps.push("tx".to_string());
        }
        if device_tx_capable && self.0.radio_mgr.tx_enabled() {
            caps.push("ptt".to_string());
        }
        if self.0.radio_mgr.tts_enabled() {
            caps.push("tts".to_string());
        }
        if self.0.radio_mgr.stt_enabled() {
            caps.push("stt".to_string());
        }
        if self.0.flight.is_enabled() {
            caps.push("flight-lookup".to_string());
        }
        if self.0.vessel.is_enabled() {
            caps.push("vessel-lookup".to_string());
        }
        caps
    }
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}
