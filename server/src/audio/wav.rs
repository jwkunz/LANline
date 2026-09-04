//! Minimal 16-bit PCM WAV writer, for `--dump-wav` diagnostics.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

pub struct WavDump {
    file: File,
    frames: u32,
}

impl WavDump {
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> io::Result<Self> {
        let mut file = File::create(path)?;
        // 44-byte header with placeholder sizes, patched in `finalize`.
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;
        file.write_all(b"RIFF")?;
        file.write_all(&0u32.to_le_bytes())?; // riff size (patched)
        file.write_all(b"WAVE")?;
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?; // PCM
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?; // bits per sample
        file.write_all(b"data")?;
        file.write_all(&0u32.to_le_bytes())?; // data size (patched)
        Ok(Self { file, frames: 0 })
    }

    pub fn write(&mut self, pcm: &[f32]) {
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for &s in pcm {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        if self.file.write_all(&bytes).is_ok() {
            self.frames = self.frames.saturating_add(pcm.len() as u32);
        }
        // Re-patch the header roughly once a second so the file stays playable
        // even if the process is killed hard.
        if self.frames % 48_000 < pcm.len() as u32 {
            self.finalize();
        }
    }

    fn finalize(&mut self) {
        let data_size = self.frames * 2;
        let here = self.file.stream_position().unwrap_or(0);
        let _ = self.file.seek(SeekFrom::Start(4));
        let _ = self.file.write_all(&(36 + data_size).to_le_bytes());
        let _ = self.file.seek(SeekFrom::Start(40));
        let _ = self.file.write_all(&data_size.to_le_bytes());
        let _ = self.file.seek(SeekFrom::Start(here.max(44)));
        let _ = self.file.flush();
    }
}

impl Drop for WavDump {
    fn drop(&mut self) {
        self.finalize();
    }
}
