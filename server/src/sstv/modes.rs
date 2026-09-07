//! SSTV mode timing table. All times in seconds; the decoder works in the
//! 16 kHz audio domain (`crate::sstv::demod::AUDIO_RATE`).
//!
//! Values are the de-facto standard line/pixel/sync timings used by MMSSTV /
//! QSSTV / pySSTV. `sep` is the 1500 Hz "porch" gap between components.

/// How a transmit line's component samples become RGB rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Three components in order → one RGB row. `order` gives the R,G,B slot.
    Rgb { order: [u8; 3] },
    /// Robot 36: a full-res Y line, then a half-time R-Y (even) or B-Y (odd)
    /// line; the two chroma halves of a row pair reconstruct one RGB row per
    /// Y line (nearest-neighbour chroma).
    Robot36,
    /// PD family: Y(odd) R-Y B-Y Y(even) → two RGB rows from one transmit
    /// line (shared chroma).
    Pd,
}

#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub name: &'static str,
    pub vis: u8,
    pub width: usize,
    /// Output image height (RGB rows).
    pub height: usize,
    /// 1200 Hz sync pulse length.
    pub sync: f64,
    /// 1500 Hz separator / porch after sync and between components.
    pub sep: f64,
    /// Seconds per pixel within a component scan.
    pub pixel: f64,
    /// Nominal seconds from one sync's leading edge to the next.
    pub line: f64,
    pub color: Color,
    /// Sync comes *before* the first component (Martin, Robot, PD) vs.
    /// Scottie's oddball sync between component 2 and 3.
    pub scottie_sync: bool,
}

impl Mode {
    /// Component (sub-scan) count per transmit line.
    pub fn components(&self) -> usize {
        match self.color {
            Color::Rgb { .. } | Color::Robot36 => 3, // Robot36: Y + chroma + (unused slot); handled specially
            Color::Pd => 4,
        }
    }
}

#[allow(clippy::too_many_arguments)]
const fn m(
    name: &'static str,
    vis: u8,
    width: usize,
    height: usize,
    sync: f64,
    sep: f64,
    pixel: f64,
    line: f64,
    color: Color,
    scottie_sync: bool,
) -> Mode {
    Mode { name, vis, width, height, sync, sep, pixel, line, color, scottie_sync }
}

// Component slots as seen sync-to-sync by the decoder.
//   Scottie: [Sync][sep][R][sep][G][sep][B]  → slot0=R slot1=G slot2=B
//   Martin:  [Sync][sep][G][sep][B][sep][R]  → R=slot2 G=slot0 B=slot1
const SCOTTIE_RGB: Color = Color::Rgb { order: [0, 1, 2] };
const MARTIN_RGB: Color = Color::Rgb { order: [2, 0, 1] };

/// Every mode the decoder can render. `by_vis` / `by_key` look them up.
pub const MODES: &[Mode] = &[
    m("Scottie 1", 60, 320, 256, 0.009, 0.0015, 0.0004320, 0.42822, SCOTTIE_RGB, true),
    m("Scottie 2", 56, 320, 256, 0.009, 0.0015, 0.0002752, 0.27767, SCOTTIE_RGB, true),
    m("Scottie DX", 76, 320, 256, 0.009, 0.0015, 0.0010800, 1.05000, SCOTTIE_RGB, true),
    m("Martin 1", 44, 320, 256, 0.004862, 0.000572, 0.0004576, 0.446446, MARTIN_RGB, false),
    m("Martin 2", 40, 320, 256, 0.004862, 0.000572, 0.0002288, 0.226798, MARTIN_RGB, false),
    // Robot 36: SYNC, porch, Y(320), sep, chroma(320 @ half pixel time).
    m("Robot 36", 8, 320, 240, 0.009, 0.003, 0.0001375, 0.150, Color::Robot36, false),
    // PD: SYNC, porch, Y-odd, R-Y, B-Y, Y-even  (all full width, same pixel).
    m("PD 120", 95, 640, 496, 0.020, 0.00208, 0.00019024, 0.508480, Color::Pd, false),
    m("PD 180", 96, 640, 496, 0.020, 0.00208, 0.000286, 0.754240, Color::Pd, false),
];

pub fn by_vis(vis: u8) -> Option<&'static Mode> {
    MODES.iter().find(|m| m.vis == vis)
}

/// Match the `mode` mode-param string (`robot36`, `scottie1`, `pd120`, …).
pub fn by_key(key: &str) -> Option<&'static Mode> {
    let norm: String = key.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    MODES.iter().find(|m| {
        let mn: String = m.name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        mn == norm
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vis_lookup() {
        assert_eq!(by_vis(60).unwrap().name, "Scottie 1");
        assert_eq!(by_vis(44).unwrap().name, "Martin 1");
        assert_eq!(by_vis(8).unwrap().name, "Robot 36");
        assert_eq!(by_vis(95).unwrap().name, "PD 120");
        assert!(by_vis(200).is_none());
    }

    #[test]
    fn key_lookup() {
        assert_eq!(by_key("scottie1").unwrap().vis, 60);
        assert_eq!(by_key("Scottie DX").unwrap().vis, 76);
        assert_eq!(by_key("pd120").unwrap().vis, 95);
        assert!(by_key("nonsense").is_none());
    }
}
