//! ADS-B (1090 MHz Extended Squitter) receive: PPM demod, Mode S framing, CPR
//! position solving, and an aircraft track table. Pure decode logic lives here
//! and is unit-tested without a radio; `crate::radio` drives it with live IQ.

pub mod beast;
pub mod cpr;
pub mod demod;
pub mod message;
pub mod tracker;

pub use tracker::{Snapshot, Tracker};

use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::broadcast;

/// How many recent raw frames (hex) `GET /api/v1/adsb/messages` keeps.
pub const HEX_RING: usize = 512;
const BEAST_CHANNEL: usize = 4096;

/// Shared, lock-guarded ADS-B state: read by the REST handlers and the Beast
/// TCP feed, written by the decode pipeline thread. All timestamps derive from
/// `base`, so the pipeline and the API agree on "now".
pub struct AdsbShared {
    base: Instant,
    pub tracker: Mutex<Tracker>,
    pub hex: Mutex<VecDeque<String>>,
    pub beast_tx: broadcast::Sender<Bytes>,
}

impl AdsbShared {
    pub fn new() -> Self {
        let (beast_tx, _) = broadcast::channel(BEAST_CHANNEL);
        Self {
            base: Instant::now(),
            tracker: Mutex::new(Tracker::default()),
            hex: Mutex::new(VecDeque::with_capacity(HEX_RING)),
            beast_tx,
        }
    }

    /// Monotonic seconds since process start — the tracker's time base.
    pub fn now_s(&self) -> f64 {
        self.base.elapsed().as_secs_f64()
    }

    /// Record a freshly decoded frame: tracker, hex ring, and Beast feed.
    pub fn record(&self, f: &demod::RawFrame) {
        let now_s = self.now_s();
        self.tracker.lock().unwrap().ingest(&f.decoded, f.rssi_dbfs, now_s);

        {
            let mut hex = self.hex.lock().unwrap();
            if hex.len() >= HEX_RING {
                hex.pop_front();
            }
            hex.push_back(to_hex(&f.bytes));
        }

        // Only encode/send when a feed client is attached.
        if self.beast_tx.receiver_count() > 0 {
            let clk = (now_s * 12_000_000.0) as u64;
            let _ = self.beast_tx.send(beast::encode(&f.bytes, f.rssi_dbfs, clk));
        }
    }

    pub fn recent_hex(&self) -> Vec<String> {
        self.hex.lock().unwrap().iter().cloned().collect()
    }
}

impl Default for AdsbShared {
    fn default() -> Self {
        Self::new()
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}
