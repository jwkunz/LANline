//! The receive pipeline and its lifecycle.
//!
//! `debug_tone` mode synthesizes a 440 Hz A4; `nbfm`/`wbfm`/`am` modes open the
//! selected SoapySDR device and run one of the [`dsp`] analog chains (FM or
//! AM, picked per mode — see `Chain`); `adsb`/`ais` run their own decoders
//! with no audio output; any other mode emits digital silence. The analog
//! modes produce 20 ms Opus frames on a broadcast channel that each WebRTC
//! session subscribes to.

#[cfg(feature = "soapy")]
pub mod dsp;

use crate::adsb::AdsbShared;
use crate::ais::AisShared;
use crate::audio::{rms_dbfs, AudioFrame};
use crate::model::RadioConfig;
use crate::registry::DeviceRegistry;
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use bytes::Bytes;
#[cfg(feature = "soapy")]
use dsp::{AmChain, AmParams, FmChain, FmParams};
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
    pub overruns: u64,
    pub source: &'static str,
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
    Fm(FmChain),
    Am(AmChain),
}

#[cfg(feature = "soapy")]
impl Chain {
    fn new(device_rate: f64, demod: DemodParams) -> Self {
        match demod {
            DemodParams::Fm(p) => Chain::Fm(FmChain::new(device_rate, p)),
            DemodParams::Am(p) => Chain::Am(AmChain::new(device_rate, p)),
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
        })
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

        #[cfg(feature = "soapy")]
        let (cmd_tx, cmd_rx) = mpsc::channel::<PipelineCmd>();
        #[cfg(not(feature = "soapy"))]
        let cmd_rx = ();

        let handle = std::thread::Builder::new()
            .name("rx-pipeline".into())
            .spawn(move || run_pipeline(params, tx, tele, stop_thread, dump, cmd_rx, adsb, ais))
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
            || old.audio.frame_ms != new.audio.frame_ms
            || old.audio.sample_rate_hz != new.audio.sample_rate_hz
            || old.audio.opus_bitrate_bps != new.audio.opus_bitrate_bps
            || (!matches!(new.mode.as_str(), "nbfm" | "wbfm" | "am") && old.mode_params != new.mode_params);

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
                    "nbfm" | "wbfm" => cmds.push(PipelineCmd::Demod(DemodParams::Fm(fm_params(new)))),
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
    reference: Option<(f64, f64)>,
    max_range_nm: f64,
    trail_secs: f64,
    forget_secs: f64,
}

#[cfg(feature = "soapy")]
struct SdrParams {
    soapy_args: String,
    device_rate: f64,
    freq_hz: f64,
    channel: usize,
    antenna: Option<String>,
    agc: bool,
    gain_overall_db: Option<f64>,
    gain_elements_db: Vec<(String, f64)>,
    dc_offset: bool,
    settings: Vec<(String, String)>,
    demod: DemodParams,
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
            "nbfm" | "wbfm" | "am" => {
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
                            reference,
                            max_range_nm: param("max_range_nm", 60.0),
                            trail_secs: param("trail_seconds", 600.0),
                            forget_secs: param("forget_seconds", 900.0),
                        }))
                    }
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

fn run_pipeline(
    p: PipelineParams,
    tx: broadcast::Sender<AudioFrame>,
    telemetry: Arc<Mutex<Telemetry>>,
    stop: Arc<AtomicBool>,
    dump: Option<PathBuf>,
    cmd_rx: CmdRx,
    adsb: Arc<AdsbShared>,
    ais: Arc<AisShared>,
) {
    let _ = (&adsb, &ais);
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
            run_synth(Synth::Tone { hz, amp }, fc, &tx, &telemetry, &stop, wav.as_mut())
        }
        SourceKind::Silence => {
            let _ = &cmd_rx;
            run_synth(Synth::Silence, fc, &tx, &telemetry, &stop, wav.as_mut())
        }
        #[cfg(feature = "soapy")]
        SourceKind::Sdr(sdr) => {
            run_sdr(*sdr, fc, &tx, &telemetry, &stop, wav.as_mut(), cmd_rx)
        }
        #[cfg(feature = "soapy")]
        SourceKind::Adsb(p) => run_adsb(*p, adsb, &telemetry, &stop, cmd_rx),
        #[cfg(feature = "soapy")]
        SourceKind::Ais(p) => run_ais(*p, ais, &telemetry, &stop, cmd_rx),
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
    cmd_rx: mpsc::Receiver<PipelineCmd>,
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
    let lo = sp.freq_hz - sp.demod.lo_offset_hz();
    log_set("frequency", dev.set_frequency(dir, ch, lo, ""));
    if let Some(ant) = &sp.antenna {
        log_set("antenna", dev.set_antenna(dir, ch, ant.as_str()));
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
    if sp.dc_offset {
        let _ = dev.set_dc_offset_mode(dir, ch, true);
    }
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

    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PipelineCmd::Retune(hz) => {
                    let new_lo = hz - sp.demod.lo_offset_hz();
                    log_set("frequency", dev.set_frequency(dir, ch, new_lo, ""));
                    chain.on_retune();
                    tracing::info!("{label}: retuned to {:.4} MHz (live)", hz / 1e6);
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

        while acc.len() >= fc.frame_samples {
            let pcm: Vec<f32> = acc.drain(..fc.frame_samples).collect();
            if let Some(w) = wav.as_deref_mut() {
                w.write(&pcm);
            }
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
                t.overruns = overruns;
            }
        }
    }

    let _ = stream.deactivate(None);
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
    log_set("frequency", dev.set_frequency(dir, ch, p.freq_hz, ""));
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
    log_set("frequency", dev.set_frequency(dir, ch, CENTER_HZ, ""));
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
