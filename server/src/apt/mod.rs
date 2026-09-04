//! NOAA APT (137 MHz weather satellite) receive: FM/subcarrier demod (see
//! [`demod`]) into a growing two-channel grayscale image, exported as a small
//! binary raster (`GET /api/v1/apt/image`) the web client renders to canvas.

pub mod demod;

use bytes::Bytes;
use demod::{Line, IMAGE_LEN};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Combined width: channel A + channel B images, side by side.
pub const IMAGE_WIDTH: usize = IMAGE_LEN * 2;

pub struct Image {
    rows: VecDeque<Vec<u8>>,
    max_rows: usize,
    total_lines: u64,
    last_sync_quality: f32,
}

impl Image {
    fn new(max_rows: usize) -> Self {
        Self { rows: VecDeque::new(), max_rows: max_rows.max(1), total_lines: 0, last_sync_quality: 0.0 }
    }

    pub fn push_line(&mut self, line: &Line) {
        let mut row = Vec::with_capacity(IMAGE_WIDTH);
        row.extend_from_slice(&line.image_a);
        row.extend_from_slice(&line.image_b);
        self.rows.push_back(row);
        while self.rows.len() > self.max_rows {
            self.rows.pop_front();
        }
        self.total_lines += 1;
        self.last_sync_quality = line.sync_quality;
    }

    /// `[u32 width LE][u32 height LE][row-major grayscale bytes]`. The client
    /// reads this straight into a canvas `ImageData` — no image crate needed.
    pub fn encode(&self) -> Bytes {
        let height = self.rows.len();
        let mut buf = Vec::with_capacity(8 + IMAGE_WIDTH * height);
        buf.extend_from_slice(&(IMAGE_WIDTH as u32).to_le_bytes());
        buf.extend_from_slice(&(height as u32).to_le_bytes());
        for row in &self.rows {
            buf.extend_from_slice(row);
        }
        Bytes::from(buf)
    }

    pub fn status(&self) -> Status {
        Status {
            width: IMAGE_WIDTH,
            height: self.rows.len(),
            lines: self.total_lines,
            sync_quality: (self.last_sync_quality * 100.0).round() / 100.0,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub width: usize,
    pub height: usize,
    pub lines: u64,
    pub sync_quality: f32,
}

/// Shared, lock-guarded APT image, written by the decode pipeline thread and
/// read by the REST handlers.
pub struct AptShared {
    pub image: Mutex<Image>,
}

impl AptShared {
    pub fn new() -> Self {
        Self { image: Mutex::new(Image::new(1200)) }
    }

    /// Reset for a fresh capture (called when the `apt` pipeline (re)starts).
    pub fn reset(&self, max_rows: usize) {
        *self.image.lock().unwrap() = Image::new(max_rows);
    }
}

impl Default for AptShared {
    fn default() -> Self {
        Self::new()
    }
}
