//! Text-to-speech: shell out to `espeak-ng` (default) or `piper`
//! (`--tts-voice <model.onnx>`), returning 48 kHz mono f32 PCM ready for the
//! TX path. No Rust engine dependency — one of those binaries must be on
//! `PATH`.

use crate::radio::dsp::LinearResampler;
use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone)]
pub struct TtsCfg {
    /// Piper voice model (`*.onnx`). `None` → espeak-ng.
    pub voice: Option<PathBuf>,
    /// espeak-ng speaking rate, words/min (80–450).
    pub rate_wpm: u32,
}

impl TtsCfg {
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Self { voice: cfg.tts_voice.clone(), rate_wpm: 155 }
    }

    /// The backend name, for logs / the API response.
    pub fn backend(&self) -> &'static str {
        if self.voice.is_some() {
            "piper"
        } else {
            "espeak-ng"
        }
    }
}

/// Synthesize `text` → 48 kHz mono PCM. Rejects empty / oversize input.
pub fn synthesize(text: &str, cfg: &TtsCfg) -> Result<Vec<f32>> {
    let text = text.trim();
    if text.is_empty() {
        bail!("empty text");
    }
    if text.len() > 1000 {
        bail!("text too long (max 1000 chars)");
    }

    let (pcm, sr) = match &cfg.voice {
        Some(model) => piper(text, model)?,
        None => espeak(text, cfg.rate_wpm)?,
    };

    if (sr - 48_000).abs() < 1 {
        return Ok(pcm);
    }
    let mut r = LinearResampler::new(sr as f64, 48_000.0);
    let mut out = Vec::with_capacity(pcm.len() * 48_000 / sr.max(1) as usize + 8);
    r.process(&pcm, &mut out);
    Ok(out)
}

/// `espeak-ng -s <rate> --stdout <text>` → a WAV (22050 Hz s16 mono).
fn espeak(text: &str, rate_wpm: u32) -> Result<(Vec<f32>, i32)> {
    let out = Command::new("espeak-ng")
        .args(["-s", &rate_wpm.clamp(80, 450).to_string(), "--stdout", text])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("running `espeak-ng` (is it installed / on PATH?)")?;
    if !out.status.success() {
        bail!("espeak-ng failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_wav(&out.stdout)
}

/// `piper --model <onnx> --output_raw` reads text on stdin, writes raw s16le
/// mono at the model's sample rate (from the sidecar `<onnx>.json`).
fn piper(text: &str, model: &std::path::Path) -> Result<(Vec<f32>, i32)> {
    let sr = piper_sample_rate(model).unwrap_or(22_050);
    let mut child = Command::new("piper")
        .args(["--model", &model.to_string_lossy(), "--output_raw"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `piper` (is it installed / on PATH?)")?;
    child
        .stdin
        .take()
        .context("piper stdin")?
        .write_all(text.as_bytes())
        .context("writing text to piper")?;
    let out = child.wait_with_output().context("waiting on piper")?;
    if !out.status.success() {
        bail!("piper failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok((s16le_to_f32(&out.stdout), sr))
}

fn piper_sample_rate(model: &std::path::Path) -> Option<i32> {
    let json = std::fs::read_to_string(model.with_extension("onnx.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    v.get("audio")?.get("sample_rate")?.as_i64().map(|n| n as i32)
}

fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}


/// Minimal RIFF/WAVE reader: find the `data` chunk, assume PCM s16 mono.
fn parse_wav(bytes: &[u8]) -> Result<(Vec<f32>, i32)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV stream ({} bytes)", bytes.len());
    }
    let mut sr = 22_050i32;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            sr = i32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
        } else if id == b"data" {
            let end = (body + len).min(bytes.len());
            return Ok((s16le_to_f32(&bytes[body..end]), sr));
        }
        pos = body + len + (len & 1); // chunks are word-aligned
    }
    bail!("WAV has no data chunk")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_espeak() -> bool {
        Command::new("espeak-ng").arg("--version").output().is_ok()
    }

    #[test]
    fn espeak_synthesizes_48k_pcm() {
        if !has_espeak() {
            eprintln!("skipping: espeak-ng not on PATH");
            return;
        }
        let cfg = TtsCfg { voice: None, rate_wpm: 155 };
        let pcm = synthesize("radio check one two", &cfg).unwrap();
        // A few words at 48 kHz is well over a tenth of a second.
        assert!(pcm.len() > 48_000 / 10, "got {} samples", pcm.len());
        assert!(pcm.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(pcm.iter().any(|s| s.abs() > 0.01), "silent output");
    }

    #[test]
    fn parse_wav_reads_a_data_chunk() {
        // 8 s16 samples at 22050
        let mut w = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0".to_vec();
        w.extend_from_slice(&22_050u32.to_le_bytes());
        w.extend_from_slice(&[0u8; 8]); // byte_rate(4) + block_align(2) + bits(2) — 16-byte fmt body
        w.extend_from_slice(b"data");
        w.extend_from_slice(&16u32.to_le_bytes());
        for i in 0..8i16 {
            w.extend_from_slice(&(i * 1000).to_le_bytes());
        }
        let (pcm, sr) = parse_wav(&w).unwrap();
        assert_eq!(sr, 22_050);
        assert_eq!(pcm.len(), 8);
        assert!((pcm[1] - 1000.0 / 32768.0).abs() < 1e-6);
    }
}
