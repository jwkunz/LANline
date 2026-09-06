//! The receive pipeline and its lifecycle.
//!
//! `debug_tone` mode synthesizes a 440 Hz A4; `nbfm`/`wbfm`/`am` modes open the
//! selected SoapySDR device and run one of the [`dsp`] analog chains (FM or
//! AM, picked per mode — see `Chain`); `adsb`/`ais` run their own decoders
//! with no audio output; any other mode emits digital silence. The analog
//! modes produce 20 ms Opus frames on a broadcast channel that each WebRTC
//! session subscribes to.

pub mod audio_rec;
#[cfg(feature = "soapy")]
pub mod dsp;

use crate::adsb::AdsbShared;
use crate::ais::AisShared;
use crate::analysis::AnalysisShared;
use crate::apt::AptShared;
use crate::audio::{rms_dbfs, AudioFrame};
use crate::model::{RadioConfig, TxLogEntry};
use crate::radio::audio_rec::AudioRecorder;
use crate::registry::DeviceRegistry;
use std::collections::VecDeque;
use time::OffsetDateTime;
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use bytes::Bytes;
#[cfg(feature = "soapy")]
use dsp::{AmChain, AmParams, FmChain, FmParams, TxModulator};
use crate::audio::wav::WavDump;
#[cfg(feature = "soapy")]
use crate::model::GainMode;
use std::f64::consts::TAU;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "soapy")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 64;
const OPUS_MAX_FRAME: usize = 4000;
const AUDIO_RATE: u32 = 48_000;

#[derive(Default, Clone)]
pub struct Telemetry {
    pub frames_sent: u64,
    pub audio_level_dbfs: Option<f32>,
    pub rssi_dbfs: Option<f32>,
    pub snr_db: Option<f32>,
    pub squelch_open: bool,
    /// Configured CTCSS tone (Hz) while currently detected present; `None`
    /// when CTCSS is off or the tone isn't locked.
    pub ctcss_tone_hz: Option<f32>,
    /// Strongest standard CTCSS tone seen on the channel (Hz), whatever is
    /// configured; `None` when none stands out. Narrowband FM only.
    pub ctcss_scan_hz: Option<f32>,
    /// Channel-scan state (`run_sdr` only). `scan_freq_hz` is the live tuned
    /// frequency while a scan is running (the config frequency is stale then).
    pub scanning: bool,
    pub scan_parked: bool,
    pub scan_freq_hz: Option<f64>,
    pub scan_label: Option<String>,
    pub scan_index: usize,
    pub scan_total: usize,
    pub overruns: u64,
    pub source: &'static str,
    /// Currently transmitting (push-to-talk keyed). See `PipelineCmd::Key`.
    pub tx_keyed: bool,
}

/// Hard ceiling on one PTT key, regardless of what the client asks for — an
/// auto-unkey safety net against a lost `unkey` request (a dropped
/// connection, a crashed client) leaving the transmitter keyed indefinitely.
#[cfg(feature = "soapy")]
const MAX_TX_SECS: f64 = 10.0;

/// One push-to-talk transmission: live mic audio (see `TxAudioSource`),
/// frequency-modulated via `TxModulator`. `gain_db` is a single overall TX
/// gain (HackRF's TX chain has VGA + AMP elements; SoapySDR's aggregate
/// `setGain` distributes across them).
#[cfg(feature = "soapy")]
#[derive(Clone, Copy)]
struct TxKeySpec {
    deviation_hz: f64,
    /// Linear gain on decoded mic PCM before modulation; see `frs`'s
    /// `tx_mic_gain` mode param in `catalog.rs`.
    mic_gain: f64,
    gain_db: f64,
    max_secs: f64,
    /// Transmit-frequency offset from the current RX frequency, Hz (repeater
    /// split; 0 for simplex). Applied as a plain LO retune for the key.
    offset_hz: f64,
    /// CTCSS uplink tone to encode onto the transmission, Hz (0 = none) —
    /// what a repeater needs to hear to key up.
    tone_hz: f64,
}

/// Bridges live mic audio from the (async, tokio) WebRTC receive task to the
/// (blocking) SDR pipeline thread's TX-key loop. A single global buffer, not
/// per-session — only one physical radio exists, so only one transmission
/// can be live at a time regardless of how many sessions are connected.
pub struct TxAudioSource {
    buf: Mutex<std::collections::VecDeque<f32>>,
}

impl TxAudioSource {
    fn new() -> Arc<Self> {
        Arc::new(Self { buf: Mutex::new(std::collections::VecDeque::new()) })
    }

    /// Called from the WebRTC mic-decode task as frames arrive. Caps the
    /// buffer (~2s of 48kHz mono audio) so a mic feed that outpaces TX
    /// draining it — or one that's simply never keyed — can't grow forever.
    pub fn push(&self, samples: &[f32]) {
        let mut b = self.buf.lock().unwrap();
        b.extend(samples.iter().copied());
        let cap = 48_000 * 2;
        while b.len() > cap {
            b.pop_front();
        }
    }

    /// Called from the TX-key loop. Pops up to `n` samples, oldest first;
    /// pads with silence on underrun so the loop's pacing (set by the SDR's
    /// own TX buffer backpressure in `write()`) never has to block waiting
    /// on network/mic jitter — a brief silent patch, not a stall.
    #[cfg(feature = "soapy")]
    fn pull(&self, n: usize, out: &mut Vec<f32>) {
        out.clear();
        let mut b = self.buf.lock().unwrap();
        for _ in 0..n {
            out.push(b.pop_front().unwrap_or(0.0));
        }
    }

    /// Drop anything buffered — called at key-up, so a stale tail (silence
    /// that piled up between transmissions, since nothing was draining it)
    /// doesn't play back at the start of the next one.
    #[cfg(feature = "soapy")]
    fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }
}

pub struct RadioManager {
    cfg: Arc<Mutex<RadioConfig>>,
    registry: Arc<DeviceRegistry>,
    dump_wav: Option<PathBuf>,
    tx: broadcast::Sender<AudioFrame>,
    run: Mutex<RunState>,
    telemetry: Arc<Mutex<Telemetry>>,
    adsb: Arc<AdsbShared>,
    ais: Arc<AisShared>,
    apt: Arc<AptShared>,
    analysis: Arc<AnalysisShared>,
    audio_rec: Arc<AudioRecorder>,
    /// Off by default (`--enable-tx`) — a general-purpose SDR transmitting
    /// is a much bigger deal than one only ever receiving, so it needs an
    /// explicit opt-in rather than working out of the box.
    enable_tx: bool,
    audio_in: Arc<TxAudioSource>,
    /// In-memory station log of push-to-talk keys, served by
    /// `GET /api/v1/radio/tx/log`.
    tx_audit: TxAudit,
}

/// How many transmit-log entries to keep.
const TX_AUDIT_CAP: usize = 200;

/// Bounded in-memory ring of push-to-talk keyings — an amateur-station-log
/// -adjacent record of what this server transmitted, when, and for how long.
#[derive(Default)]
struct TxAudit(Mutex<VecDeque<TxLogEntry>>);

impl TxAudit {
    #[allow(clippy::too_many_arguments)]
    fn key(
        &self,
        client: String,
        mode: String,
        tx_frequency_hz: u64,
        offset_hz: i64,
        tone_hz: f64,
        gain_db: f64,
    ) {
        let mut log = self.0.lock().unwrap();
        if log.len() >= TX_AUDIT_CAP {
            log.pop_front();
        }
        log.push_back(TxLogEntry {
            keyed_at: OffsetDateTime::now_utc(),
            client,
            mode,
            tx_frequency_hz,
            offset_hz,
            tone_hz,
            gain_db,
            released_at: None,
            duration_ms: None,
        });
    }

    /// Close out this client's most recent still-open entry.
    fn release(&self, client: &str) {
        let now = OffsetDateTime::now_utc();
        let mut log = self.0.lock().unwrap();
        if let Some(e) = log.iter_mut().rev().find(|e| e.client == client && e.released_at.is_none())
        {
            e.released_at = Some(now);
            e.duration_ms = Some(((now - e.keyed_at).whole_milliseconds().max(0)) as u64);
        }
    }

    fn snapshot(&self) -> Vec<TxLogEntry> {
        self.0.lock().unwrap().iter().rev().cloned().collect()
    }
}

#[derive(Default)]
struct RunState {
    running: bool,
    stop: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
    #[cfg(feature = "soapy")]
    cmd_tx: Option<mpsc::Sender<PipelineCmd>>,
}

/// Changes applied to a running SDR pipeline without restarting the stream.
#[cfg(feature = "soapy")]
enum PipelineCmd {
    Retune(f64),
    Gain { agc: bool, overall: Option<f64>, elements: Vec<(String, f64)> },
    Demod(DemodParams),
    /// Begin a push-to-talk transmission (`run_sdr` only — see its handling
    /// for the RX/TX device hand-off; other pipelines ignore this).
    Key(TxKeySpec),
    /// End the current transmission early (before `TxKeySpec::max_secs`).
    Unkey,
    /// Start (`Some`) or stop (`None`) a channel scan (`run_sdr` only). A
    /// manual `Retune` while scanning also stops it.
    Scan(Option<ScanConfig>),
}

/// One channel in a scan list.
#[cfg(feature = "soapy")]
#[derive(Clone)]
struct ScanChan {
    freq_hz: f64,
    label: String,
}

/// A running channel scan: sweep `chans`, dwelling `dwell` on each, and park
/// (let audio flow, stop advancing) on any channel whose squelch opens above
/// `rssi_gate_dbfs`; resume sweeping `hang` after the signal drops.
#[cfg(feature = "soapy")]
#[derive(Clone)]
struct ScanConfig {
    chans: Vec<ScanChan>,
    dwell: Duration,
    hang: Duration,
    rssi_gate_dbfs: f32,
}

/// Live state of an in-progress scan inside `run_sdr`.
#[cfg(feature = "soapy")]
struct ScanRun {
    cfg: ScanConfig,
    idx: usize,
    dwell_start: Instant,
    parked: bool,
    /// When the parked signal dropped — resume sweeping once `hang` elapses.
    lost_at: Option<Instant>,
}

/// Shared LO-retune sequence for a running `run_sdr`: move the hardware,
/// track the live frequency + LO, and drop discriminator/CTCSS memory so the
/// step doesn't click.
#[cfg(feature = "soapy")]
#[allow(clippy::too_many_arguments)]
fn sdr_retune(
    dev: &soapysdr::Device,
    dir: soapysdr::Direction,
    ch: usize,
    sp: &SdrParams,
    log_set: &dyn Fn(&str, Result<(), soapysdr::Error>),
    hz: f64,
    chain: &mut Chain,
    cur_freq_hz: &mut f64,
    lo: &mut f64,
) {
    *cur_freq_hz = hz;
    *lo = hz - sp.demod.lo_offset_hz();
    log_set("frequency", dev.set_frequency(dir, ch, apply_ppm(*lo, sp.freq_correction_ppm), ""));
    chain.on_retune();
}

/// Which analog demodulator `run_sdr` builds. `nbfm`/`wbfm` share the FM
/// chain (only the constants differ); `am` is a plain envelope detector.
#[cfg(feature = "soapy")]
#[derive(Clone, Copy)]
enum DemodParams {
    Fm(FmParams),
    Am(AmParams),
}

#[cfg(feature = "soapy")]
impl DemodParams {
    fn lo_offset_hz(&self) -> f64 {
        match self {
            DemodParams::Fm(p) => p.lo_offset_hz,
            DemodParams::Am(p) => p.lo_offset_hz,
        }
    }
    fn label(&self) -> &'static str {
        match self {
            DemodParams::Fm(_) => "fm",
            DemodParams::Am(_) => "am",
        }
    }
}

/// The two demod chains behind one `run_sdr` loop.
#[cfg(feature = "soapy")]
enum Chain {
    // Both boxed: `FmChain` grew (the optional CTCSS detector is several
    // biquads) and the two variants are large and lopsided — keep the enum
    // itself pointer-sized.
    Fm(Box<FmChain>),
    Am(Box<AmChain>),
}

#[cfg(feature = "soapy")]
impl Chain {
    fn new(device_rate: f64, demod: DemodParams) -> Self {
        match demod {
            DemodParams::Fm(p) => Chain::Fm(Box::new(FmChain::new(device_rate, p))),
            DemodParams::Am(p) => Chain::Am(Box::new(AmChain::new(device_rate, p))),
        }
    }
    fn channel_rate(&self) -> f64 {
        match self {
            Chain::Fm(c) => c.channel_rate(),
            Chain::Am(c) => c.channel_rate(),
        }
    }
    fn metrics(&self) -> dsp::ChainMetrics {
        match self {
            Chain::Fm(c) => c.metrics(),
            Chain::Am(c) => c.metrics(),
        }
    }
    fn on_retune(&mut self) {
        match self {
            Chain::Fm(c) => c.on_retune(),
            Chain::Am(c) => c.on_retune(),
        }
    }
    fn process(&mut self, iq: &[num_complex::Complex32], out: &mut Vec<f32>) {
        match self {
            Chain::Fm(c) => c.process(iq, out),
            Chain::Am(c) => c.process(iq, out),
        }
    }
}

impl RadioManager {
    pub fn new(
        cfg: Arc<Mutex<RadioConfig>>,
        registry: Arc<DeviceRegistry>,
        dump_wav: Option<PathBuf>,
        iq_dir: std::path::PathBuf,
        enable_tx: bool,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            cfg,
            registry,
            dump_wav,
            tx,
            run: Mutex::new(RunState::default()),
            telemetry: Arc::new(Mutex::new(Telemetry::default())),
            adsb: Arc::new(AdsbShared::new()),
            ais: Arc::new(AisShared::new()),
            apt: Arc::new(AptShared::new()),
            analysis: Arc::new(AnalysisShared::new(iq_dir.clone())),
            audio_rec: Arc::new(AudioRecorder::new(iq_dir)),
            enable_tx,
            audio_in: TxAudioSource::new(),
            tx_audit: TxAudit::default(),
        })
    }

    pub fn tx_enabled(&self) -> bool {
        self.enable_tx
    }

    /// Start a channel scan on the running SDR pipeline. `chans` is
    /// `(frequency_hz, label)` pairs; the pipeline sweeps them, dwelling
    /// `dwell_ms` on each, and parks on any whose squelch opens above
    /// `rssi_gate_dbfs`, resuming `hang_ms` after the signal drops.
    #[cfg(feature = "soapy")]
    pub fn scan_start(
        &self,
        chans: Vec<(f64, String)>,
        dwell_ms: u64,
        hang_ms: u64,
        rssi_gate_dbfs: f32,
    ) -> Result<(), &'static str> {
        if chans.is_empty() {
            return Err("scan list is empty");
        }
        let cfg = ScanConfig {
            chans: chans
                .into_iter()
                .map(|(freq_hz, label)| ScanChan { freq_hz, label })
                .collect(),
            dwell: Duration::from_millis(dwell_ms.clamp(30, 5_000)),
            hang: Duration::from_millis(hang_ms.clamp(0, 30_000)),
            rssi_gate_dbfs,
        };
        let run = self.run.lock().unwrap();
        let cmd_tx = run.cmd_tx.as_ref().ok_or("radio is not running")?;
        cmd_tx
            .send(PipelineCmd::Scan(Some(cfg)))
            .map_err(|_| "pipeline is not accepting commands")
    }

    #[cfg(not(feature = "soapy"))]
    pub fn scan_start(
        &self,
        _chans: Vec<(f64, String)>,
        _dwell_ms: u64,
        _hang_ms: u64,
        _rssi_gate_dbfs: f32,
    ) -> Result<(), &'static str> {
        Err("built without SDR support")
    }

    /// Stop a running scan, leaving RX wherever the scan currently sits.
    pub fn scan_stop(&self) {
        #[cfg(feature = "soapy")]
        if let Some(tx) = self.run.lock().unwrap().cmd_tx.as_ref() {
            let _ = tx.send(PipelineCmd::Scan(None));
        }
    }

    /// Append a "keyed" entry to the transmit audit log.
    #[allow(clippy::too_many_arguments)]
    pub fn tx_log_key(
        &self,
        client: String,
        mode: String,
        tx_frequency_hz: u64,
        offset_hz: i64,
        tone_hz: f64,
        gain_db: f64,
    ) {
        self.tx_audit.key(client, mode, tx_frequency_hz, offset_hz, tone_hz, gain_db);
    }

    /// Close out this client's most recent still-open transmit-log entry.
    pub fn tx_log_release(&self, client: &str) {
        self.tx_audit.release(client);
    }

    /// The transmit audit log, newest first.
    pub fn tx_log(&self) -> Vec<TxLogEntry> {
        self.tx_audit.snapshot()
    }

    /// The mic-audio bridge — pushed into by the WebRTC layer as it decodes
    /// an incoming Opus track, pulled from by a running TX key. Public so
    /// `media::WebrtcEngine` (which only holds `Arc<RadioManager>`) can
    /// reach it without another constructor parameter threaded everywhere.
    pub fn audio_in(&self) -> Arc<TxAudioSource> {
        self.audio_in.clone()
    }

    /// Begin a push-to-talk transmission on the currently-running pipeline,
    /// keying live mic audio pushed into `audio_in()` (silence if none has
    /// arrived — see `TxAudioSource::pull`). Only meaningful in `frs` mode;
    /// the caller (the API handler) is responsible for checking that before
    /// calling this.
    #[cfg(feature = "soapy")]
    pub fn key(
        &self,
        gain_db: f64,
        deviation_hz: f64,
        mic_gain: f64,
        offset_hz: f64,
        tone_hz: f64,
    ) -> Result<(), &'static str> {
        if !self.enable_tx {
            return Err("transmit is disabled on this server — see --enable-tx");
        }
        self.audio_in.clear();
        let run = self.run.lock().unwrap();
        let cmd_tx = run.cmd_tx.as_ref().ok_or("radio is not running")?;
        cmd_tx
            .send(PipelineCmd::Key(TxKeySpec {
                deviation_hz,
                mic_gain,
                gain_db,
                max_secs: MAX_TX_SECS,
                offset_hz,
                tone_hz,
            }))
            .map_err(|_| "pipeline is not accepting commands")
    }

    #[cfg(not(feature = "soapy"))]
    pub fn key(
        &self,
        _gain_db: f64,
        _deviation_hz: f64,
        _mic_gain: f64,
        _offset_hz: f64,
        _tone_hz: f64,
    ) -> Result<(), &'static str> {
        Err("built without SDR support")
    }

    /// End the current transmission early (a no-op if nothing is keyed).
    pub fn unkey(&self) {
        #[cfg(feature = "soapy")]
        {
            let run = self.run.lock().unwrap();
            if let Some(cmd_tx) = &run.cmd_tx {
                let _ = cmd_tx.send(PipelineCmd::Unkey);
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AudioFrame> {
        self.tx.subscribe()
    }

    /// Shared ADS-B track table + Beast feed handle (populated only while the
    /// `adsb` mode pipeline is running).
    pub fn adsb(&self) -> Arc<AdsbShared> {
        self.adsb.clone()
    }

    /// Shared AIS vessel table + AIVDM feed handle (populated only while the
    /// `ais` mode pipeline is running).
    pub fn ais(&self) -> Arc<AisShared> {
        self.ais.clone()
    }

    /// Shared APT image buffer (populated only while the `apt` mode pipeline
    /// is running).
    pub fn apt(&self) -> Arc<AptShared> {
        self.apt.clone()
    }

    /// Shared receiver-analysis spectrum/waterfall + IQ recorder
    /// (populated only while the `analysis` mode pipeline is running).
    pub fn analysis(&self) -> Arc<AnalysisShared> {
        self.analysis.clone()
    }

    /// Shared demod-audio recorder (start/stop over REST; the pipeline
    /// thread writes 20 ms frames into it while a recording is active).
    pub fn audio_rec(&self) -> Arc<AudioRecorder> {
        self.audio_rec.clone()
    }

    pub fn is_running(&self) -> bool {
        self.run.lock().unwrap().running
    }

    pub fn telemetry(&self) -> Telemetry {
        self.telemetry.lock().unwrap().clone()
    }

    pub fn start(&self) {
        let mut run = self.run.lock().unwrap();
        if run.running {
            return;
        }
        let params = {
            let cfg = self.cfg.lock().unwrap();
            let device = self.registry.selected();
            PipelineParams::from_config(&cfg, device.as_ref())
        };
        let stop = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let tele = self.telemetry.clone();
        *self.telemetry.lock().unwrap() = Telemetry::default();

        let stop_thread = stop.clone();
        let dump = self.dump_wav.clone();
        let adsb = self.adsb.clone();
        let ais = self.ais.clone();
        let apt = self.apt.clone();
        let analysis = self.analysis.clone();
        let audio_rec = self.audio_rec.clone();
        let audio_in = self.audio_in.clone();

        #[cfg(feature = "soapy")]
        let (cmd_tx, cmd_rx) = mpsc::channel::<PipelineCmd>();
        #[cfg(not(feature = "soapy"))]
        let cmd_rx = ();

        let handle = std::thread::Builder::new()
            .name("rx-pipeline".into())
            .spawn(move || {
                run_pipeline(
                    params, tx, tele, stop_thread, dump, cmd_rx, adsb, ais, apt, analysis, audio_rec,
                    audio_in,
                )
            })
            .expect("spawn rx-pipeline thread");

        run.stop = Some(stop);
        run.handle = Some(handle);
        #[cfg(feature = "soapy")]
        {
            run.cmd_tx = Some(cmd_tx);
        }
        run.running = true;
        tracing::info!("pipeline started");
    }

    pub fn stop(&self) {
        // Finalize any in-progress audio recording — the pipeline thread that
        // feeds it is about to exit (and a mode switch bounces through here).
        if let Some(info) = self.audio_rec.stop() {
            tracing::info!("audio recording finalized: {} ({:.1}s)", info.filename, info.secs);
        }
        let (stop, handle) = {
            let mut run = self.run.lock().unwrap();
            run.running = false;
            #[cfg(feature = "soapy")]
            {
                run.cmd_tx = None;
            }
            (run.stop.take(), run.handle.take())
        };
        if let Some(stop) = stop {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        *self.telemetry.lock().unwrap() = Telemetry::default();
        tracing::info!("pipeline stopped");
    }

    /// Apply a live config change by bouncing the pipeline if it is running.
    pub fn reconfigure(&self) {
        if self.is_running() {
            self.stop();
            self.start();
        }
    }

    /// Apply a `PATCH /radio` change to a running pipeline: hardware retune and
    /// gain, plus NBFM filter params, go live without an audio gap; anything
    /// that changes the sample rate, mode, channel, antenna or LO offset
    /// bounces the pipeline.
    pub fn apply_patch(&self, old: &RadioConfig, new: &RadioConfig) {
        if !self.is_running() {
            return;
        }

        let f = |a: f64, b: f64| (a - b).abs() > 1.0;
        let needs_restart = old.mode != new.mode
            || old.enabled != new.enabled
            || f(old.tuner.sample_rate_hz, new.tuner.sample_rate_hz)
            || old.tuner.channel != new.tuner.channel
            || old.tuner.antenna != new.tuner.antenna
            || f(old.tuner.lo_offset_hz, new.tuner.lo_offset_hz)
            // Applied only at pipeline open (see `run_sdr`), so a live change
            // needs a bounce. These are set-and-leave knobs, not things you
            // sweep — the sweepable ones (frequency, gain) hot-apply below.
            || old.tuner.bandwidth_hz != new.tuner.bandwidth_hz
            || (old.tuner.freq_correction_ppm - new.tuner.freq_correction_ppm).abs() > 1e-9
            || old.tuner.dc_offset_correction != new.tuner.dc_offset_correction
            || old.tuner.iq_balance_correction != new.tuner.iq_balance_correction
            || old.tuner.device_settings != new.tuner.device_settings
            || old.audio.frame_ms != new.audio.frame_ms
            || old.audio.sample_rate_hz != new.audio.sample_rate_hz
            || old.audio.opus_bitrate_bps != new.audio.opus_bitrate_bps
            || (!matches!(new.mode.as_str(), "nbfm" | "wbfm" | "am" | "frs" | "ham") && old.mode_params != new.mode_params);

        if needs_restart {
            self.reconfigure();
            return;
        }

        #[cfg(feature = "soapy")]
        {
            let mut cmds = Vec::new();
            if old.frequency_hz != new.frequency_hz {
                cmds.push(PipelineCmd::Retune(new.frequency_hz as f64));
            }
            if old.tuner.gain_mode != new.tuner.gain_mode
                || old.tuner.gain_db != new.tuner.gain_db
                || old.tuner.gain_elements_db != new.tuner.gain_elements_db
            {
                cmds.push(PipelineCmd::Gain {
                    agc: matches!(new.tuner.gain_mode, GainMode::Agc),
                    overall: new.tuner.gain_db,
                    elements: new
                        .tuner
                        .gain_elements_db
                        .iter()
                        .map(|(k, v)| (k.clone(), *v))
                        .collect(),
                });
            }
            if old.mode_params != new.mode_params {
                match new.mode.as_str() {
                    "nbfm" | "wbfm" | "frs" | "ham" => {
                        cmds.push(PipelineCmd::Demod(DemodParams::Fm(fm_params(new))))
                    }
                    "am" => cmds.push(PipelineCmd::Demod(DemodParams::Am(am_params(new)))),
                    _ => {}
                }
            }
            if !cmds.is_empty() {
                if let Some(tx) = &self.run.lock().unwrap().cmd_tx {
                    for c in cmds {
                        let _ = tx.send(c);
                    }
                }
            }
        }
        #[cfg(not(feature = "soapy"))]
        {
            let _ = (old, new);
        }
    }
}

/// Build the DSP chain parameters from the current radio config.
#[cfg(feature = "soapy")]
fn fm_params(cfg: &RadioConfig) -> dsp::FmParams {
    let p = |key: &str, default: f64| cfg.mode_params.get(key).and_then(|v| v.as_f64()).unwrap_or(default);
    dsp::FmParams {
        deviation_hz: p("deviation_hz", 5_000.0),
        channel_bw_hz: p("channel_bw_hz", 16_000.0),
        deemphasis_us: p("deemphasis_us", 75.0),
        audio_lpf_hz: p("audio_lpf_hz", 3_400.0),
        squelch_dbfs: p("squelch_db", -80.0),
        noise_squelch: p("noise_squelch", 0.18),
        lo_offset_hz: cfg.tuner.lo_offset_hz.abs().max(1.0),
        ctcss_hz: p("ctcss_hz", 0.0),
        // `ctcss_squelch` 0 = monitor only (detect + report, don't gate).
        ctcss_squelch: p("ctcss_squelch", 1.0) != 0.0,
    }
}

/// Build the AM DSP chain parameters from the current radio config.
#[cfg(feature = "soapy")]
fn am_params(cfg: &RadioConfig) -> dsp::AmParams {
    let p = |key: &str, default: f64| cfg.mode_params.get(key).and_then(|v| v.as_f64()).unwrap_or(default);
    dsp::AmParams {
        channel_bw_hz: p("channel_bw_hz", 10_000.0),
        audio_lpf_hz: p("audio_lpf_hz", 5_000.0),
        squelch_dbfs: p("squelch_db", -80.0),
        noise_squelch: p("noise_squelch", 0.5),
        lo_offset_hz: cfg.tuner.lo_offset_hz.abs().max(1.0),
    }
}

// --- pipeline parameters ------------------------------------------------

struct PipelineParams {
    kind: SourceKind,
    frames: FrameCfg,
}

#[derive(Clone, Copy)]
struct FrameCfg {
    frame_samples: usize,
    frame: Duration,
    bitrate_bps: i32,
}

enum SourceKind {
    Tone { hz: f64, amp: f32 },
    Silence,
    #[cfg(feature = "soapy")]
    Sdr(Box<SdrParams>),
    #[cfg(feature = "soapy")]
    Adsb(Box<AdsbSdrParams>),
    #[cfg(feature = "soapy")]
    Ais(Box<AisSdrParams>),
    #[cfg(feature = "soapy")]
    Apt(Box<AptSdrParams>),
    #[cfg(feature = "soapy")]
    Analysis(Box<AnalysisSdrParams>),
}

#[cfg(feature = "soapy")]
struct AdsbSdrParams {
    soapy_args: String,
    device_rate: f64,
    freq_hz: f64,
    channel: usize,
    antenna: Option<String>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    settings: Vec<(String, String)>,
    freq_correction_ppm: f64,
    reference: Option<(f64, f64)>,
    max_range_nm: f64,
    trail_secs: f64,
    forget_secs: f64,
    fix_errors: bool,
}

#[cfg(feature = "soapy")]
struct AisSdrParams {
    soapy_args: String,
    device_rate: f64,
    channel: usize,
    antenna: Option<String>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    settings: Vec<(String, String)>,
    freq_correction_ppm: f64,
    reference: Option<(f64, f64)>,
    max_range_nm: f64,
    trail_secs: f64,
    forget_secs: f64,
}

#[cfg(feature = "soapy")]
struct AptSdrParams {
    soapy_args: String,
    device_rate: f64,
    freq_hz: f64,
    channel: usize,
    antenna: Option<String>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    settings: Vec<(String, String)>,
    freq_correction_ppm: f64,
    apt: crate::apt::demod::AptParams,
    max_lines: usize,
}

#[cfg(feature = "soapy")]
struct AnalysisSdrParams {
    soapy_args: String,
    device_rate: f64,
    freq_hz: f64,
    channel: usize,
    antenna: Option<String>,
    bandwidth_hz: Option<f64>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    settings: Vec<(String, String)>,
    freq_correction_ppm: f64,
    fft_size: usize,
    window_code: u32,
    frame_rate_hz: f64,
    avg_alpha: f32,
    max_rows: usize,
}

#[cfg(feature = "soapy")]
struct SdrParams {
    soapy_args: String,
    device_rate: f64,
    freq_hz: f64,
    channel: usize,
    antenna: Option<String>,
    /// Analog filter bandwidth (`SoapySDRDevice_setBandwidth`). `None` leaves
    /// the driver default.
    bandwidth_hz: Option<f64>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    dc_offset: bool,
    settings: Vec<(String, String)>,
    freq_correction_ppm: f64,
    demod: DemodParams,
}

/// Correct a requested RF frequency for a device's known crystal error
/// (parts-per-million). A HackRF's stock TCXO commonly drifts a few to a few
/// tens of ppm; at UHF (462 MHz FRS, say) that's several kHz — enough to push
/// a narrowband FM discriminator (±2.5 kHz deviation) out of its linear range
/// entirely, while the same error is negligible at VHF. Determine a device's
/// ppm empirically (tune to a known-frequency signal, watch for a steady
/// discriminator bias — see docs/architecture.md) and set it once via
/// `--freq-correction-ppm` or `PATCH /radio`'s `tuner.freq_correction_ppm`.
#[cfg(feature = "soapy")]
fn apply_ppm(freq_hz: f64, ppm: f64) -> f64 {
    freq_hz * (1.0 + ppm * 1e-6)
}

impl PipelineParams {
    fn from_config(cfg: &RadioConfig, device: Option<&crate::model::DeviceInfo>) -> Self {
        let frame_ms = cfg.audio.frame_ms.clamp(10, 60);
        let frame_samples = (AUDIO_RATE as usize / 1000) * frame_ms as usize;
        let bitrate_bps = cfg.audio.opus_bitrate_bps as i32;
        let frame = Duration::from_millis(frame_ms as u64);

        let param = |key: &str, default: f64| -> f64 {
            cfg.mode_params.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
        };

        let kind = match cfg.mode.as_str() {
            "debug_tone" => {
                let hz = param("tone_hz", 440.0).clamp(20.0, 20_000.0);
                let level_dbfs = param("level_dbfs", -12.0).clamp(-60.0, -1.0);
                SourceKind::Tone { hz, amp: 10f64.powf(level_dbfs / 20.0) as f32 }
            }
            "nbfm" | "wbfm" | "am" | "frs" | "ham" => {
                #[cfg(not(feature = "soapy"))]
                {
                    let _ = device;
                    SourceKind::Silence
                }
                #[cfg(feature = "soapy")]
                match device {
                Some(dev) => SourceKind::Sdr(Box::new(SdrParams {
                    soapy_args: dev.soapy_args.clone(),
                    device_rate: cfg.tuner.sample_rate_hz,
                    freq_hz: cfg.frequency_hz as f64,
                    channel: cfg.tuner.channel,
                    antenna: cfg.tuner.antenna.clone(),
                    bandwidth_hz: cfg.tuner.bandwidth_hz,
                    agc: matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc),
                    gain_overall_db: cfg.tuner.gain_db,
                    gain_elements_db: cfg
                        .tuner
                        .gain_elements_db
                        .iter()
                        .map(|(k, v)| (k.clone(), *v))
                        .collect(),
                    dc_offset: cfg.tuner.dc_offset_correction,
                    settings: cfg
                        .tuner
                        .device_settings
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    freq_correction_ppm: cfg.tuner.freq_correction_ppm,
                    demod: if cfg.mode == "am" {
                        DemodParams::Am(am_params(cfg))
                    } else {
                        DemodParams::Fm(fm_params(cfg))
                    },
                })),
                None => SourceKind::Silence,
                }
            }
            "adsb" => {
                #[cfg(not(feature = "soapy"))]
                {
                    let _ = device;
                    SourceKind::Silence
                }
                #[cfg(feature = "soapy")]
                match device {
                    Some(dev) => {
                        let rlat = param("reference_lat", 0.0);
                        let rlon = param("reference_lon", 0.0);
                        let reference =
                            (rlat.abs() > 0.01 || rlon.abs() > 0.01).then_some((rlat, rlon));
                        SourceKind::Adsb(Box::new(AdsbSdrParams {
                            soapy_args: dev.soapy_args.clone(),
                            device_rate: cfg.tuner.sample_rate_hz,
                            freq_hz: cfg.frequency_hz as f64,
                            channel: cfg.tuner.channel,
                            antenna: cfg.tuner.antenna.clone(),
                            agc: matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc),
                            gain_overall_db: cfg.tuner.gain_db,
                            gain_elements_db: cfg
                                .tuner
                                .gain_elements_db
                                .iter()
                                .map(|(k, v)| (k.clone(), *v))
                                .collect(),
                            settings: cfg
                                .tuner
                                .device_settings
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                            freq_correction_ppm: cfg.tuner.freq_correction_ppm,
                            reference,
                            max_range_nm: param("max_range_nm", 250.0),
                            trail_secs: param("trail_seconds", 120.0),
                            forget_secs: param("forget_seconds", 60.0),
                            fix_errors: param("fix_errors", 1.0) != 0.0,
                        }))
                    }
                    None => SourceKind::Silence,
                }
            }
            "ais" => {
                #[cfg(not(feature = "soapy"))]
                {
                    let _ = device;
                    SourceKind::Silence
                }
                #[cfg(feature = "soapy")]
                match device {
                    Some(dev) => {
                        let rlat = param("reference_lat", 0.0);
                        let rlon = param("reference_lon", 0.0);
                        let reference =
                            (rlat.abs() > 0.01 || rlon.abs() > 0.01).then_some((rlat, rlon));
                        SourceKind::Ais(Box::new(AisSdrParams {
                            soapy_args: dev.soapy_args.clone(),
                            device_rate: cfg.tuner.sample_rate_hz,
                            channel: cfg.tuner.channel,
                            antenna: cfg.tuner.antenna.clone(),
                            agc: matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc),
                            gain_overall_db: cfg.tuner.gain_db,
                            gain_elements_db: cfg
                                .tuner
                                .gain_elements_db
                                .iter()
                                .map(|(k, v)| (k.clone(), *v))
                                .collect(),
                            settings: cfg
                                .tuner
                                .device_settings
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                            freq_correction_ppm: cfg.tuner.freq_correction_ppm,
                            reference,
                            max_range_nm: param("max_range_nm", 60.0),
                            trail_secs: param("trail_seconds", 600.0),
                            forget_secs: param("forget_seconds", 900.0),
                        }))
                    }
                    None => SourceKind::Silence,
                }
            }
            "apt" => {
                #[cfg(not(feature = "soapy"))]
                {
                    let _ = device;
                    SourceKind::Silence
                }
                #[cfg(feature = "soapy")]
                match device {
                    Some(dev) => SourceKind::Apt(Box::new(AptSdrParams {
                        soapy_args: dev.soapy_args.clone(),
                        device_rate: cfg.tuner.sample_rate_hz,
                        freq_hz: cfg.frequency_hz as f64,
                        channel: cfg.tuner.channel,
                        antenna: cfg.tuner.antenna.clone(),
                        agc: matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc),
                        gain_overall_db: cfg.tuner.gain_db,
                        gain_elements_db: cfg
                            .tuner
                            .gain_elements_db
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect(),
                        settings: cfg
                            .tuner
                            .device_settings
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        freq_correction_ppm: cfg.tuner.freq_correction_ppm,
                        apt: crate::apt::demod::AptParams {
                            deviation_hz: param("deviation_hz", 17_000.0),
                            channel_bw_hz: param("channel_bw_hz", 40_000.0),
                            subcarrier_hz: 2_400.0,
                            lo_offset_hz: cfg.tuner.lo_offset_hz.abs().max(1.0),
                        },
                        max_lines: param("max_lines", 1200.0).max(1.0) as usize,
                    })),
                    None => SourceKind::Silence,
                }
            }
            "analysis" => {
                #[cfg(not(feature = "soapy"))]
                {
                    let _ = device;
                    SourceKind::Silence
                }
                #[cfg(feature = "soapy")]
                match device {
                    Some(dev) => SourceKind::Analysis(Box::new(AnalysisSdrParams {
                        soapy_args: dev.soapy_args.clone(),
                        device_rate: cfg.tuner.sample_rate_hz,
                        freq_hz: cfg.frequency_hz as f64,
                        channel: cfg.tuner.channel,
                        antenna: cfg.tuner.antenna.clone(),
                        bandwidth_hz: cfg.tuner.bandwidth_hz,
                        agc: matches!(cfg.tuner.gain_mode, crate::model::GainMode::Agc),
                        gain_overall_db: cfg.tuner.gain_db,
                        gain_elements_db: cfg
                            .tuner
                            .gain_elements_db
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect(),
                        settings: cfg
                            .tuner
                            .device_settings
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        freq_correction_ppm: cfg.tuner.freq_correction_ppm,
                        fft_size: param("fft_size", 4096.0).max(256.0) as usize,
                        window_code: param("window", 0.0).max(0.0) as u32,
                        frame_rate_hz: param("frame_rate_hz", 20.0).clamp(1.0, 60.0),
                        avg_alpha: (1.0 - param("averaging", 0.5).clamp(0.0, 0.98)) as f32,
                        max_rows: 2000,
                    })),
                    None => SourceKind::Silence,
                }
            }
            _ => SourceKind::Silence,
        };

        Self { kind, frames: FrameCfg { frame_samples, frame, bitrate_bps } }
    }
}

// --- pipeline execution ------------------------------------------------

fn new_encoder(bitrate_bps: i32) -> Option<Encoder> {
    let mut enc = match Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("opus encoder init failed: {e}");
            return None;
        }
    };
    if let Err(e) = enc.set_bitrate(Bitrate::BitsPerSecond(bitrate_bps)) {
        tracing::warn!("opus set_bitrate failed: {e}");
    }
    Some(enc)
}

#[cfg(feature = "soapy")]
type CmdRx = mpsc::Receiver<PipelineCmd>;
#[cfg(not(feature = "soapy"))]
type CmdRx = ();

#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    p: PipelineParams,
    tx: broadcast::Sender<AudioFrame>,
    telemetry: Arc<Mutex<Telemetry>>,
    stop: Arc<AtomicBool>,
    dump: Option<PathBuf>,
    cmd_rx: CmdRx,
    adsb: Arc<AdsbShared>,
    ais: Arc<AisShared>,
    apt: Arc<AptShared>,
    analysis: Arc<AnalysisShared>,
    audio_rec: Arc<AudioRecorder>,
    audio_in: Arc<TxAudioSource>,
) {
    let _ = (&adsb, &ais, &apt, &analysis, &audio_in);
    let fc = p.frames;
    let mut wav = dump.and_then(|path| match WavDump::create(&path, AUDIO_RATE, 1) {
        Ok(w) => {
            tracing::info!("dumping pipeline audio to {}", path.display());
            Some(w)
        }
        Err(e) => {
            tracing::warn!("dump-wav {}: {e}", path.display());
            None
        }
    });
    match p.kind {
        SourceKind::Tone { hz, amp } => {
            let _ = &cmd_rx;
            run_synth(Synth::Tone { hz, amp }, fc, &tx, &telemetry, &stop, wav.as_mut(), &audio_rec)
        }
        SourceKind::Silence => {
            let _ = &cmd_rx;
            run_synth(Synth::Silence, fc, &tx, &telemetry, &stop, wav.as_mut(), &audio_rec)
        }
        #[cfg(feature = "soapy")]
        SourceKind::Sdr(sdr) => {
            run_sdr(*sdr, fc, &tx, &telemetry, &stop, wav.as_mut(), &audio_rec, cmd_rx, &audio_in)
        }
        #[cfg(feature = "soapy")]
        SourceKind::Adsb(p) => run_adsb(*p, adsb, &telemetry, &stop, cmd_rx),
        #[cfg(feature = "soapy")]
        SourceKind::Ais(p) => run_ais(*p, ais, &telemetry, &stop, cmd_rx),
        #[cfg(feature = "soapy")]
        SourceKind::Apt(p) => run_apt(*p, apt, &telemetry, &stop, cmd_rx),
        #[cfg(feature = "soapy")]
        SourceKind::Analysis(p) => run_analysis(*p, analysis, &telemetry, &stop, cmd_rx),
    }
}

enum Synth {
    Tone { hz: f64, amp: f32 },
    Silence,
}

fn run_synth(
    synth: Synth,
    fc: FrameCfg,
    tx: &broadcast::Sender<AudioFrame>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    mut wav: Option<&mut WavDump>,
    audio_rec: &AudioRecorder,
) {
    let Some(encoder) = new_encoder(fc.bitrate_bps) else {
        return;
    };
    {
        let mut t = telemetry.lock().unwrap();
        t.source = match synth {
            Synth::Tone { .. } => "debug_tone",
            Synth::Silence => "silence",
        };
        t.squelch_open = matches!(synth, Synth::Tone { .. });
    }

    let mut pcm = vec![0f32; fc.frame_samples];
    let mut buf = vec![0u8; OPUS_MAX_FRAME];
    let mut phase = 0f64;
    let dphase = match synth {
        Synth::Tone { hz, .. } => TAU * hz / AUDIO_RATE as f64,
        Synth::Silence => 0.0,
    };
    let mut next = Instant::now() + fc.frame;

    while !stop.load(Ordering::SeqCst) {
        match synth {
            Synth::Tone { amp, .. } => {
                for s in pcm.iter_mut() {
                    *s = phase.sin() as f32 * amp;
                    phase += dphase;
                    if phase >= TAU {
                        phase -= TAU;
                    }
                }
            }
            Synth::Silence => pcm.iter_mut().for_each(|s| *s = 0.0),
        }

        let rms = rms_dbfs(&pcm);
        if let Some(w) = wav.as_deref_mut() {
            w.write(&pcm);
        }
        audio_rec.write(&pcm);
        if let Ok(n) = encoder.encode_float(&pcm, &mut buf) {
            let _ = tx.send(AudioFrame {
                data: Bytes::copy_from_slice(&buf[..n]),
                duration: fc.frame,
            });
            let mut t = telemetry.lock().unwrap();
            t.frames_sent = t.frames_sent.wrapping_add(1);
            t.audio_level_dbfs = Some(rms);
        }

        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += fc.frame;
        if next < now {
            next = now + fc.frame;
        }
    }
}

#[cfg(feature = "soapy")]
#[allow(clippy::too_many_arguments)]
fn run_sdr(
    sp: SdrParams,
    fc: FrameCfg,
    tx: &broadcast::Sender<AudioFrame>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    mut wav: Option<&mut WavDump>,
    audio_rec: &AudioRecorder,
    cmd_rx: mpsc::Receiver<PipelineCmd>,
    audio_in: &Arc<TxAudioSource>,
) {
    use num_complex::Complex32;
    use soapysdr::{Device, Direction, ErrorCode};

    let dir = Direction::Rx;
    let ch = sp.channel;
    let label = sp.demod.label();

    let dev = match Device::new(sp.soapy_args.as_str()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("{label}: cannot open device: {e}");
            return;
        }
    };

    let log_set = |what: &str, r: Result<(), soapysdr::Error>| {
        if let Err(e) = r {
            tracing::warn!("{label}: set {what}: {e}");
        }
    };
    log_set("sample_rate", dev.set_sample_rate(dir, ch, sp.device_rate));
    // `sp` is immutable and only carries the *start* channel. Track the live
    // RX frequency here so it survives `PipelineCmd::Retune` — both the TX
    // key (see `key_tx`) and the post-key RX restore must read this, not
    // `sp.freq_hz`, or they snap back to whatever channel the pipeline
    // launched on.
    let mut cur_freq_hz = sp.freq_hz;
    let mut lo = cur_freq_hz - sp.demod.lo_offset_hz();
    log_set("frequency", dev.set_frequency(dir, ch, apply_ppm(lo, sp.freq_correction_ppm), ""));
    if let Some(ant) = &sp.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
    }
    if let Some(bw) = sp.bandwidth_hz {
        log_set("bandwidth", dev.set_bandwidth(dir, ch, bw));
    }
    if sp.agc {
        log_set("agc", dev.set_gain_mode(dir, ch, true));
    } else {
        log_set("gain_mode", dev.set_gain_mode(dir, ch, false));
        if let Some(g) = sp.gain_overall_db {
            log_set("gain", dev.set_gain(dir, ch, g));
        }
        for (name, v) in &sp.gain_elements_db {
            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
        }
    }
    log_set("dc_offset_mode", dev.set_dc_offset_mode(dir, ch, sp.dc_offset));
    for (k, v) in &sp.settings {
        log_set("setting", dev.write_setting(k.as_str(), v.as_str()));
    }

    let mut stream = match dev.rx_stream::<Complex32>(&[ch]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("{label}: rx_stream: {e}");
            return;
        }
    };
    if let Err(e) = stream.activate(None) {
        tracing::error!("{label}: stream activate: {e}");
        return;
    }

    let mtu = stream.mtu().unwrap_or(65_536).clamp(1024, 1 << 20);
    let mut iq = vec![Complex32::new(0.0, 0.0); mtu];
    let mut chain = Chain::new(sp.device_rate, sp.demod);
    let encoder = match new_encoder(fc.bitrate_bps) {
        Some(e) => e,
        None => {
            let _ = stream.deactivate(None);
            return;
        }
    };
    telemetry.lock().unwrap().source = label;
    tracing::info!(
        "{label}: {} @ {:.4} MHz (LO {:.3} MHz), {:.3} Msps, channel rate {:.0} Hz",
        sp.soapy_args,
        sp.freq_hz / 1e6,
        lo / 1e6,
        sp.device_rate / 1e6,
        chain.channel_rate(),
    );

    let mut acc: Vec<f32> = Vec::with_capacity(fc.frame_samples * 4);
    let mut buf = vec![0u8; OPUS_MAX_FRAME];
    let mut overruns: u64 = 0;
    let mut scan: Option<ScanRun> = None;

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCmd::Retune(hz) => {
                    // A manual tune cancels an in-progress scan.
                    if scan.take().is_some() {
                        tracing::info!("{label}: scan cancelled by manual retune");
                    }
                    sdr_retune(
                        &dev, dir, ch, &sp, &log_set, hz, &mut chain, &mut cur_freq_hz, &mut lo,
                    );
                    tracing::info!("{label}: retuned to {:.4} MHz (live)", hz / 1e6);
                }
                PipelineCmd::Scan(Some(cfg)) => {
                    if cfg.chans.is_empty() {
                        scan = None;
                    } else {
                        let first = cfg.chans[0].freq_hz;
                        sdr_retune(
                            &dev, dir, ch, &sp, &log_set, first, &mut chain, &mut cur_freq_hz,
                            &mut lo,
                        );
                        tracing::info!("{label}: scan start — {} channels", cfg.chans.len());
                        scan = Some(ScanRun {
                            cfg,
                            idx: 0,
                            dwell_start: Instant::now(),
                            parked: false,
                            lost_at: None,
                        });
                    }
                }
                PipelineCmd::Scan(None) => {
                    if scan.take().is_some() {
                        tracing::info!(
                            "{label}: scan stopped, holding {:.4} MHz",
                            cur_freq_hz / 1e6
                        );
                    }
                }
                PipelineCmd::Gain { agc, overall, elements } => {
                    log_set("gain_mode", dev.set_gain_mode(dir, ch, agc));
                    if !agc {
                        if let Some(g) = overall {
                            log_set("gain", dev.set_gain(dir, ch, g));
                        }
                        for (name, v) in &elements {
                            log_set(
                                "gain_element",
                                dev.set_gain_element(dir, ch, name.as_str(), *v),
                            );
                        }
                    }
                }
                PipelineCmd::Demod(params) => {
                    chain = Chain::new(sp.device_rate, params);
                }
                PipelineCmd::Key(spec) => {
                    // Repeater split: transmit on RX freq + offset (a plain LO
                    // retune, restored to `lo` for RX below).
                    let tx_freq_hz = cur_freq_hz + spec.offset_hz;
                    key_tx(
                        &dev, ch, &log_set, &sp, tx_freq_hz, &spec, &cmd_rx, stop, telemetry, label,
                        &mut stream, audio_in,
                    );
                    // The device was retuned to the TX frequency for the
                    // duration of the key; put RX back on the live channel
                    // (which may have moved via Retune since pipeline start).
                    log_set(
                        "frequency",
                        dev.set_frequency(dir, ch, apply_ppm(lo, sp.freq_correction_ppm), ""),
                    );
                    if let Err(e) = stream.activate(None) {
                        tracing::error!("{label}: rx stream reactivate after TX: {e}");
                    }
                    chain.on_retune();
                    tracing::info!("{label}: TX unkeyed, RX resumed");
                }
                // A stray Unkey with nothing currently keyed (already ended,
                // or arrived after `key_tx` already returned) is a no-op —
                // the real unkey path is the `cmd_rx` check inside `key_tx`.
                PipelineCmd::Unkey => {}
            }
        }

        let n = match stream.read(&mut [iq.as_mut_slice()], 200_000) {
            Ok(n) => n,
            Err(e) => {
                match e.code {
                    ErrorCode::Timeout => {}
                    ErrorCode::Overflow => overruns += 1,
                    _ => {
                        tracing::warn!("{label}: stream read: {e}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                continue;
            }
        };
        if n == 0 {
            continue;
        }

        chain.process(&iq[..n], &mut acc);

        // Channel-scan state machine. `chain.metrics()` is fresh from the
        // `process` call above.
        if let Some(sc) = scan.as_mut() {
            let m = chain.metrics();
            let open = m.squelch_open && m.rssi_dbfs >= sc.cfg.rssi_gate_dbfs;
            let now = Instant::now();
            let n_chans = sc.cfg.chans.len();
            if sc.parked {
                if open {
                    sc.lost_at = None;
                } else if let Some(t) = sc.lost_at {
                    if now.duration_since(t) >= sc.cfg.hang {
                        sc.parked = false;
                        sc.lost_at = None;
                        sc.idx = (sc.idx + 1) % n_chans;
                        let hz = sc.cfg.chans[sc.idx].freq_hz;
                        sdr_retune(
                            &dev, dir, ch, &sp, &log_set, hz, &mut chain, &mut cur_freq_hz, &mut lo,
                        );
                        sc.dwell_start = now;
                    }
                } else {
                    sc.lost_at = Some(now);
                }
            } else if now.duration_since(sc.dwell_start) >= sc.cfg.dwell {
                // Dwell elapsed (squelch/noise envelope has settled since the
                // retune) — decide: park on a live channel, else step on.
                if open {
                    sc.parked = true;
                    sc.lost_at = None;
                    tracing::info!(
                        "{label}: scan parked on {} ({:.4} MHz, {:.0} dBFS)",
                        sc.cfg.chans[sc.idx].label,
                        cur_freq_hz / 1e6,
                        m.rssi_dbfs
                    );
                } else {
                    sc.idx = (sc.idx + 1) % n_chans;
                    let hz = sc.cfg.chans[sc.idx].freq_hz;
                    sdr_retune(
                        &dev, dir, ch, &sp, &log_set, hz, &mut chain, &mut cur_freq_hz, &mut lo,
                    );
                    sc.dwell_start = now;
                }
            }
        }

        while acc.len() >= fc.frame_samples {
            let pcm: Vec<f32> = acc.drain(..fc.frame_samples).collect();
            if let Some(w) = wav.as_deref_mut() {
                w.write(&pcm);
            }
            audio_rec.write(&pcm);
            if let Ok(nb) = encoder.encode_float(&pcm, &mut buf) {
                let _ = tx.send(AudioFrame {
                    data: Bytes::copy_from_slice(&buf[..nb]),
                    duration: fc.frame,
                });
                let m = chain.metrics();
                let mut t = telemetry.lock().unwrap();
                t.frames_sent = t.frames_sent.wrapping_add(1);
                t.audio_level_dbfs = Some(m.audio_dbfs);
                t.rssi_dbfs = Some(m.rssi_dbfs);
                t.snr_db = Some(m.snr_db);
                t.squelch_open = m.squelch_open;
                t.ctcss_tone_hz = (m.ctcss_tone_hz > 0.0).then_some(m.ctcss_tone_hz);
                t.ctcss_scan_hz = (m.ctcss_scan_hz > 0.0).then_some(m.ctcss_scan_hz);
                t.overruns = overruns;
                if let Some(sc) = scan.as_ref() {
                    t.scanning = true;
                    t.scan_parked = sc.parked;
                    t.scan_freq_hz = Some(cur_freq_hz);
                    t.scan_label = Some(sc.cfg.chans[sc.idx].label.clone());
                    t.scan_index = sc.idx;
                    t.scan_total = sc.cfg.chans.len();
                } else {
                    t.scanning = false;
                    t.scan_parked = false;
                    t.scan_freq_hz = None;
                    t.scan_label = None;
                    t.scan_index = 0;
                    t.scan_total = 0;
                }
            }
        }
    }

    let _ = stream.deactivate(None);
}

/// One push-to-talk transmission. HackRF (like most low-cost SDRs) is
/// half-duplex — confirmed directly against this project's own unit
/// (`SoapySDRUtil --probe`: "Full-duplex: NO" on both RX and TX channels) —
/// so this owns the full RX-down / TX-up / TX-down / (caller resumes RX)
/// hand-off: deactivates `rx_stream` first, retunes to `tx_freq_hz` — the
/// live RX channel, which may have moved via `PipelineCmd::Retune` since the
/// pipeline started, so the caller passes it in rather than this reading the
/// stale `sp.freq_hz` — straight, with no LO-offset digital mixing on the way
/// out, unlike RX (there's no DC-spike self-interference concern to dodge
/// when transmitting), opens+activates a TX stream, and repeatedly pulls live mic
/// audio from `audio_in` (silence-padded on underrun — see
/// `TxAudioSource::pull`), frequency-modulating it via `dsp::TxModulator` and
/// writing the resulting IQ, until `PipelineCmd::Unkey` arrives,
/// `spec.max_secs` elapses, or the pipeline is stopped.
#[cfg(feature = "soapy")]
#[allow(clippy::too_many_arguments)]
fn key_tx(
    dev: &soapysdr::Device,
    ch: usize,
    log_set: &dyn Fn(&str, Result<(), soapysdr::Error>),
    sp: &SdrParams,
    tx_freq_hz: f64,
    spec: &TxKeySpec,
    cmd_rx: &mpsc::Receiver<PipelineCmd>,
    stop: &Arc<AtomicBool>,
    telemetry: &Arc<Mutex<Telemetry>>,
    label: &'static str,
    rx_stream: &mut soapysdr::RxStream<num_complex::Complex32>,
    audio_in: &Arc<TxAudioSource>,
) {
    use num_complex::Complex32;
    use soapysdr::Direction;

    let _ = rx_stream.deactivate(None);
    telemetry.lock().unwrap().tx_keyed = true;
    tracing::info!(
        "{label}: TX key — {:.4} MHz ({:+.0} kHz offset), {:.0} Hz deviation, \
         {:.1} dB gain, CTCSS {}, max {:.0}s",
        tx_freq_hz / 1e6,
        spec.offset_hz / 1e3,
        spec.deviation_hz,
        if spec.tone_hz > 0.0 { format!("{:.1} Hz", spec.tone_hz) } else { "off".into() },
        spec.gain_db,
        spec.max_secs
    );

    let txdir = Direction::Tx;
    log_set(
        "tx frequency",
        dev.set_frequency(txdir, ch, apply_ppm(tx_freq_hz, sp.freq_correction_ppm), ""),
    );
    log_set("tx gain", dev.set_gain(txdir, ch, spec.gain_db));

    let result = (|| -> Result<(), soapysdr::Error> {
        let mut txs = dev.tx_stream::<Complex32>(&[ch])?;
        txs.activate(None)?;

        let mut modulator = TxModulator::new(sp.device_rate, spec.deviation_hz, spec.tone_hz);
        // 20ms @ 48kHz — matches the Opus frame size used elsewhere in this
        // app, though nothing here actually depends on that; it's just a
        // reasonable poll granularity for pulling from `audio_in`.
        const AUDIO_CHUNK: usize = 960;
        let mut audio_chunk: Vec<f32> = Vec::with_capacity(AUDIO_CHUNK);
        let mut iq_chunk: Vec<Complex32> = Vec::new();
        let mtu = txs.mtu().unwrap_or(65_536).max(1);
        let started = Instant::now();
        let max_dur = Duration::from_secs_f64(spec.max_secs.clamp(0.5, MAX_TX_SECS));

        'key: while !stop.load(Ordering::SeqCst) && started.elapsed() < max_dur {
            while let Ok(c) = cmd_rx.try_recv() {
                if matches!(c, PipelineCmd::Unkey) {
                    break 'key;
                }
                // Retune/Gain/Demod arriving mid-key are receive-oriented
                // and don't apply while transmitting; dropped.
            }
            // Silence-padded on underrun (no mic audio has arrived yet, or
            // network/decode is behind) — an unmodulated carrier for that
            // stretch, not a stall; see `TxAudioSource::pull`.
            audio_in.pull(AUDIO_CHUNK, &mut audio_chunk);
            // Boost the (typically low-level) mic PCM, then hard-limit to
            // full scale so peak deviation stays capped at `deviation_hz`
            // however hot the gain is set — see `frs`'s `tx_mic_gain`.
            if spec.mic_gain != 1.0 {
                let g = spec.mic_gain as f32;
                for s in audio_chunk.iter_mut() {
                    *s = (*s * g).clamp(-1.0, 1.0);
                }
            }
            iq_chunk.clear();
            modulator.process(&audio_chunk, &mut iq_chunk);

            let mut off = 0;
            while off < iq_chunk.len() {
                let end = (off + mtu).min(iq_chunk.len());
                let n = txs.write(&[&iq_chunk[off..end]], None, false, 200_000)?;
                off += n.max(1); // never spin forever on a persistent 0-write
            }
        }
        txs.deactivate(None)
    })();

    if let Err(e) = result {
        tracing::error!("{label}: TX: {e}");
    }
    telemetry.lock().unwrap().tx_keyed = false;
}

/// ADS-B pipeline: 2 Msps @ 1090 MHz into the [`crate::adsb`] demod/tracker.
/// Produces no audio — the tracks are read over REST and the Beast feed.
#[cfg(feature = "soapy")]
fn run_adsb(
    p: AdsbSdrParams,
    shared: Arc<AdsbShared>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    cmd_rx: mpsc::Receiver<PipelineCmd>,
) {
    use crate::adsb::demod::Demod;
    use num_complex::Complex32;
    use soapysdr::{Device, Direction, ErrorCode};

    let dir = Direction::Rx;
    let ch = p.channel;

    shared
        .tracker
        .lock()
        .unwrap()
        .reset(p.reference, p.max_range_nm, p.trail_secs, p.forget_secs);
    shared.hex.lock().unwrap().clear();

    let dev = match Device::new(p.soapy_args.as_str()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("adsb: cannot open device: {e}");
            return;
        }
    };
    let log_set = |what: &str, r: Result<(), soapysdr::Error>| {
        if let Err(e) = r {
            tracing::warn!("adsb: set {what}: {e}");
        }
    };
    log_set("sample_rate", dev.set_sample_rate(dir, ch, p.device_rate));
    log_set("frequency", dev.set_frequency(dir, ch, apply_ppm(p.freq_hz, p.freq_correction_ppm), ""));
    if let Some(ant) = &p.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
    }
    if p.agc {
        log_set("agc", dev.set_gain_mode(dir, ch, true));
    } else {
        log_set("gain_mode", dev.set_gain_mode(dir, ch, false));
        if let Some(g) = p.gain_overall_db {
            log_set("gain", dev.set_gain(dir, ch, g));
        }
        for (name, v) in &p.gain_elements_db {
            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
        }
    }
    for (k, v) in &p.settings {
        log_set("setting", dev.write_setting(k.as_str(), v.as_str()));
    }

    let mut stream = match dev.rx_stream::<Complex32>(&[ch]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("adsb: rx_stream: {e}");
            return;
        }
    };
    if let Err(e) = stream.activate(None) {
        tracing::error!("adsb: stream activate: {e}");
        return;
    }

    let mtu = stream.mtu().unwrap_or(65_536).clamp(1024, 1 << 20);
    let mut iq = vec![Complex32::new(0.0, 0.0); mtu];
    let mut demod = Demod::new(p.fix_errors);

    telemetry.lock().unwrap().source = "adsb";
    tracing::info!(
        "adsb: {} @ {:.3} MHz, {:.1} Msps, ref {}",
        p.soapy_args,
        p.freq_hz / 1e6,
        p.device_rate / 1e6,
        match p.reference {
            Some((la, lo)) => format!("{la:.3},{lo:.3}"),
            None => "none".into(),
        },
    );

    let mut frames: u64 = 0;
    let mut overruns: u64 = 0;
    let mut rssi_ema = -60.0f32;
    let mut last_status = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            if let PipelineCmd::Gain { agc, overall, elements } = cmd {
                log_set("gain_mode", dev.set_gain_mode(dir, ch, agc));
                if !agc {
                    if let Some(g) = overall {
                        log_set("gain", dev.set_gain(dir, ch, g));
                    }
                    for (name, v) in &elements {
                        log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
                    }
                }
            }
        }

        let n = match stream.read(&mut [iq.as_mut_slice()], 200_000) {
            Ok(n) => n,
            Err(e) => {
                match e.code {
                    ErrorCode::Timeout => {}
                    ErrorCode::Overflow => overruns += 1,
                    _ => {
                        tracing::warn!("adsb: stream read: {e}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                continue;
            }
        };
        if n == 0 {
            continue;
        }

        let shared_ref = &shared;
        demod.process(&iq[..n], |f| {
            frames += 1;
            rssi_ema = rssi_ema * 0.95 + f.rssi_dbfs * 0.05;
            shared_ref.record(&f);
        });

        if last_status.elapsed() >= Duration::from_millis(500) {
            last_status = Instant::now();
            shared.tracker.lock().unwrap().prune(shared.now_s());
            let mut t = telemetry.lock().unwrap();
            t.frames_sent = frames;
            t.rssi_dbfs = Some(rssi_ema);
            t.squelch_open = frames > 0;
            t.overruns = overruns;
        }
    }

    let _ = stream.deactivate(None);
}

/// AIS pipeline: 2 Msps @ 162.0 MHz, two GMSK channel decoders into the
/// [`crate::ais`] tracker + AIVDM feed. No audio.
#[cfg(feature = "soapy")]
fn run_ais(
    p: AisSdrParams,
    shared: Arc<AisShared>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    cmd_rx: mpsc::Receiver<PipelineCmd>,
) {
    use crate::ais::demod::ChannelDemod;
    use crate::ais::{CENTER_HZ, CHANNEL_A_HZ, CHANNEL_B_HZ};
    use num_complex::Complex32;
    use soapysdr::{Device, Direction, ErrorCode};

    let dir = Direction::Rx;
    let ch = p.channel;

    shared
        .tracker
        .lock()
        .unwrap()
        .reset(p.reference, p.max_range_nm, p.trail_secs, p.forget_secs);
    shared.sentences.lock().unwrap().clear();

    let dev = match Device::new(p.soapy_args.as_str()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("ais: cannot open device: {e}");
            return;
        }
    };
    let log_set = |what: &str, r: Result<(), soapysdr::Error>| {
        if let Err(e) = r {
            tracing::warn!("ais: set {what}: {e}");
        }
    };
    log_set("sample_rate", dev.set_sample_rate(dir, ch, p.device_rate));
    log_set("frequency", dev.set_frequency(dir, ch, apply_ppm(CENTER_HZ, p.freq_correction_ppm), ""));
    if let Some(ant) = &p.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
    }
    if p.agc {
        log_set("agc", dev.set_gain_mode(dir, ch, true));
    } else {
        log_set("gain_mode", dev.set_gain_mode(dir, ch, false));
        if let Some(g) = p.gain_overall_db {
            log_set("gain", dev.set_gain(dir, ch, g));
        }
        for (name, v) in &p.gain_elements_db {
            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
        }
    }
    for (k, v) in &p.settings {
        log_set("setting", dev.write_setting(k.as_str(), v.as_str()));
    }

    let mut stream = match dev.rx_stream::<Complex32>(&[ch]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("ais: rx_stream: {e}");
            return;
        }
    };
    if let Err(e) = stream.activate(None) {
        tracing::error!("ais: stream activate: {e}");
        return;
    }

    let mtu = stream.mtu().unwrap_or(65_536).clamp(1024, 1 << 20);
    let mut iq = vec![Complex32::new(0.0, 0.0); mtu];
    let mut chan_a = ChannelDemod::new(p.device_rate, CHANNEL_A_HZ - CENTER_HZ, 'A');
    let mut chan_b = ChannelDemod::new(p.device_rate, CHANNEL_B_HZ - CENTER_HZ, 'B');

    telemetry.lock().unwrap().source = "ais";
    tracing::info!(
        "ais: {} @ {:.3} MHz, {:.1} Msps, channel rate {:.0} Hz, ref {}",
        p.soapy_args,
        CENTER_HZ / 1e6,
        p.device_rate / 1e6,
        chan_a.channel_rate(),
        match p.reference {
            Some((la, lo)) => format!("{la:.3},{lo:.3}"),
            None => "none".into(),
        },
    );

    let mut frames: u64 = 0;
    let mut overruns: u64 = 0;
    let mut rssi_ema = -70.0f32;
    let mut last_status = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            if let PipelineCmd::Gain { agc, overall, elements } = cmd {
                log_set("gain_mode", dev.set_gain_mode(dir, ch, agc));
                if !agc {
                    if let Some(g) = overall {
                        log_set("gain", dev.set_gain(dir, ch, g));
                    }
                    for (name, v) in &elements {
                        log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
                    }
                }
            }
        }

        let n = match stream.read(&mut [iq.as_mut_slice()], 200_000) {
            Ok(n) => n,
            Err(e) => {
                match e.code {
                    ErrorCode::Timeout => {}
                    ErrorCode::Overflow => overruns += 1,
                    _ => {
                        tracing::warn!("ais: stream read: {e}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                continue;
            }
        };
        if n == 0 {
            continue;
        }

        let shared_ref = &shared;
        let mut on_frame = |f: crate::ais::demod::RawSentence| {
            frames += 1;
            rssi_ema = rssi_ema * 0.9 + f.rssi_dbfs * 0.1;
            shared_ref.record(&f);
        };
        chan_a.process(&iq[..n], &mut on_frame);
        chan_b.process(&iq[..n], &mut on_frame);

        if last_status.elapsed() >= Duration::from_millis(500) {
            last_status = Instant::now();
            shared.tracker.lock().unwrap().prune(shared.now_s());
            let mut t = telemetry.lock().unwrap();
            t.frames_sent = frames;
            t.rssi_dbfs = Some(rssi_ema);
            t.squelch_open = frames > 0;
            t.overruns = overruns;
        }
    }

    let _ = stream.deactivate(None);
}

/// NOAA APT pipeline: tunes 137 MHz, feeds the [`crate::apt`] demod, and
/// accumulates decoded scan lines into a growing image read over REST. No
/// audio output.
#[cfg(feature = "soapy")]
fn run_apt(
    p: AptSdrParams,
    shared: Arc<AptShared>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    cmd_rx: mpsc::Receiver<PipelineCmd>,
) {
    use crate::apt::demod::AptDemod;
    use num_complex::Complex32;
    use soapysdr::{Device, Direction, ErrorCode};

    let dir = Direction::Rx;
    let ch = p.channel;

    shared.reset(p.max_lines);

    let dev = match Device::new(p.soapy_args.as_str()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("apt: cannot open device: {e}");
            return;
        }
    };
    let log_set = |what: &str, r: Result<(), soapysdr::Error>| {
        if let Err(e) = r {
            tracing::warn!("apt: set {what}: {e}");
        }
    };
    log_set("sample_rate", dev.set_sample_rate(dir, ch, p.device_rate));
    let lo = p.freq_hz - p.apt.lo_offset_hz;
    log_set("frequency", dev.set_frequency(dir, ch, apply_ppm(lo, p.freq_correction_ppm), ""));
    if let Some(ant) = &p.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
    }
    if p.agc {
        log_set("agc", dev.set_gain_mode(dir, ch, true));
    } else {
        log_set("gain_mode", dev.set_gain_mode(dir, ch, false));
        if let Some(g) = p.gain_overall_db {
            log_set("gain", dev.set_gain(dir, ch, g));
        }
        for (name, v) in &p.gain_elements_db {
            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
        }
    }
    for (k, v) in &p.settings {
        log_set("setting", dev.write_setting(k.as_str(), v.as_str()));
    }

    let mut stream = match dev.rx_stream::<Complex32>(&[ch]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("apt: rx_stream: {e}");
            return;
        }
    };
    if let Err(e) = stream.activate(None) {
        tracing::error!("apt: stream activate: {e}");
        return;
    }

    let mtu = stream.mtu().unwrap_or(65_536).clamp(1024, 1 << 20);
    let mut iq = vec![Complex32::new(0.0, 0.0); mtu];
    let mut demod = AptDemod::new(p.device_rate, p.apt);

    telemetry.lock().unwrap().source = "apt";
    tracing::info!(
        "apt: {} @ {:.4} MHz (LO {:.3} MHz), {:.3} Msps, channel rate {:.0} Hz",
        p.soapy_args,
        p.freq_hz / 1e6,
        lo / 1e6,
        p.device_rate / 1e6,
        demod.channel_rate(),
    );

    let mut lines: u64 = 0;
    let mut overruns: u64 = 0;
    let mut rssi_ema = -60.0f32;
    let mut last_sync = 0.0f32;
    let mut last_status = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCmd::Retune(hz) => {
                    let new_lo = hz - p.apt.lo_offset_hz;
                    log_set(
                        "frequency",
                        dev.set_frequency(dir, ch, apply_ppm(new_lo, p.freq_correction_ppm), ""),
                    );
                    tracing::info!("apt: retuned to {:.4} MHz (live)", hz / 1e6);
                }
                PipelineCmd::Gain { agc, overall, elements } => {
                    log_set("gain_mode", dev.set_gain_mode(dir, ch, agc));
                    if !agc {
                        if let Some(g) = overall {
                            log_set("gain", dev.set_gain(dir, ch, g));
                        }
                        for (name, v) in &elements {
                            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
                        }
                    }
                }
                PipelineCmd::Demod(_) => {}
                // apt has no TX support (and no antenna-sharing concern —
                // it never transmits); a stray key/unkey/scan is a no-op here.
                PipelineCmd::Key(_) | PipelineCmd::Unkey | PipelineCmd::Scan(_) => {}
            }
        }

        let n = match stream.read(&mut [iq.as_mut_slice()], 200_000) {
            Ok(n) => n,
            Err(e) => {
                match e.code {
                    ErrorCode::Timeout => {}
                    ErrorCode::Overflow => overruns += 1,
                    _ => {
                        tracing::warn!("apt: stream read: {e}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                continue;
            }
        };
        if n == 0 {
            continue;
        }

        let power: f32 = iq[..n].iter().map(|c| c.norm_sqr()).sum::<f32>() / n as f32;
        let rssi_dbfs = (10.0 * (power + 1e-12).log10()).clamp(-90.0, 0.0);
        rssi_ema = rssi_ema * 0.95 + rssi_dbfs * 0.05;

        let shared_ref = &shared;
        demod.process(&iq[..n], |line| {
            lines += 1;
            last_sync = line.sync_quality;
            shared_ref.image.lock().unwrap().push_line(&line);
        });

        if last_status.elapsed() >= Duration::from_millis(500) {
            last_status = Instant::now();
            let mut t = telemetry.lock().unwrap();
            t.frames_sent = lines;
            t.rssi_dbfs = Some(rssi_ema);
            // Matched-filter SNR confidence from `AptDemod`; calibrated
            // against synthetic real-signal (~8) vs. noise (~3.2 ceiling)
            // steady-state values — see `apt::demod` module docs/tests.
            t.squelch_open = last_sync > 5.0;
            t.overruns = overruns;
        }
    }

    let _ = stream.deactivate(None);
}

/// Receiver Analysis pipeline: raw IQ → FFT panadapter + waterfall (written
/// into `analysis::Spectrum`), plus an optional IQ-to-WAV recorder tap. No
/// demod, no audio — the client renders the spectrum to canvas.
#[cfg(feature = "soapy")]
fn run_analysis(
    p: AnalysisSdrParams,
    shared: Arc<AnalysisShared>,
    telemetry: &Arc<Mutex<Telemetry>>,
    stop: &Arc<AtomicBool>,
    cmd_rx: mpsc::Receiver<PipelineCmd>,
) {
    use crate::analysis::Analyzer;
    use num_complex::Complex32;
    use soapysdr::{Device, Direction, ErrorCode};

    let dir = Direction::Rx;
    let ch = p.channel;

    let dev = match Device::new(p.soapy_args.as_str()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("analysis: cannot open device: {e}");
            return;
        }
    };
    let log_set = |what: &str, r: Result<(), soapysdr::Error>| {
        if let Err(e) = r {
            tracing::warn!("analysis: set {what}: {e}");
        }
    };
    log_set("sample_rate", dev.set_sample_rate(dir, ch, p.device_rate));
    // Analysis tunes the device straight to the center frequency — the whole
    // point is to see what's actually there, DC spike included.
    log_set(
        "frequency",
        dev.set_frequency(dir, ch, apply_ppm(p.freq_hz, p.freq_correction_ppm), ""),
    );
    if let Some(ant) = &p.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
    }
    if let Some(bw) = p.bandwidth_hz {
        log_set("bandwidth", dev.set_bandwidth(dir, ch, bw));
    }
    if p.agc {
        log_set("agc", dev.set_gain_mode(dir, ch, true));
    } else {
        log_set("gain_mode", dev.set_gain_mode(dir, ch, false));
        if let Some(g) = p.gain_overall_db {
            log_set("gain", dev.set_gain(dir, ch, g));
        }
        for (name, v) in &p.gain_elements_db {
            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
        }
    }
    for (k, v) in &p.settings {
        log_set("setting", dev.write_setting(k.as_str(), v.as_str()));
    }

    let mut analyzer = Analyzer::new(p.fft_size, p.window_code, p.frame_rate_hz);
    shared
        .spectrum
        .lock()
        .unwrap()
        .reset(p.freq_hz, p.device_rate, analyzer.size(), p.max_rows);

    let mut stream = match dev.rx_stream::<Complex32>(&[ch]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("analysis: rx_stream: {e}");
            return;
        }
    };
    if let Err(e) = stream.activate(None) {
        tracing::error!("analysis: stream activate: {e}");
        return;
    }

    let mtu = stream.mtu().unwrap_or(65_536).clamp(1024, 1 << 20);
    let mut iq = vec![Complex32::new(0.0, 0.0); mtu];

    telemetry.lock().unwrap().source = "analysis";
    tracing::info!(
        "analysis: {} @ {:.4} MHz, {:.3} Msps, {}-pt FFT @ {:.0} fps",
        p.soapy_args,
        p.freq_hz / 1e6,
        p.device_rate / 1e6,
        analyzer.size(),
        p.frame_rate_hz,
    );

    let mut overruns: u64 = 0;
    let mut rssi_ema = -60.0f32;
    let mut last_status = Instant::now();
    let avg_alpha = p.avg_alpha;

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCmd::Retune(hz) => {
                    log_set(
                        "frequency",
                        dev.set_frequency(dir, ch, apply_ppm(hz, p.freq_correction_ppm), ""),
                    );
                    shared.spectrum.lock().unwrap().set_center(hz);
                    tracing::info!("analysis: retuned to {:.4} MHz (live)", hz / 1e6);
                }
                PipelineCmd::Gain { agc, overall, elements } => {
                    log_set("gain_mode", dev.set_gain_mode(dir, ch, agc));
                    if !agc {
                        if let Some(g) = overall {
                            log_set("gain", dev.set_gain(dir, ch, g));
                        }
                        for (name, v) in &elements {
                            log_set("gain_element", dev.set_gain_element(dir, ch, name.as_str(), *v));
                        }
                    }
                }
                PipelineCmd::Demod(_)
                | PipelineCmd::Key(_)
                | PipelineCmd::Unkey
                | PipelineCmd::Scan(_) => {}
            }
        }

        let n = match stream.read(&mut [iq.as_mut_slice()], 200_000) {
            Ok(n) => n,
            Err(e) => {
                match e.code {
                    ErrorCode::Timeout => {}
                    ErrorCode::Overflow => overruns += 1,
                    _ => {
                        tracing::warn!("analysis: stream read: {e}");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                continue;
            }
        };
        if n == 0 {
            continue;
        }
        let block = &iq[..n];

        shared.record_iq(block);

        let power: f32 = block.iter().map(|c| c.norm_sqr()).sum::<f32>() / n as f32;
        let rssi_dbfs = (10.0 * (power + 1e-12).log10()).clamp(-120.0, 0.0);
        rssi_ema = rssi_ema * 0.95 + rssi_dbfs * 0.05;

        let sp = &shared.spectrum;
        analyzer.process(block, |db| {
            sp.lock().unwrap().push(db, avg_alpha);
        });

        if last_status.elapsed() >= Duration::from_millis(300) {
            last_status = Instant::now();
            let rows = shared.spectrum.lock().unwrap().seq;
            let mut t = telemetry.lock().unwrap();
            t.frames_sent = rows;
            t.rssi_dbfs = Some(rssi_ema);
            t.squelch_open = false;
            t.overruns = overruns;
        }
    }

    // Stop any recording cleanly on pipeline shutdown.
    if let Some(rec) = shared.recorder.lock().unwrap().take() {
        if let Ok(info) = rec.finish() {
            tracing::info!("analysis: IQ recording closed: {} ({} bytes)", info.filename, info.bytes);
            *shared.last_recording.lock().unwrap() = Some(info);
        }
    }
    let _ = stream.deactivate(None);
}

#[cfg(all(test, feature = "soapy"))]
mod tests {
    use super::{apply_ppm, TxAudit, TxAudioSource, TX_AUDIT_CAP};

    #[test]
    fn tx_audit_pairs_key_with_release_and_bounds_the_ring() {
        let a = TxAudit::default();
        a.key("HT".into(), "ham".into(), 146_340_000, -600_000, 100.0, 6.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        a.release("HT");

        let log = a.snapshot();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tx_frequency_hz, 146_340_000);
        assert_eq!(log[0].offset_hz, -600_000);
        assert!(log[0].released_at.is_some(), "release should close the open entry");
        assert!(log[0].duration_ms.unwrap() >= 5, "duration {:?}", log[0].duration_ms);

        // A release with no matching open key is a no-op, not a panic.
        a.release("nobody");

        // Ring stays bounded, newest-first ordering holds.
        for i in 0..TX_AUDIT_CAP + 20 {
            a.key(format!("c{i}"), "frs".into(), 462_562_500, 0, 0.0, 0.0);
        }
        let log = a.snapshot();
        assert_eq!(log.len(), TX_AUDIT_CAP);
        assert_eq!(log[0].client, format!("c{}", TX_AUDIT_CAP + 19), "newest first");
    }

    #[test]
    fn tx_audio_source_pulls_fifo_and_pads_silence_on_underrun() {
        let src = TxAudioSource::new();
        src.push(&[1.0, 2.0, 3.0]);
        src.push(&[4.0, 5.0]);

        let mut out = Vec::new();
        src.pull(4, &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0], "FIFO order across two pushes");

        out.clear();
        src.pull(4, &mut out);
        assert_eq!(out, vec![5.0, 0.0, 0.0, 0.0], "underrun pads with silence, doesn't block");

        src.push(&[9.0]);
        src.clear();
        out.clear();
        src.pull(2, &mut out);
        assert_eq!(out, vec![0.0, 0.0], "clear() drops anything buffered");
    }

    #[test]
    fn ppm_correction_shifts_frequency_proportionally() {
        // The empirically-measured correction from a real HackRF unit at
        // 462.5625 MHz FRS channel 1: about -8.9 ppm, i.e. roughly -4.1 kHz.
        let corrected = apply_ppm(462_562_500.0, -8.9);
        assert!(
            (corrected - 462_558_383.19).abs() < 0.01,
            "corrected {corrected}, expected ~462_558_383.19"
        );
    }

    #[test]
    fn zero_ppm_is_a_no_op() {
        assert_eq!(apply_ppm(137_100_000.0, 0.0), 137_100_000.0);
    }
}
