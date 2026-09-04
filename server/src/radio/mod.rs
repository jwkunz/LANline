//! The receive pipeline and its lifecycle.
//!
//! Phase 1b: the pipeline synthesizes audio (a 440 Hz tone in `debug_tone`
//! mode, digital silence otherwise), Opus-encodes 20 ms frames, and fans them
//! out on a broadcast channel that each WebRTC session subscribes to. The real
//! SoapySDR + DSP source replaces the synth in phase 1c.

use crate::audio::{rms_dbfs, AudioFrame};
use crate::model::RadioConfig;
use audiopus::coder::Encoder;
use audiopus::{Application, Bitrate, Channels, SampleRate};
use bytes::Bytes;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const BROADCAST_CAPACITY: usize = 64;
const OPUS_MAX_FRAME: usize = 4000;

#[derive(Default, Clone)]
pub struct Telemetry {
    pub frames_sent: u64,
    pub audio_level_dbfs: Option<f32>,
    pub squelch_open: bool,
    pub source: &'static str,
}

pub struct RadioManager {
    cfg: Arc<Mutex<RadioConfig>>,
    tx: broadcast::Sender<AudioFrame>,
    run: Mutex<RunState>,
    telemetry: Arc<Mutex<Telemetry>>,
}

#[derive(Default)]
struct RunState {
    running: bool,
    stop: Option<Arc<AtomicBool>>,
    handle: Option<JoinHandle<()>>,
}

impl RadioManager {
    pub fn new(cfg: Arc<Mutex<RadioConfig>>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            cfg,
            tx,
            run: Mutex::new(RunState::default()),
            telemetry: Arc::new(Mutex::new(Telemetry::default())),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AudioFrame> {
        self.tx.subscribe()
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
        let params = PipelineParams::from_config(&self.cfg.lock().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let tele = self.telemetry.clone();
        *self.telemetry.lock().unwrap() = Telemetry::default();

        let stop_thread = stop.clone();
        let handle = std::thread::Builder::new()
            .name("audio-pipeline".into())
            .spawn(move || run_pipeline(params, tx, tele, stop_thread))
            .expect("spawn audio-pipeline thread");

        run.stop = Some(stop);
        run.handle = Some(handle);
        run.running = true;
        tracing::info!("pipeline started");
    }

    pub fn stop(&self) {
        let (stop, handle) = {
            let mut run = self.run.lock().unwrap();
            run.running = false;
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
}

enum Source {
    Tone { hz: f64, amp: f32 },
    Silence,
}

struct PipelineParams {
    source: Source,
    sample_rate: u32,
    frame_samples: usize,
    frame: Duration,
    bitrate_bps: i32,
}

impl PipelineParams {
    fn from_config(cfg: &RadioConfig) -> Self {
        let sample_rate = cfg.audio.sample_rate_hz.max(8000);
        let frame_ms = cfg.audio.frame_ms.clamp(10, 60);
        let frame_samples = (sample_rate as usize / 1000) * frame_ms as usize;

        let source = if cfg.mode == "debug_tone" {
            let hz = param(cfg, "tone_hz", 440.0).clamp(20.0, 20_000.0);
            let level_dbfs = param(cfg, "level_dbfs", -12.0).clamp(-60.0, -1.0);
            Source::Tone { hz, amp: 10f64.powf(level_dbfs / 20.0) as f32 }
        } else {
            Source::Silence
        };

        Self {
            source,
            sample_rate,
            frame_samples,
            frame: Duration::from_millis(frame_ms as u64),
            bitrate_bps: cfg.audio.opus_bitrate_bps as i32,
        }
    }
}

fn param(cfg: &RadioConfig, key: &str, default: f64) -> f64 {
    cfg.mode_params.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn run_pipeline(
    p: PipelineParams,
    tx: broadcast::Sender<AudioFrame>,
    telemetry: Arc<Mutex<Telemetry>>,
    stop: Arc<AtomicBool>,
) {
    let mut encoder = match Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Audio) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("opus encoder init failed: {e}");
            return;
        }
    };
    if let Err(e) = encoder.set_bitrate(Bitrate::BitsPerSecond(p.bitrate_bps)) {
        tracing::warn!("opus set_bitrate failed: {e}");
    }

    {
        let mut t = telemetry.lock().unwrap();
        t.source = match p.source {
            Source::Tone { .. } => "debug_tone",
            Source::Silence => "silence",
        };
        t.squelch_open = matches!(p.source, Source::Tone { .. });
    }

    let mut pcm = vec![0f32; p.frame_samples];
    let mut buf = vec![0u8; OPUS_MAX_FRAME];
    let mut phase = 0f64;
    let dphase = match p.source {
        Source::Tone { hz, .. } => TAU * hz / p.sample_rate as f64,
        Source::Silence => 0.0,
    };

    let mut next = Instant::now() + p.frame;
    while !stop.load(Ordering::SeqCst) {
        match p.source {
            Source::Tone { amp, .. } => {
                for s in pcm.iter_mut() {
                    *s = phase.sin() as f32 * amp;
                    phase += dphase;
                    if phase >= TAU {
                        phase -= TAU;
                    }
                }
            }
            Source::Silence => pcm.iter_mut().for_each(|s| *s = 0.0),
        }

        let rms = rms_dbfs(&pcm);
        match encoder.encode_float(&pcm, &mut buf) {
            Ok(n) => {
                let _ = tx.send(AudioFrame {
                    data: Bytes::copy_from_slice(&buf[..n]),
                    duration: p.frame,
                });
                let mut t = telemetry.lock().unwrap();
                t.frames_sent = t.frames_sent.wrapping_add(1);
                t.audio_level_dbfs = Some(rms);
            }
            Err(e) => tracing::warn!("opus encode failed: {e}"),
        }

        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += p.frame;
        if next < now {
            next = now + p.frame; // fell behind; resync
        }
    }
}
