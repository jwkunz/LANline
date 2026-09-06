//! API-triggered recording of the demodulated 48 kHz mono audio to a WAV
//! file, for any mode that produces audio (nbfm/wbfm/am/frs/ham/debug_tone).
//!
//! The `--dump-wav` startup flag dumps the whole session; this is the
//! per-request version — start/stop over REST, a hard length cap, and a
//! download endpoint — mirroring `analysis`'s IQ recorder but for mono
//! audio. The pipeline thread calls [`AudioRecorder::write`] every 20 ms
//! frame; start/stop and status come from the API layer.

use crate::audio::wav::WavDump;
use crate::model::AudioRecInfo;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const AUDIO_RATE: u32 = 48_000;

struct Active {
    wav: WavDump,
    path: PathBuf,
    filename: String,
    frames: u64,
    max_frames: u64,
    mode: String,
    frequency_hz: u64,
}

impl Active {
    fn info(&self) -> AudioRecInfo {
        AudioRecInfo {
            path: self.path.display().to_string(),
            filename: self.filename.clone(),
            bytes: 44 + self.frames * 2,
            secs: self.frames as f64 / AUDIO_RATE as f64,
            sample_rate_hz: AUDIO_RATE,
            mode: self.mode.clone(),
            frequency_hz: self.frequency_hz,
        }
    }
}

/// `RadioManager`-owned recorder state. Shared between the API layer and the
/// pipeline thread.
pub struct AudioRecorder {
    active: Mutex<Option<Active>>,
    last: Mutex<Option<AudioRecInfo>>,
    dir: PathBuf,
}

impl AudioRecorder {
    pub fn new(dir: PathBuf) -> Self {
        Self { active: Mutex::new(None), last: Mutex::new(None), dir }
    }

    pub fn is_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    pub fn last(&self) -> Option<AudioRecInfo> {
        self.last.lock().unwrap().clone()
    }

    /// Begin a recording. `max_secs` is clamped to 1 s..1 h.
    pub fn start(&self, mode: &str, frequency_hz: u64, max_secs: f64) -> io::Result<AudioRecInfo> {
        let mut g = self.active.lock().unwrap();
        if g.is_some() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already recording"));
        }
        let stamp: String = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default()
            .chars()
            .filter(|c| !matches!(c, ':' | '-'))
            .collect();
        let filename =
            format!("lanline-audio-{mode}-{:.4}MHz-{stamp}.wav", frequency_hz as f64 / 1e6);
        let path = self.dir.join(&filename);
        let wav = WavDump::create(&path, AUDIO_RATE, 1)?;
        let max_frames = (max_secs.clamp(1.0, 3600.0) * AUDIO_RATE as f64) as u64;
        let active = Active {
            wav,
            path,
            filename,
            frames: 0,
            max_frames,
            mode: mode.to_string(),
            frequency_hz,
        };
        let info = active.info();
        *g = Some(active);
        Ok(info)
    }

    /// Stop and finalize any active recording, recording it as `last`.
    /// Returns the finished info, or `None` if nothing was recording.
    pub fn stop(&self) -> Option<AudioRecInfo> {
        let mut g = self.active.lock().unwrap();
        let active = g.take()?;
        let info = active.info();
        drop(active); // WavDump::drop backpatches the header
        *self.last.lock().unwrap() = Some(info.clone());
        Some(info)
    }

    /// Feed one 20 ms PCM frame from the pipeline. Auto-stops (recording
    /// `last`) once the length cap is hit; a no-op when nothing is recording.
    pub fn write(&self, pcm: &[f32]) {
        let mut g = self.active.lock().unwrap();
        let Some(active) = g.as_mut() else { return };
        active.wav.write(pcm);
        active.frames += pcm.len() as u64;
        if active.frames >= active.max_frames {
            let done = g.take().unwrap();
            let info = done.info();
            drop(done);
            *self.last.lock().unwrap() = Some(info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_starts_writes_and_auto_stops_at_the_cap() {
        let dir = std::env::temp_dir().join(format!("lanline-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec = AudioRecorder::new(dir.clone());

        // 1 s cap (the API floor); 960-sample (20 ms) frames -> ~50 to fill.
        let info = rec.start("nbfm", 162_550_000, 1.0).unwrap();
        assert!(info.filename.contains("nbfm") && info.filename.contains("162.5500MHz"));
        assert!(rec.is_active());
        assert!(rec.start("nbfm", 1, 1.0).is_err(), "second start must be rejected");

        let frame = [0.1f32; 960];
        for _ in 0..70 {
            rec.write(&frame);
        }
        assert!(!rec.is_active(), "cap should have auto-stopped it");
        let last = rec.last().expect("last recording recorded");
        assert!((last.secs - 1.0).abs() < 0.05, "capped near 1.0s, got {}", last.secs);
        // 44-byte header + 2 bytes/sample; ~1 s * 48 kHz.
        assert!(last.bytes >= 44 + 47_000 * 2 && last.bytes <= 44 + 49_000 * 2, "{}", last.bytes);
        let meta = std::fs::metadata(&last.path).unwrap();
        assert_eq!(meta.len(), last.bytes, "reported size matches the file on disk");

        // Explicit stop of a fresh recording.
        rec.start("ham", 146_520_000, 60.0).unwrap();
        rec.write(&frame);
        let stopped = rec.stop().unwrap();
        assert!(stopped.mode == "ham" && stopped.secs > 0.0);
        assert!(rec.stop().is_none(), "stop with nothing active is a no-op");

        std::fs::remove_dir_all(&dir).ok();
    }
}
