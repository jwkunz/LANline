//! APRS receive: 1200-baud AFSK on 144.390 MHz, AX.25/HDLC deframing, APRS
//! payload decode (position / MIC-E / status / message), a per-callsign
//! station track table, and a TNC2-format TCP monitor feed. Decode logic is
//! unit-tested without a radio; `crate::radio` drives it with live IQ.

pub mod ax25;
pub mod demod;
pub mod feed;
pub mod parse;
pub mod tracker;

pub use tracker::{Snapshot, Tracker};

use ax25::Ax25Frame;
use bytes::Bytes;
use parse::AprsData;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::broadcast;

/// APRS calling frequency (North America).
pub const APRS_HZ: f64 = 144_390_000.0;

pub const PACKET_RING: usize = 512;
const FEED_CHANNEL: usize = 4096;

/// Shared, lock-guarded APRS state: read by the REST handlers and the TNC2
/// feed, written by the decode pipeline thread.
pub struct AprsShared {
    base: Instant,
    pub tracker: Mutex<Tracker>,
    /// Recent decoded packets as TNC2 monitor lines (`SRC>DEST,PATH:info`).
    pub packets: Mutex<VecDeque<String>>,
    pub feed_tx: broadcast::Sender<Bytes>,
}

impl AprsShared {
    pub fn new() -> Self {
        let (feed_tx, _) = broadcast::channel(FEED_CHANNEL);
        Self {
            base: Instant::now(),
            tracker: Mutex::new(Tracker::default()),
            packets: Mutex::new(VecDeque::with_capacity(PACKET_RING)),
            feed_tx,
        }
    }

    pub fn now_s(&self) -> f64 {
        self.base.elapsed().as_secs_f64()
    }

    /// Record a validated AX.25 frame: parse the APRS payload, fold it into
    /// the tracker, keep the TNC2 line in the ring, and push it to the feed.
    pub fn record(&self, octets: &[u8], rssi_dbfs: f32) {
        let Some(frame) = Ax25Frame::parse(octets) else {
            return;
        };
        let data = AprsData::parse(&frame);
        self.tracker.lock().unwrap().ingest(&frame, &data, rssi_dbfs, self.now_s());

        let line = frame.to_tnc2();
        {
            let mut ring = self.packets.lock().unwrap();
            if ring.len() >= PACKET_RING {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        let _ = self.feed_tx.send(Bytes::from(format!("{line}\r\n")));
    }
}

impl Default for AprsShared {
    fn default() -> Self {
        Self::new()
    }
}
