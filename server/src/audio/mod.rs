//! Encoded-audio frame type shared between the DSP/tone pipeline and the
//! per-session WebRTC senders.

use bytes::Bytes;
use std::time::Duration;

/// One encoded Opus frame (typically 20 ms). PCM level is tracked separately
/// in the pipeline's telemetry, not carried per frame.
#[derive(Clone)]
pub struct AudioFrame {
    pub data: Bytes,
    pub duration: Duration,
}

/// RMS level of a mono f32 buffer in dBFS, floored at -120.
pub fn rms_dbfs(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return -120.0;
    }
    let sum_sq: f64 = pcm.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum_sq / pcm.len() as f64).sqrt();
    if rms <= 1e-6 {
        -120.0
    } else {
        (20.0 * rms.log10()).max(-120.0) as f32
    }
}
