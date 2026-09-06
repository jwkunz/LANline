//! REST DTOs. These mirror `docs/rest-api.md` exactly; the web client's
//! `api.ts` types are the TypeScript counterpart.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn rfc3339_now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub uptime_s: f64,
    pub version: &'static str,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Ports {
    pub c2: u16,
    pub audio_out: u16,
    pub audio_in: u16,
    /// Beast binary Mode S feed (ADS-B). 0 when disabled.
    pub beast: u16,
    /// AIVDM (NMEA 0183) marine AIS feed. 0 when disabled.
    pub ais_nmea: u16,
}

#[derive(Serialize, Clone, Debug)]
pub struct SelectedDeviceSummary {
    pub id: String,
    pub driver: String,
    pub label: String,
    pub tx_capable: bool,
}

#[derive(Serialize)]
pub struct ServerInfo {
    pub server_id: Uuid,
    pub protocol_version: u32,
    pub version: &'static str,
    pub hostname: String,
    /// `"http"` or `"https"` — the transport this C2 port is served over.
    pub scheme: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    pub ports: Ports,
    pub capabilities: Vec<String>,
    pub selected_device: Option<SelectedDeviceSummary>,
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    /// 0 means continuous.
    pub step: f64,
}

impl Range {
    #[allow(dead_code)] // used once the DSP pipeline validates config (phase 1c)
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        Self { min, max, step }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct GainElement {
    pub name: String,
    pub range_db: Range,
}

#[derive(Serialize, Clone, Debug)]
pub struct SettingInfo {
    pub key: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub default: String,
    pub description: String,
    pub options: Option<Vec<String>>,
}

/// Variants beyond `Ready`/`Absent` are produced once the pipeline holds an
/// open device (phase 1c) but are part of the wire contract now.
#[allow(dead_code)]
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    Ready,
    Busy,
    Error,
    Absent,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceSummary {
    pub id: String,
    pub driver: String,
    pub label: String,
    pub serial: String,
    pub soapy_args: String,
    pub tx_capable: bool,
    pub available: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct RxCapabilities {
    pub channels: usize,
    pub antennas: Vec<String>,
    pub frequency_ranges_hz: Vec<Range>,
    pub sample_rate_ranges_hz: Vec<Range>,
    pub bandwidth_ranges_hz: Vec<Range>,
    pub gain_elements: Vec<GainElement>,
    pub overall_gain_range_db: Range,
    pub has_agc: bool,
    pub has_dc_offset_mode: bool,
    pub has_iq_balance_mode: bool,
    pub has_frequency_correction: bool,
    pub sensors: Vec<String>,
    pub setting_info: Vec<SettingInfo>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceInfo {
    pub id: String,
    pub driver: String,
    pub label: String,
    pub serial: String,
    pub soapy_args: String,
    pub status: DeviceStatus,
    pub tx_capable: bool,
    pub rx: RxCapabilities,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceHealth {
    pub present: bool,
    pub status: DeviceStatus,
    pub last_error: Option<String>,
    pub samples_read: u64,
    pub overruns: u64,
    pub sensors: BTreeMap<String, String>,
}

impl DeviceHealth {
    pub fn absent() -> Self {
        Self {
            present: false,
            status: DeviceStatus::Absent,
            last_error: None,
            samples_read: 0,
            overruns: 0,
            sensors: BTreeMap::new(),
        }
    }
}

/// `PUT /api/v1/device` body: exactly one of the fields.
#[derive(Deserialize, Debug)]
pub struct DeviceSelector {
    pub id: Option<String>,
    pub soapy_args: Option<String>,
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct ModeParamSpec {
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub default: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_: Option<Vec<f64>>,
    pub unit: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct ModeInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub tx_capable: bool,
    pub params: BTreeMap<&'static str, ModeParamSpec>,
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub builtin: bool,
    pub config: Value,
}

// ---------------------------------------------------------------------------
// Radio configuration
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TunerConfig {
    pub device_id: Option<String>,
    pub channel: usize,
    pub antenna: Option<String>,
    pub sample_rate_hz: f64,
    pub bandwidth_hz: Option<f64>,
    pub lo_offset_hz: f64,
    pub gain_mode: GainMode,
    pub gain_db: Option<f64>,
    pub gain_elements_db: BTreeMap<String, f64>,
    pub freq_correction_ppm: f64,
    pub dc_offset_correction: bool,
    pub iq_balance_correction: bool,
    pub device_settings: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GainMode {
    Manual,
    Agc,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioConfig {
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub opus_bitrate_bps: u32,
    pub frame_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { sample_rate_hz: 48_000, channels: 1, opus_bitrate_bps: 24_000, frame_ms: 20 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RadioConfig {
    pub enabled: bool,
    pub running: bool,
    pub mode: String,
    pub frequency_hz: u64,
    pub tuner: TunerConfig,
    pub mode_params: Value,
    pub audio: AudioConfig,
}

impl RadioConfig {
    /// The NOAA KHB29 default the server boots with.
    pub fn default_noaa() -> Self {
        Self {
            enabled: true,
            running: false,
            mode: "nbfm".to_string(),
            frequency_hz: 162_550_000,
            tuner: TunerConfig {
                device_id: None,
                channel: 0,
                antenna: None,
                // HackRF advertises integer-MHz rates; 2.0 Msps is its
                // supported floor. The DSP chain decimates + rationally
                // resamples from here to 48 kHz.
                sample_rate_hz: 2_000_000.0,
                bandwidth_hz: None,
                // Offset the LO to keep the NWR carrier off the ZIF DC spike;
                // the DSP chain mixes it back to baseband.
                lo_offset_hz: 250_000.0,
                gain_mode: GainMode::Manual,
                gain_db: None,
                gain_elements_db: BTreeMap::new(),
                freq_correction_ppm: 0.0,
                dc_offset_correction: true,
                iq_balance_correction: true,
                device_settings: BTreeMap::new(),
            },
            mode_params: crate::catalog::default_mode_params("nbfm"),
            audio: AudioConfig::default(),
        }
    }
}

/// Live pipeline telemetry (`GET /api/v1/radio/status`).
#[derive(Serialize, Clone, Debug)]
pub struct RadioStatus {
    pub running: bool,
    pub mode: String,
    pub frequency_hz: u64,
    pub device_status: DeviceStatus,
    pub dsp: DspStatus,
    pub audio: AudioStatus,
    pub recording: RecordingStatus,
    pub scan: ScanStatus,
    pub clients: usize,
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DspStatus {
    pub rssi_dbfs: Option<f64>,
    pub snr_db: Option<f64>,
    pub squelch_open: bool,
    pub audio_level_dbfs: Option<f64>,
    /// Configured CTCSS sub-audible tone (Hz) while it is currently detected
    /// on the channel; `null` when CTCSS is disabled or the tone isn't
    /// present. FM modes only.
    pub ctcss_tone_hz: Option<f64>,
    /// Strongest standard CTCSS tone the receiver sees on the channel (Hz),
    /// independent of `ctcss_hz` — for identifying an unknown repeater's
    /// tone. `null` when none stands out. Narrowband FM only.
    pub ctcss_scan_hz: Option<f64>,
    pub sample_overruns: u64,
    pub pipeline_latency_ms: Option<f64>,
    /// Currently transmitting (push-to-talk keyed).
    pub tx_keyed: bool,
}

/// A demod-audio recording (`POST /api/v1/radio/record`), in progress or
/// finished — the shape reported in `RadioStatus.recording.last` and by the
/// start/stop endpoint.
#[derive(Serialize, Clone, Debug)]
pub struct AudioRecInfo {
    pub path: String,
    pub filename: String,
    pub bytes: u64,
    pub secs: f64,
    pub sample_rate_hz: u32,
    pub mode: String,
    pub frequency_hz: u64,
}

/// `RadioStatus.recording` — whether a demod-audio capture is running, and
/// the most recent finished one.
#[derive(Serialize, Clone, Debug, Default)]
pub struct RecordingStatus {
    pub active: bool,
    pub last: Option<AudioRecInfo>,
}

/// `RadioStatus.scan` — channel-scan state. While `active`, `frequency_hz`
/// is the live tuned frequency (the top-level `RadioStatus.frequency_hz`
/// reflects the stored config and is stale during a scan).
#[derive(Serialize, Clone, Debug, Default)]
pub struct ScanStatus {
    pub active: bool,
    pub parked: bool,
    pub frequency_hz: Option<u64>,
    pub label: Option<String>,
    pub index: usize,
    pub total: usize,
}

/// One entry in the transmit audit log (`GET /api/v1/radio/tx/log`) — a
/// station log of every push-to-talk key, kept in memory (bounded ring).
#[derive(Serialize, Clone, Debug)]
pub struct TxLogEntry {
    #[serde(with = "time::serde::rfc3339")]
    pub keyed_at: OffsetDateTime,
    /// The keying client's declared name (`ClientInfo.name`).
    pub client: String,
    pub mode: String,
    /// Actual transmit frequency (RX frequency + `offset_hz`).
    pub tx_frequency_hz: u64,
    pub offset_hz: i64,
    /// Encoded CTCSS uplink tone (Hz), 0 = none.
    pub tone_hz: f64,
    pub gain_db: f64,
    /// `null` if the transmission was still open / auto-released at the
    /// server cap when the log was read (no explicit `unkey` recorded).
    #[serde(with = "time::serde::rfc3339::option")]
    pub released_at: Option<OffsetDateTime>,
    pub duration_ms: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct AudioStatus {
    pub encoder: &'static str,
    pub bitrate_bps: u32,
    pub frames_sent: u64,
    pub sample_rate_hz: u32,
    pub channels: u8,
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub client: ClientInfo,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ClientInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Serialize)]
pub struct CreateSessionResponse {
    pub session_id: Uuid,
    pub token: String,
    pub heartbeat_interval_s: u64,
    pub expires_in_s: u64,
}

#[derive(Serialize)]
pub struct SessionSummary {
    pub session_id: Uuid,
    pub client: ClientInfo,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires: OffsetDateTime,
    pub audio: SessionAudio,
}

#[derive(Serialize)]
pub struct SessionAudio {
    pub state: AudioConnState,
}

#[derive(Serialize)]
pub struct HeartbeatResponse {
    pub expires_in_s: u64,
}

// ---------------------------------------------------------------------------
// Audio / WebRTC signaling
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct SdpMessage {
    pub sdp: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Deserialize)]
pub struct IceMessage {
    #[allow(dead_code)]
    pub candidate: Value,
}

/// Most transitions are driven by the WebRTC peer (phase 1b); the full set is
/// part of the wire contract now.
#[allow(dead_code)]
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioConnState {
    Idle,
    Negotiating,
    Connected,
    Failed,
    Closed,
}

#[derive(Serialize)]
pub struct AudioStateResponse {
    pub state: AudioConnState,
    pub ice_state: &'static str,
    pub dtls_state: &'static str,
    pub packets_sent: u64,
    pub bytes_sent: u64,
}

/// Convenience: the `time` field many status payloads carry.
pub fn now_utc() -> OffsetDateTime {
    rfc3339_now()
}
