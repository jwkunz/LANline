//! WebRTC media transport: one shared UDP-muxed port, one peer connection per
//! session, each fed Opus frames from the radio pipeline's broadcast fan-out.

use crate::model::{AudioConnState, AudioStateResponse};
use crate::radio::RadioManager;
use crate::sessions::SessionStore;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::ice::udp_mux::{UDPMuxDefault, UDPMuxParams};
use webrtc::ice::udp_network::UDPNetwork;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

pub struct WebrtcEngine {
    api: API,
    radio_mgr: Arc<RadioManager>,
    sessions: Arc<SessionStore>,
    peers: Mutex<HashMap<Uuid, Peer>>,
}

struct Peer {
    pc: Arc<RTCPeerConnection>,
    pump: Option<JoinHandle<()>>,
    rtcp: Option<JoinHandle<()>>,
    conn_state: Arc<Mutex<RTCPeerConnectionState>>,
    ice_state: Arc<Mutex<RTCIceConnectionState>>,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
}

impl WebrtcEngine {
    /// Binds the shared media UDP socket (the `audio_out` port; `requested_port`
    /// 0 lets the OS choose) and builds the WebRTC API (Opus, default
    /// interceptors, UDP mux). Returns the engine and the port actually bound,
    /// which is the authoritative value to advertise.
    pub async fn new(
        bind_ip: IpAddr,
        requested_port: u16,
        radio_mgr: Arc<RadioManager>,
        sessions: Arc<SessionStore>,
    ) -> Result<(Arc<Self>, u16)> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;

        let socket = tokio::net::UdpSocket::bind((bind_ip, requested_port))
            .await
            .with_context(|| format!("binding WebRTC media UDP {bind_ip}:{requested_port}"))?;
        let audio_out_port = socket.local_addr()?.port();
        let mux = UDPMuxDefault::new(UDPMuxParams::new(socket));
        let mut setting_engine = SettingEngine::default();
        setting_engine.set_udp_network(UDPNetwork::Muxed(mux));

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(setting_engine)
            .build();

        let engine = Arc::new(Self {
            api,
            radio_mgr,
            sessions,
            peers: Mutex::new(HashMap::new()),
        });
        Ok((engine, audio_out_port))
    }

    /// Handle a browser SDP offer for `session_id`: build a peer, attach the
    /// Opus track, answer with non-trickle ICE, and start pumping audio.
    /// Returns the answer SDP.
    pub async fn negotiate(self: &Arc<Self>, session_id: Uuid, offer_sdp: String) -> Result<String> {
        // Replace any existing peer for this session.
        self.close(session_id).await;

        let pc = Arc::new(
            self.api
                .new_peer_connection(RTCConfiguration::default())
                .await?,
        );

        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            "audio".to_owned(),
            "sdr-c2".to_owned(),
        ));
        let rtp_sender = pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        let conn_state = Arc::new(Mutex::new(RTCPeerConnectionState::New));
        let ice_state = Arc::new(Mutex::new(RTCIceConnectionState::New));
        let packets = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));

        // Drain sender RTCP so interceptors (NACK/TWCC/reports) run.
        let rtcp = tokio::spawn(async move {
            let mut buf = vec![0u8; 1500];
            while rtp_sender.read(&mut buf).await.is_ok() {}
        });

        {
            let sessions = self.sessions.clone();
            let cell = conn_state.clone();
            pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                *cell.lock().unwrap() = s;
                sessions.set_audio_state(session_id, map_conn(s));
                Box::pin(async {})
            }));
        }
        {
            let cell = ice_state.clone();
            pc.on_ice_connection_state_change(Box::new(move |s: RTCIceConnectionState| {
                *cell.lock().unwrap() = s;
                Box::pin(async {})
            }));
        }

        // Non-trickle ICE: answer only after gathering completes.
        let offer = RTCSessionDescription::offer(offer_sdp)?;
        pc.set_remote_description(offer).await?;
        let answer = pc.create_answer(None).await?;
        let mut gather_complete = pc.gathering_complete_promise().await;
        pc.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;
        let local = pc
            .local_description()
            .await
            .ok_or_else(|| anyhow!("no local description after ICE gathering"))?;

        // Pump encoded Opus frames from the pipeline into the track.
        let mut rx = self.radio_mgr.subscribe();
        let track_pump = Arc::clone(&track);
        let pkt_c = packets.clone();
        let byt_c = bytes.clone();
        let pump = tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(frame) => {
                        let n = frame.data.len() as u64;
                        let sample = Sample {
                            data: frame.data,
                            duration: frame.duration,
                            ..Default::default()
                        };
                        if track_pump.write_sample(&sample).await.is_err() {
                            break;
                        }
                        pkt_c.fetch_add(1, Ordering::Relaxed);
                        byt_c.fetch_add(n, Ordering::Relaxed);
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!("audio pump for {session_id} lagged {n} frames");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });

        self.peers.lock().unwrap().insert(
            session_id,
            Peer {
                pc,
                pump: Some(pump),
                rtcp: Some(rtcp),
                conn_state,
                ice_state,
                packets,
                bytes,
            },
        );
        self.sessions.set_audio_state(session_id, AudioConnState::Negotiating);

        tracing::info!("webrtc: answered offer for session {session_id}");
        Ok(local.sdp)
    }

    pub async fn close(&self, session_id: Uuid) {
        let peer = self.peers.lock().unwrap().remove(&session_id);
        if let Some(mut peer) = peer {
            if let Some(h) = peer.pump.take() {
                h.abort();
            }
            if let Some(h) = peer.rtcp.take() {
                h.abort();
            }
            let _ = peer.pc.close().await;
            self.sessions.set_audio_state(session_id, AudioConnState::Closed);
            tracing::info!("webrtc: closed peer for session {session_id}");
        }
    }

    pub fn snapshot(&self, session_id: Uuid) -> Option<AudioStateResponse> {
        let peers = self.peers.lock().unwrap();
        let peer = peers.get(&session_id)?;
        let conn = *peer.conn_state.lock().unwrap();
        let ice = *peer.ice_state.lock().unwrap();
        Some(AudioStateResponse {
            state: map_conn(conn),
            ice_state: ice_str(ice),
            dtls_state: conn_str(conn),
            packets_sent: peer.packets.load(Ordering::Relaxed),
            bytes_sent: peer.bytes.load(Ordering::Relaxed),
        })
    }

    pub fn peer_count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
}

fn map_conn(s: RTCPeerConnectionState) -> AudioConnState {
    match s {
        RTCPeerConnectionState::Connected => AudioConnState::Connected,
        RTCPeerConnectionState::Failed => AudioConnState::Failed,
        RTCPeerConnectionState::Closed | RTCPeerConnectionState::Disconnected => {
            AudioConnState::Closed
        }
        _ => AudioConnState::Negotiating,
    }
}

fn conn_str(s: RTCPeerConnectionState) -> &'static str {
    match s {
        RTCPeerConnectionState::Unspecified => "unspecified",
        RTCPeerConnectionState::New => "new",
        RTCPeerConnectionState::Connecting => "connecting",
        RTCPeerConnectionState::Connected => "connected",
        RTCPeerConnectionState::Disconnected => "disconnected",
        RTCPeerConnectionState::Failed => "failed",
        RTCPeerConnectionState::Closed => "closed",
    }
}

fn ice_str(s: RTCIceConnectionState) -> &'static str {
    match s {
        RTCIceConnectionState::Unspecified => "unspecified",
        RTCIceConnectionState::New => "new",
        RTCIceConnectionState::Checking => "checking",
        RTCIceConnectionState::Connected => "connected",
        RTCIceConnectionState::Completed => "completed",
        RTCIceConnectionState::Disconnected => "disconnected",
        RTCIceConnectionState::Failed => "failed",
        RTCIceConnectionState::Closed => "closed",
    }
}
