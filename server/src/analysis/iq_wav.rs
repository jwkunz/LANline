//! IQ → WAV recorder: a 2-channel 16-bit PCM RIFF/WAVE file, interleaved as
//! `I, Q` per frame at the device sample rate. This is the de-facto SDR IQ
//! recording format — GQRX, SDR#, SDRuno and SDRangel all read it.
//!
//! The header's size fields are written last (backpatched on `finish`), so a
//! crash mid-recording still leaves a file whose data is intact, just with a
//! zero length that most tools recover from anyway.

use super::RecordingInfo;
use num_complex::Complex32;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HEADER_LEN: u64 = 44;

pub struct IqRecorder {
    file: io::BufWriter<File>,
    path: PathBuf,
    sample_rate_hz: u32,
    center_hz: f64,
    frames: u64,
    max_frames: u64,
    scratch: Vec<u8>,
}

impl IqRecorder {
    /// Create the file and write a placeholder header. `max_secs` caps the
    /// recording; `write` returns `Ok(true)` once the cap is reached.
    pub fn start(
        path: &Path,
        sample_rate_hz: u32,
        center_hz: f64,
        max_secs: f64,
    ) -> io::Result<Self> {
        let file = File::create(path)?;
        let mut w = io::BufWriter::new(file);
        write_header(&mut w, sample_rate_hz, 0)?;
        let max_frames = (sample_rate_hz as f64 * max_secs.max(0.1)).round() as u64;
        Ok(Self {
            file: w,
            path: path.to_path_buf(),
            sample_rate_hz,
            center_hz,
            frames: 0,
            max_frames,
            scratch: Vec::with_capacity(1 << 16),
        })
    }

    /// Append an IQ block. `Ok(true)` = the duration cap was hit and the
    /// caller should `finish()`.
    pub fn write(&mut self, iq: &[Complex32]) -> io::Result<bool> {
        let room = self.max_frames.saturating_sub(self.frames) as usize;
        let take = iq.len().min(room);
        self.scratch.clear();
        self.scratch.reserve(take * 4);
        for c in &iq[..take] {
            for v in [c.re, c.im] {
                let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                self.scratch.extend_from_slice(&s.to_le_bytes());
            }
        }
        self.file.write_all(&self.scratch)?;
        self.frames += take as u64;
        Ok(self.frames >= self.max_frames)
    }

    /// Backpatch the RIFF/`data` sizes, flush, and report what was written.
    pub fn finish(mut self) -> io::Result<RecordingInfo> {
        self.file.flush()?;
        let data_bytes = self.frames * 4; // 2 ch × i16
        let mut f = self.file.into_inner()?;
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&((HEADER_LEN - 8 + data_bytes) as u32).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&(data_bytes as u32).to_le_bytes())?;
        f.flush()?;
        Ok(RecordingInfo {
            filename: self
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("iq.wav")
                .to_string(),
            path: self.path.display().to_string(),
            bytes: HEADER_LEN + data_bytes,
            secs: self.frames as f64 / self.sample_rate_hz.max(1) as f64,
            sample_rate_hz: self.sample_rate_hz,
            center_hz: self.center_hz,
        })
    }
}

fn write_header(w: &mut impl Write, sample_rate: u32, data_bytes: u32) -> io::Result<()> {
    let channels: u16 = 2;
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    w.write_all(b"RIFF")?;
    w.write_all(&(HEADER_LEN as u32 - 8 + data_bytes).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // PCM fmt chunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}
