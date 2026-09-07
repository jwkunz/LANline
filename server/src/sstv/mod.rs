//! SSTV (slow-scan TV) receive: RF demod (FM / SSB) → audio subcarrier
//! discriminator → VIS header → per-mode line decode (see [`demod`]) into a
//! growing RGB image, exported as a small binary raster
//! (`GET /api/v1/sstv/image`) the web client renders to canvas — the same
//! shape as [`crate::apt`].

pub mod demod;
pub mod encode;
pub mod modes;

use bytes::Bytes;
use demod::Row;
use serde::Serialize;
use std::sync::Mutex;

/// One in-progress or finished decoded picture.
pub struct Image {
    width: usize,
    height: usize,
    /// `height` rows of `width` RGBA pixels; unfilled rows stay transparent.
    px: Vec<[u8; 4]>,
    mode: &'static str,
    line: usize,
    frames: u64,
    snr_db: f32,
    locked: bool,
}

impl Image {
    fn new() -> Self {
        Self {
            width: 320,
            height: 256,
            px: vec![[0, 0, 0, 0]; 320 * 256],
            mode: "—",
            line: 0,
            frames: 0,
            snr_db: -40.0,
            locked: false,
        }
    }

    /// Fold in one decoded row. A row from a new mode/geometry, or the first
    /// row of a fresh frame, resizes/clears the canvas.
    pub fn push_row(&mut self, row: &Row) {
        if row.width != self.width || row.height != self.height {
            self.width = row.width;
            self.height = row.height;
            self.px = vec![[0, 0, 0, 0]; self.width * self.height];
        } else if row.y == 0 {
            self.px.iter_mut().for_each(|p| *p = [0, 0, 0, 0]);
        }
        self.mode = row.mode;
        self.line = row.y + 1;
        if row.y < self.height {
            let off = row.y * self.width;
            for (i, rgb) in row.rgb.iter().take(self.width).enumerate() {
                self.px[off + i] = [rgb[0], rgb[1], rgb[2], 255];
            }
        }
        if row.frame_done {
            self.frames += 1;
        }
    }

    pub fn set_meta(&mut self, snr_db: f32, locked: bool) {
        self.snr_db = snr_db;
        self.locked = locked;
    }

    /// `[u32 width LE][u32 height LE][row-major RGBA bytes]` — read straight
    /// into a canvas `ImageData`, no image crate.
    pub fn encode(&self) -> Bytes {
        let mut buf = Vec::with_capacity(8 + self.px.len() * 4);
        buf.extend_from_slice(&(self.width as u32).to_le_bytes());
        buf.extend_from_slice(&(self.height as u32).to_le_bytes());
        for p in &self.px {
            buf.extend_from_slice(p);
        }
        Bytes::from(buf)
    }

    pub fn status(&self) -> Status {
        Status {
            width: self.width,
            height: self.height,
            mode: self.mode.to_string(),
            line: self.line,
            frames: self.frames,
            snr_db: (self.snr_db * 10.0).round() / 10.0,
            locked: self.locked,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub width: usize,
    pub height: usize,
    pub mode: String,
    pub line: usize,
    pub frames: u64,
    pub snr_db: f32,
    pub locked: bool,
}

/// Shared, lock-guarded SSTV image — written by the decode pipeline thread,
/// read by the REST handlers.
pub struct SstvShared {
    pub image: Mutex<Image>,
}

impl SstvShared {
    pub fn new() -> Self {
        Self { image: Mutex::new(Image::new()) }
    }
    /// Fresh capture (called when the `sstv` pipeline (re)starts).
    pub fn reset(&self) {
        *self.image.lock().unwrap() = Image::new();
    }
}

impl Default for SstvShared {
    fn default() -> Self {
        Self::new()
    }
}
