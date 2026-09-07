//! Command-line / environment configuration.

use clap::Parser;
use std::net::IpAddr;
use std::path::PathBuf;
use uuid::Uuid;

/// Runtime configuration for the LANline server.
#[derive(Debug, Clone, Parser)]
#[command(name = "lanline-server", version, about = "LANline — LAN-native SDR command & control")]
pub struct Config {
    /// Address for the REST/HTTP (C2) listener to bind.
    #[arg(long, env = "LANLINE_BIND", default_value = "0.0.0.0")]
    pub bind: IpAddr,

    /// C2 (REST + web client) port. Fixed by default so a bookmarked browser
    /// URL keeps working across restarts; pass `0` for a random free port.
    #[arg(long, env = "LANLINE_C2_PORT", default_value_t = 8730)]
    pub c2_port: u16,

    /// Audio-out (WebRTC media) port. 0 picks a random free port.
    #[arg(long, env = "LANLINE_AUDIO_OUT_PORT", default_value_t = 0)]
    pub audio_out_port: u16,

    /// Audio-in (reserved, phase 2) port. 0 picks a random free port.
    #[arg(long, env = "LANLINE_AUDIO_IN_PORT", default_value_t = 0)]
    pub audio_in_port: u16,

    /// UDP port for the discovery beacon.
    #[arg(long, env = "LANLINE_BEACON_PORT", default_value_t = 50055)]
    pub beacon_port: u16,

    /// Disable the discovery beacon entirely.
    #[arg(long, env = "LANLINE_NO_BEACON", default_value_t = false)]
    pub no_beacon: bool,

    /// Beacon interval in milliseconds.
    #[arg(long, env = "LANLINE_BEACON_INTERVAL_MS", default_value_t = 1000)]
    pub beacon_interval_ms: u64,

    /// Disable the multicast-DNS responder (`<name>.local` + `_lanline._tcp`).
    #[arg(long, env = "LANLINE_NO_MDNS", default_value_t = false)]
    pub no_mdns: bool,

    /// mDNS instance/host label: the server is reachable at `http://<name>.local:<c2-port>/`.
    #[arg(long, env = "LANLINE_MDNS_NAME", default_value = "lanline")]
    pub mdns_name: String,

    /// Optional human label for this radio instance (e.g. "VHF", "APRS"),
    /// surfaced in `GET /api/v1/server` and the discovery beacon. Set
    /// automatically by `lanline-hypervisor` for each child; unset for a
    /// plain standalone server.
    #[arg(long, env = "LANLINE_INSTANCE_LABEL")]
    pub instance_label: Option<String>,

    /// Stable server identity (UUID). Clients dedupe discovered servers on it.
    /// Default: a fresh random UUID each run. `lanline-hypervisor` pins one per
    /// child so a supervised restart keeps the same identity.
    #[arg(long, env = "LANLINE_SERVER_ID")]
    pub server_id: Option<Uuid>,

    /// URL of the `lanline-hypervisor` fleet page this radio belongs to, if
    /// any. Surfaced in `GET /api/v1/server` and the beacon; the web client
    /// shows a "back to the fleet" link when it is set. `lanline-hypervisor`
    /// passes it to each child.
    #[arg(long, env = "LANLINE_FLEET_URL")]
    pub fleet_url: Option<String>,

    /// Host/IP to advertise to clients. Defaults to the autodetected primary
    /// LAN IPv4 address.
    #[arg(long, env = "LANLINE_ADVERTISE_HOST")]
    pub advertise_host: Option<IpAddr>,

    /// SoapySDR device filter to select at startup, e.g. `driver=hackrf`.
    /// When unset the first enumerated device is selected.
    #[arg(long, env = "LANLINE_DEVICE")]
    pub device: Option<String>,

    /// Start in debug-tone mode (440 Hz A4, no SDR required).
    #[arg(long, env = "LANLINE_DEBUG_TONE", default_value_t = false)]
    pub debug_tone: bool,

    /// TCP port for the Beast binary Mode S feed (ADS-B). `0` disables it.
    #[arg(long, env = "LANLINE_BEAST_PORT", default_value_t = 30005)]
    pub beast_port: u16,

    /// TCP port for the AIVDM (NMEA 0183) marine AIS feed. `0` disables it.
    #[arg(long, env = "LANLINE_AIS_NMEA_PORT", default_value_t = 10110)]
    pub ais_nmea_port: u16,

    /// TCP port for the TNC2-format APRS monitor feed. `0` disables it.
    #[arg(long, env = "LANLINE_APRS_PORT", default_value_t = 10152)]
    pub aprs_port: u16,

    /// Also write the pre-Opus 48 kHz mono audio to this path as a 16-bit WAV
    /// (diagnostic; overwritten on each pipeline start).
    #[arg(long, env = "LANLINE_DUMP_WAV")]
    pub dump_wav: Option<PathBuf>,

    /// Directory for Receiver Analysis IQ `.wav` recordings. Default: the
    /// current working directory. Files can be large (~8 MB/s at 2 Msps).
    #[arg(long, env = "LANLINE_IQ_DIR")]
    pub iq_dir: Option<PathBuf>,

    /// This device's known crystal frequency error, in parts-per-million
    /// (applied to every tuned frequency on every mode). A HackRF's stock
    /// TCXO commonly drifts a few to a few tens of ppm; negligible at VHF,
    /// but enough at UHF (FRS's 462/467 MHz, say) to push a narrowband FM
    /// discriminator out of its linear range. Determine a device's ppm
    /// empirically (tune to a known-frequency signal and watch for a steady
    /// discriminator bias — see docs/architecture.md) rather than guessing;
    /// 0 (the default) applies no correction. Also settable live via
    /// `PATCH /radio`'s `tuner.freq_correction_ppm`.
    #[arg(long, env = "LANLINE_FREQ_CORRECTION_PPM", allow_hyphen_values = true, default_value_t = 0.0)]
    pub freq_correction_ppm: f64,

    /// Text-to-speech for the voice modes: `POST /radio/tx/say` synthesizes
    /// the text and transmits it. Needs the `tts` build feature and either
    /// `espeak-ng` (default) or `piper` (see `--tts-voice`) on `PATH`.
    #[arg(long, env = "LANLINE_TTS", default_value_t = false)]
    pub tts: bool,

    /// Path to a Piper voice model (`*.onnx`, with its `*.onnx.json` beside
    /// it). When set, `--tts` uses Piper's neural voice; otherwise espeak-ng.
    #[arg(long, env = "LANLINE_TTS_VOICE")]
    pub tts_voice: Option<PathBuf>,

    /// Speech-to-text on received transmissions: each over is transcribed and
    /// appended to `GET /radio/transcript`. Needs the `stt` build feature.
    #[arg(long, env = "LANLINE_STT", default_value_t = false)]
    pub stt: bool,

    /// Directory holding a Whisper model (`config.json`, `tokenizer.json`,
    /// `model.safetensors` — the HF layout, e.g. from `openai/whisper-base.en`).
    /// Required when `--stt` is set.
    #[arg(long, env = "LANLINE_STT_MODEL")]
    pub stt_model: Option<PathBuf>,

    /// Allow push-to-talk transmit (`POST /radio/tx/key`). Off by default —
    /// a general-purpose SDR keying a real transmitter is a materially
    /// bigger deal than one that only ever receives, so it needs explicit
    /// opt-in rather than working out of the box. See
    /// docs/architecture.md's PTT section for what this actually does (and
    /// doesn't yet) support, and the regulatory considerations for whatever
    /// frequencies you point it at.
    #[arg(long, env = "LANLINE_ENABLE_TX", default_value_t = false)]
    pub enable_tx: bool,

    /// Maximum number of concurrent sessions.
    #[arg(long, env = "LANLINE_MAX_SESSIONS", default_value_t = 8)]
    pub max_sessions: usize,

    /// Session heartbeat interval in seconds. The session TTL is 3x this.
    #[arg(long, env = "LANLINE_HEARTBEAT_S", default_value_t = 15)]
    pub heartbeat_s: u64,

    /// Comma-separated CORS allow-list for non-GET methods. `*` allows any
    /// origin (fine on a trusted LAN, the default for now).
    #[arg(long, env = "LANLINE_CORS_ORIGINS", default_value = "*")]
    pub cors_origins: String,

    /// Serve the C2 port over HTTPS. Browsers only allow `getUserMedia` (the
    /// push-to-talk mic) and the geolocation button on a "secure context" —
    /// `https://` or `http://localhost` — so reaching the server by its LAN
    /// address over plain HTTP, those features silently do nothing. With no
    /// `--tls-cert`/`--tls-key`, a self-signed cert is generated on first run
    /// and cached (see `--tls-dir`); browsers show a one-time "not private"
    /// warning to click through, after which the origin is a secure context.
    /// For no warning at all, issue a cert with `mkcert` and pass it below.
    #[arg(long, env = "LANLINE_TLS", default_value_t = false)]
    pub tls: bool,

    /// PEM certificate chain to serve (implies `--tls`). Pair with `--tls-key`.
    #[arg(long, env = "LANLINE_TLS_CERT")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key for `--tls-cert`.
    #[arg(long, env = "LANLINE_TLS_KEY")]
    pub tls_key: Option<PathBuf>,

    /// Directory for the auto-generated self-signed `cert.pem` / `key.pem`.
    /// Default: a per-user state directory. Delete it to force a fresh cert.
    #[arg(long, env = "LANLINE_TLS_DIR")]
    pub tls_dir: Option<PathBuf>,

    /// Look up tail number, type, owner and route for ADS-B contacts via
    /// adsbdb.com (`GET /api/v1/adsb/flight/{icao}`). On by default; this is
    /// the only feature that reaches the internet, and it tells adsbdb which
    /// aircraft your receiver is watching. Disable for airgapped use with
    /// `--flight-lookup false` or `LANLINE_FLIGHT_LOOKUP=0`.
    #[arg(long, env = "LANLINE_FLIGHT_LOOKUP", default_value_t = true,
          action = clap::ArgAction::Set)]
    pub flight_lookup: bool,

    /// Base URL of the adsbdb-compatible API used by `--flight-lookup`
    /// (override to point at a self-hosted mirror). No trailing slash.
    #[arg(long, env = "LANLINE_FLIGHT_LOOKUP_URL", default_value = "https://api.adsbdb.com/v0", hide = true)]
    pub flight_lookup_url: String,
}

impl Config {
    /// Session time-to-live derived from the heartbeat interval.
    pub fn session_ttl_s(&self) -> u64 {
        self.heartbeat_s.saturating_mul(3)
    }
}
