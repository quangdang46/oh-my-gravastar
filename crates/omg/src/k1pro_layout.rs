//! K1 Pro (916_K1_PRO) physical key layout — Rust port of
//! `web/src/k1pro-layout.ts` (`K1_PRO_BLOCKS`), itself ported verbatim
//! from the vendor configurator's `keycodeConfig/keyboard_916_K1_PRO.json`
//! (docs/research/khub-webapp/). Used by `kb916 light gif` to map image
//! pixels onto the 126 cmd-6/cmd-8 LED slots.
//!
//! Coordinates are "key units" px (standard key = 44 px); `pos` is the
//! keymap/light domain position (`pos = 8 + n*4`, so slot `n =
//! (pos - 8) / 4`). `display:false` entries occupy keymap slots but are
//! not physical keys (media/system/aux positions).

/// One layout block. Only the fields the LED sampler needs are kept.
pub(crate) struct KeyBlock {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub display: bool,
    /// Vendor position `8 + slot_index*4`.
    pub pos: u16,
}

/// Slot-table index for a vendor `pos` (`pos = 8 + n*4`).
pub(crate) fn slot_index(pos: u16) -> usize {
    ((pos - 8) / 4) as usize
}

pub(crate) const K1_PRO_BLOCKS: &[KeyBlock] = &[
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 8,
    },
    KeyBlock {
        x: 0,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 12,
    },
    KeyBlock {
        x: 0,
        y: 102,
        width: 66,
        height: 44,
        display: true,
        pos: 16,
    },
    KeyBlock {
        x: 0,
        y: 148,
        width: 77,
        height: 44,
        display: true,
        pos: 20,
    },
    KeyBlock {
        x: 0,
        y: 193,
        width: 88,
        height: 44,
        display: true,
        pos: 24,
    },
    KeyBlock {
        x: 0,
        y: 239,
        width: 55,
        height: 44,
        display: true,
        pos: 28,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 32,
    },
    KeyBlock {
        x: 45,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 36,
    },
    KeyBlock {
        x: 68,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 40,
    },
    KeyBlock {
        x: 80,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 44,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 48,
    },
    KeyBlock {
        x: 58,
        y: 239,
        width: 55,
        height: 44,
        display: true,
        pos: 52,
    },
    KeyBlock {
        x: 91,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 56,
    },
    KeyBlock {
        x: 91,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 60,
    },
    KeyBlock {
        x: 113,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 64,
    },
    KeyBlock {
        x: 125,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 68,
    },
    KeyBlock {
        x: 92,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 72,
    },
    KeyBlock {
        x: 114,
        y: 239,
        width: 55,
        height: 44,
        display: true,
        pos: 76,
    },
    KeyBlock {
        x: 136,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 80,
    },
    KeyBlock {
        x: 137,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 84,
    },
    KeyBlock {
        x: 159,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 88,
    },
    KeyBlock {
        x: 170,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 92,
    },
    KeyBlock {
        x: 138,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 96,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 100,
    },
    KeyBlock {
        x: 182,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 104,
    },
    KeyBlock {
        x: 183,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 108,
    },
    KeyBlock {
        x: 205,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 112,
    },
    KeyBlock {
        x: 216,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 116,
    },
    KeyBlock {
        x: 184,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 120,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 124,
    },
    KeyBlock {
        x: 228,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 128,
    },
    KeyBlock {
        x: 228,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 132,
    },
    KeyBlock {
        x: 251,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 136,
    },
    KeyBlock {
        x: 261,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 140,
    },
    KeyBlock {
        x: 230,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 144,
    },
    KeyBlock {
        x: 171,
        y: 239,
        width: 286,
        height: 44,
        display: true,
        pos: 148,
    },
    KeyBlock {
        x: 296,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 152,
    },
    KeyBlock {
        x: 273,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 156,
    },
    KeyBlock {
        x: 296,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 160,
    },
    KeyBlock {
        x: 307,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 164,
    },
    KeyBlock {
        x: 275,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 168,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 172,
    },
    KeyBlock {
        x: 342,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 176,
    },
    KeyBlock {
        x: 319,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 180,
    },
    KeyBlock {
        x: 341,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 184,
    },
    KeyBlock {
        x: 353,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 188,
    },
    KeyBlock {
        x: 320,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 192,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 196,
    },
    KeyBlock {
        x: 387,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 200,
    },
    KeyBlock {
        x: 364,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 204,
    },
    KeyBlock {
        x: 386,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 208,
    },
    KeyBlock {
        x: 398,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 212,
    },
    KeyBlock {
        x: 366,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 216,
    },
    KeyBlock {
        x: 456,
        y: 239,
        width: 55,
        height: 44,
        display: true,
        pos: 220,
    },
    KeyBlock {
        x: 432,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 224,
    },
    KeyBlock {
        x: 410,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 228,
    },
    KeyBlock {
        x: 432,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 232,
    },
    KeyBlock {
        x: 443,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 236,
    },
    KeyBlock {
        x: 412,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 240,
    },
    KeyBlock {
        x: 513,
        y: 239,
        width: 55,
        height: 44,
        display: true,
        pos: 244,
    },
    KeyBlock {
        x: 500,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 248,
    },
    KeyBlock {
        x: 455,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 252,
    },
    KeyBlock {
        x: 478,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 256,
    },
    KeyBlock {
        x: 489,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 260,
    },
    KeyBlock {
        x: 457,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 264,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 268,
    },
    KeyBlock {
        x: 547,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 272,
    },
    KeyBlock {
        x: 501,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 276,
    },
    KeyBlock {
        x: 524,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 280,
    },
    KeyBlock {
        x: 535,
        y: 148,
        width: 44,
        height: 44,
        display: true,
        pos: 284,
    },
    KeyBlock {
        x: 503,
        y: 193,
        width: 44,
        height: 44,
        display: true,
        pos: 288,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 292,
    },
    KeyBlock {
        x: 592,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 296,
    },
    KeyBlock {
        x: 546,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 300,
    },
    KeyBlock {
        x: 570,
        y: 102,
        width: 44,
        height: 44,
        display: true,
        pos: 304,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 308,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 312,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 316,
    },
    KeyBlock {
        x: 638,
        y: 0,
        width: 44,
        height: 44,
        display: true,
        pos: 320,
    },
    KeyBlock {
        x: 592,
        y: 57,
        width: 88,
        height: 44,
        display: true,
        pos: 324,
    },
    KeyBlock {
        x: 615,
        y: 102,
        width: 66,
        height: 44,
        display: true,
        pos: 328,
    },
    KeyBlock {
        x: 581,
        y: 148,
        width: 99,
        height: 44,
        display: true,
        pos: 332,
    },
    KeyBlock {
        x: 548,
        y: 193,
        width: 88,
        height: 44,
        display: true,
        pos: 336,
    },
    KeyBlock {
        x: 592,
        y: 239,
        width: 44,
        height: 44,
        display: true,
        pos: 340,
    },
    KeyBlock {
        x: 890,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 344,
    },
    KeyBlock {
        x: 958,
        y: 228,
        width: 44,
        height: 44,
        display: false,
        pos: 348,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 352,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 356,
    },
    KeyBlock {
        x: 638,
        y: 192,
        width: 44,
        height: 44,
        display: true,
        pos: 360,
    },
    KeyBlock {
        x: 638,
        y: 239,
        width: 44,
        height: 44,
        display: true,
        pos: 364,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 368,
    },
    KeyBlock {
        x: 684,
        y: 57,
        width: 44,
        height: 44,
        display: true,
        pos: 372,
    },
    KeyBlock {
        x: 684,
        y: 101,
        width: 44,
        height: 44,
        display: true,
        pos: 376,
    },
    KeyBlock {
        x: 684,
        y: 145,
        width: 44,
        height: 44,
        display: true,
        pos: 380,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 384,
    },
    KeyBlock {
        x: 684,
        y: 239,
        width: 44,
        height: 44,
        display: true,
        pos: 388,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 392,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 396,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 400,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 404,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 408,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 412,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 416,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 420,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 424,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 428,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 432,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 436,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 440,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 444,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 448,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 452,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 456,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 460,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 464,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 468,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 472,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 476,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 480,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 484,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 488,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 492,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 496,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 500,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 504,
    },
    KeyBlock {
        x: 0,
        y: 0,
        width: 44,
        height: 44,
        display: false,
        pos: 508,
    },
];

/// Pixel bounds of the displayed keys (vendor `g()` / TS `layoutBounds`):
/// `max(x) + unit` over `display` blocks, where `unit` is the first
/// square key width (44).
pub(crate) fn layout_bounds() -> (u32, u32) {
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut unit = None;
    let mut any = false;
    for b in K1_PRO_BLOCKS.iter().filter(|b| b.display) {
        any = true;
        if unit.is_none() && b.width == b.height {
            unit = Some(b.width);
        }
        max_x = max_x.max(b.x);
        max_y = max_y.max(b.y);
    }
    if !any {
        return (0, 0);
    }
    (max_x + unit.unwrap_or(44), max_y + unit.unwrap_or(44))
}

/// Normalized LED sample point for one slot: centre (0..1 over layout
/// bounds) + region radii as fractions of canvas width/height. Port of
/// `k1proLedPoints()` in `web/src/proto/gifmap.ts` — the same
/// neighbourhood fraction the reference app's `O3` sampler uses
/// (0.35 × min(key width, one unit)). Non-physical slots get the same
/// bottom-edge fallback spread as the web port so aux positions still
/// receive colour instead of staying black.
#[derive(Clone, Copy)]
pub(crate) struct LedPoint {
    pub x: f64,
    pub y: f64,
    /// Region half-width as a fraction of canvas width.
    pub rx: f64,
    /// Region half-height as a fraction of canvas height.
    pub ry: f64,
}

const KEY_SLOTS: usize = 126;
const UNIT: f64 = 44.0;

pub(crate) fn led_points() -> Vec<LedPoint> {
    let (bw, bh) = layout_bounds();
    let bw = bw as f64;
    let bh = bh as f64;
    let mut pts: Vec<Option<LedPoint>> = vec![None; KEY_SLOTS];
    for b in K1_PRO_BLOCKS {
        if !b.display {
            continue;
        }
        let n = slot_index(b.pos);
        if n >= KEY_SLOTS {
            continue;
        }
        pts[n] = Some(LedPoint {
            x: (b.x as f64 + b.width as f64 / 2.0) / bw,
            y: (b.y as f64 + b.height as f64 / 2.0) / bh,
            rx: 0.35 * (b.width as f64).min(UNIT) / bw,
            ry: 0.35 * UNIT / bh,
        });
    }
    (0..KEY_SLOTS)
        .map(|i| {
            pts[i].unwrap_or(LedPoint {
                x: (i as f64 / (KEY_SLOTS - 1) as f64) * 0.9 + 0.05,
                y: 0.98,
                rx: 0.01,
                ry: 0.01,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_bounds_matches_web() {
        // max x = 684 (Del/PgUp/PgDn/right), max y = 239, unit = 44.
        assert_eq!(layout_bounds(), (728, 283));
    }

    #[test]
    fn led_points_cover_all_slots() {
        let pts = led_points();
        assert_eq!(pts.len(), KEY_SLOTS);
        // ESC (slot 0) centre of the top-left 44px key.
        assert!((pts[0].x - 22.0 / 728.0).abs() < 1e-9);
        assert!((pts[0].y - 22.0 / 283.0).abs() < 1e-9);
        // Space bar (slot 35, pos 148): rx uses min(width,44) so it
        // matches a normal key; only the centre differs.
        let space = pts[35];
        assert!((space.rx - pts[0].rx).abs() < 1e-12);
        assert!((space.x - (171.0 + 143.0) / 728.0).abs() < 1e-9);
        // A non-physical slot (e.g. index 6, pos 32) uses the fallback.
        assert!((pts[6].y - 0.98).abs() < 1e-9);
    }
}
