//! AIS (marine 1090-equivalent: 161.975 / 162.025 MHz Class A/B) receive:
//! two-channel GMSK demod, HDLC framing, ITU-R M.1371 message decode, and a
//! per-MMSI vessel track table. Decode logic is unit-tested without a radio;
//! `crate::radio` drives it with live IQ.

pub mod demod;
pub mod message;
pub mod nmea;
pub mod tracker;

pub use tracker::{Snapshot, Tracker};

use bytes::Bytes;
use message::{AisMessage, Bits};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::broadcast;

/// AIS channel centre frequencies.
pub const CHANNEL_A_HZ: f64 = 161_975_000.0;
pub const CHANNEL_B_HZ: f64 = 162_025_000.0;
/// Tune here so both channels sit at ±25 kHz, clear of the ZIF DC spike.
pub const CENTER_HZ: f64 = 162_000_000.0;

pub const SENTENCE_RING: usize = 512;
const NMEA_CHANNEL: usize = 4096;

/// Shared, lock-guarded AIS state: read by the REST handlers and the AIVDM
/// feed, written by the decode pipeline thread.
pub struct AisShared {
    base: Instant,
    pub tracker: Mutex<Tracker>,
    pub sentences: Mutex<VecDeque<String>>,
    pub nmea_tx: broadcast::Sender<Bytes>,
}

impl AisShared {
    pub fn new() -> Self {
        let (nmea_tx, _) = broadcast::channel(NMEA_CHANNEL);
        Self {
            base: Instant::now(),
            tracker: Mutex::new(Tracker::default()),
            sentences: Mutex::new(VecDeque::with_capacity(SENTENCE_RING)),
            nmea_tx,
        }
    }

    pub fn now_s(&self) -> f64 {
        self.base.elapsed().as_secs_f64()
    }

    /// Record a validated frame: parse + fold into the tracker, keep the
    /// AIVDM sentence in the ring, and push it to the feed.
    pub fn record(&self, f: &demod::RawSentence) {
        let sentence = nmea::aivdm(&f.payload, f.bit_len, f.channel);

        if let Some(msg) = AisMessage::parse(&Bits::new(&f.payload, f.bit_len)) {
            self.tracker.lock().unwrap().ingest(&msg, f.rssi_dbfs, self.now_s());
        }

        {
            let mut ring = self.sentences.lock().unwrap();
            if ring.len() >= SENTENCE_RING {
                ring.pop_front();
            }
            ring.push_back(sentence.trim_end().to_string());
        }

        if self.nmea_tx.receiver_count() > 0 {
            let _ = self.nmea_tx.send(Bytes::from(sentence));
        }
    }

    pub fn recent_sentences(&self) -> Vec<String> {
        self.sentences.lock().unwrap().iter().cloned().collect()
    }
}

impl Default for AisShared {
    fn default() -> Self {
        Self::new()
    }
}
