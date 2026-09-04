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
}

impl AppState {
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
            server_id: Uuid::new_v4(),
            hostname: crate::net::hostname(),
            started: Instant::now(),
            ports,
            advertised_host,
            registry,
            sessions,
            radio,
            radio_mgr,
            webrtc,
            config,
        }))
    }

    pub fn uptime_s(&self) -> f64 {
        self.0.started.elapsed().as_secs_f64()
    }

    /// `["rx", "webrtc", "nbfm", "debug_tone"]` plus `"tx"` when the selected
    /// device can transmit.
    pub fn capabilities(&self) -> Vec<String> {
        let mut caps = crate::catalog::base_capabilities();
        if self.0.registry.selected().map(|d| d.tx_capable).unwrap_or(false) {
            caps.push("tx".to_string());
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
