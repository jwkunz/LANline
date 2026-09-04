//! UDP discovery beacon. See `docs/beacon-protocol.md`.

use crate::model::{PROTOCOL_VERSION, SERVER_VERSION};
use crate::state::AppState;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use tokio::net::UdpSocket;

pub struct Beacon {
    state: AppState,
}

impl Beacon {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    pub async fn run(self) {
        let sock = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("beacon: bind failed: {e}");
                return;
            }
        };
        if let Err(e) = sock.set_broadcast(true) {
            tracing::error!("beacon: could not enable SO_BROADCAST: {e}");
            return;
        }

        let port = self.state.config.beacon_port;
        let period = Duration::from_millis(self.state.config.beacon_interval_ms.max(100));
        let mut tick = tokio::time::interval(period);
        tracing::info!("beacon: udp/{port} every {} ms", period.as_millis());

        // Enumerating SoapySDR is not free; refresh the count every ~30 s.
        let mut devices_available = self.state.registry.enumerate().len();
        let mut ticks: u64 = 0;

        loop {
            tick.tick().await;
            ticks += 1;
            if ticks % 30 == 0 {
                devices_available = self.state.registry.enumerate().len();
            }

            let targets = crate::net::broadcast_targets();
            if targets.is_empty() {
                let bytes = self.payload(self.state.advertised_host, devices_available);
                let _ = sock
                    .send_to(&bytes, SocketAddrV4::new(Ipv4Addr::BROADCAST, port))
                    .await;
            } else {
                for (iface_ip, bcast) in targets {
                    let bytes = self.payload(IpAddr::V4(iface_ip), devices_available);
                    let _ = sock.send_to(&bytes, SocketAddrV4::new(bcast, port)).await;
                }
                // Also hit the global broadcast address once.
                let bytes = self.payload(self.state.advertised_host, devices_available);
                let _ = sock
                    .send_to(&bytes, SocketAddrV4::new(Ipv4Addr::BROADCAST, port))
                    .await;
            }
        }
    }

    fn payload(&self, host: IpAddr, devices_available: usize) -> Vec<u8> {
        let ports = self.state.ports;
        let device = self.state.registry.selected().map(|d| {
            json!({
                "driver": d.driver,
                "label": d.label,
                "serial": d.serial,
                "tx_capable": d.tx_capable,
            })
        });

        let obj = json!({
            "magic": "LANLINE-BEACON",
            "protocol_version": PROTOCOL_VERSION,
            "server_id": self.state.server_id,
            "version": SERVER_VERSION,
            "hostname": self.state.hostname,
            "advertised_host": host.to_string(),
            "ports": {
                "c2": ports.c2,
                "audio_out": ports.audio_out,
                "audio_in": ports.audio_in,
                "beast": ports.beast,
                "ais_nmea": ports.ais_nmea,
            },
            "c2_base_url": format!("http://{host}:{}", ports.c2),
            "device": device,
            "devices_available": devices_available,
            "capabilities": self.state.capabilities(),
            "timestamp": time::OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default(),
        });
        serde_json::to_vec(&obj).unwrap_or_default()
    }
}
