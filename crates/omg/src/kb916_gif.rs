//! `omg kb916 light gif <file>` — decode an image/animated GIF, place it
//! with the vendor Keyboard Studio transform, sample the 126 K1 Pro LED
//! slots and stream cmd-8 frames. Rust port of the web pipeline in
//! `web/src/music.ts` + `web/src/proto/gifmap.ts` (which mirror the
//! vendor bundle's `Yl`/`T3`/`R3`/`Yy`/`O3`/`M3`/`C3`/`fE` functions).
//!
//! Transport, pacing, snapshot and restore are shared with
//! `kb916 light stream` via [`crate::kb916_light::run_stream`]; this
//! module only produces the 378-byte frames.
//!
//! Verification level: code-verified against the web port and vendor
//! semantics; live-verified once on K1 Pro (still frame + `--pattern
//! check`), animated GIF path not yet exercised on hardware.

use anyhow::{anyhow, bail, Result};
use image::{AnimationDecoder, ImageReader};
use serde::Deserialize;
use std::io::Cursor;
use std::time::Duration;

use crate::k1pro_layout::{self, LedPoint};
use crate::{kb916_light, Cli};
use omg_proto_khub916::light as khub_light;

// ── Placement (vendor `Yl` defaults + `T3` clamps) ─────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Fit {
    #[default]
    Contain,
    Cover,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Sampling {
    #[default]
    Median,
    Point,
}

/// Persisted placement — vendor `Yl` shape. Session controls
/// (`--brightness`, `--speed`, play/pause) are deliberately NOT here,
/// matching the reference app.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Placement {
    pub fit: Fit,
    /// Percent, clamped 25–500.
    pub zoom: f64,
    /// Percent offsets of canvas size, clamped −250..250.
    pub x: f64,
    pub y: f64,
    pub mirror: bool,
    pub flip_y: bool,
    pub trim: bool,
    pub sampling: Sampling,
    /// 0–200 percent → saturate filter.
    pub saturation: f64,
    /// 0–96 black gate (max channel ≤ this → black).
    pub black_level: f64,
}

impl Default for Placement {
    /// Vendor `Yl` defaults.
    fn default() -> Self {
        Placement {
            fit: Fit::Contain,
            zoom: 100.0,
            x: 0.0,
            y: 0.0,
            mirror: false,
            flip_y: false,
            trim: false,
            sampling: Sampling::Median,
            saturation: 100.0,
            black_level: 24.0,
        }
    }
}

/// `T3` clamps. Returns the list of fields that were clamped (for the
/// warning message); mutates `p` in place.
fn clamp_placement(p: &mut Placement) -> Vec<&'static str> {
    let mut clamped = Vec::new();
    let mut do_clamp = |field: &'static str, v: f64, lo: f64, hi: f64| -> f64 {
        if v < lo || v > hi {
            clamped.push(field);
        }
        v.clamp(lo, hi)
    };
    p.zoom = do_clamp("zoom", p.zoom, 25.0, 500.0);
    p.x = do_clamp("x", p.x, -250.0, 250.0);
    p.y = do_clamp("y", p.y, -250.0, 250.0);
    p.saturation = do_clamp("saturation", p.saturation, 0.0, 200.0);
    p.black_level = do_clamp("blackLevel", p.black_level, 0.0, 96.0);
    clamped
}

// ── Clip decode ────────────────────────────────────────────────────────

const MAX_BYTES: u64 = 40 * 1024 * 1024;
const MAX_FRAMES: usize = 3000;
const MAX_PIXELS: u64 = 16_000_000;
/// Decode scale caps (port of the web `scale()`): longest side ≤ 640 px,
/// height ≤ 320, and total RGBA storage ≤ ~64 MB.
const CANVAS_W: u32 = 600;

pub(crate) struct GifClip {
    /// Scaled RGBA frames, `cw × ch`, RGBA8 interleaved.
    frames: Vec<Vec<u8>>,
    cw: u32,
    ch: u32,
    /// Cumulative frame end times in ms (`M3` input).
    ends: Vec<f64>,
    /// Content bounds `[x0,y0,x1,y1]` normalized over the frame (`C3`).
    bounds: [f64; 4],
    /// Human description for the pre-stream log line.
    desc: String,
}

fn detect_format(head: &[u8]) -> Result<image::ImageFormat> {
    let ascii =
        |n: usize| -> String { head.iter().take(n).map(|b| *b as char).collect::<String>() };
    if ascii(6).starts_with("GIF8") {
        Ok(image::ImageFormat::Gif)
    } else if head.len() >= 4
        && head[0] == 0x89
        && head[1] == 0x50
        && head[2] == 0x4e
        && head[3] == 0x47
    {
        Ok(image::ImageFormat::Png)
    } else if head.len() >= 2 && head[0] == 0xff && head[1] == 0xd8 {
        Ok(image::ImageFormat::Jpeg)
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Ok(image::ImageFormat::WebP)
    } else if head.len() >= 2 && &head[0..2] == b"BM" {
        Ok(image::ImageFormat::Bmp)
    } else {
        bail!("unsupported image — want GIF, PNG, JPG, WebP or BMP")
    }
}

/// Decode scale factor (web `scale()`): `min(1, 640/w, 320/h,
/// sqrt(64MB / (4·frames·w·h)))`.
fn decode_scale(w: u32, h: u32, frames: usize) -> f64 {
    let frames = frames.max(1) as f64;
    (1.0f64)
        .min(640.0 / w as f64)
        .min(320.0 / h as f64)
        .min((64.0 * 1024.0 * 1024.0 / (4.0 * frames * w as f64 * h as f64)).sqrt())
}

fn scaled_dims(w: u32, h: u32, t: f64) -> (u32, u32) {
    (
        ((w as f64 * t).floor() as u32).max(1),
        ((h as f64 * t).floor() as u32).max(1),
    )
}

/// Decode `path` into a [`GifClip`]. GIF goes through the `image` crate's
/// animation decoder (full-canvas composited RGBA8 frames, disposal
/// handled); stills decode once and loop as a single frame.
fn decode_clip(path: &str) -> Result<GifClip> {
    let bytes = if path == "-" {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)
            .map_err(|e| anyhow!("cannot read image from stdin: {e}"))?;
        buf
    } else {
        let meta = std::fs::metadata(path).map_err(|e| anyhow!("cannot stat '{path}': {e}"))?;
        if meta.len() > MAX_BYTES {
            bail!(
                "image must be smaller than 40 MB, got {} MB",
                meta.len() / (1024 * 1024)
            );
        }
        std::fs::read(path).map_err(|e| anyhow!("cannot read '{path}': {e}"))?
    };
    if bytes.len() as u64 > MAX_BYTES {
        bail!("image must be smaller than 40 MB");
    }
    if bytes.is_empty() {
        bail!("'{path}' is empty");
    }
    let format = detect_format(&bytes[..bytes.len().min(16)])?;

    if format == image::ImageFormat::Gif {
        decode_gif(&bytes, path)
    } else {
        decode_still(&bytes, path)
    }
}

/// First pass over an animated GIF: frame count, dims and delays, so the
/// decode scale can be computed before any pixels are kept. Second pass
/// re-decodes and resizes every frame (two-pass keeps memory bounded —
/// the scale factor depends on the frame count).
fn decode_gif(bytes: &[u8], path: &str) -> Result<GifClip> {
    let pass = || -> Result<(u32, u32, Vec<f64>)> {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
            .map_err(|e| anyhow!("'{path}': not a readable GIF: {e}"))?;
        let mut dims = (0u32, 0u32);
        let mut delays = Vec::new();
        for frame in decoder.into_frames() {
            let frame = frame.map_err(|e| anyhow!("'{path}': GIF decode failed: {e}"))?;
            if delays.is_empty() {
                dims = (frame.buffer().width(), frame.buffer().height());
            }
            // Vendor floor: delay → ms, minimum 20 ms.
            let (num, den) = frame.delay().numer_denom_ms();
            let ms = if den == 0 {
                100.0
            } else {
                num as f64 / den as f64
            };
            delays.push(ms.max(20.0));
            if delays.len() > MAX_FRAMES {
                bail!("GIF has more than {MAX_FRAMES} frames");
            }
        }
        if delays.is_empty() {
            bail!("'{path}': GIF has no frames");
        }
        Ok((dims.0, dims.1, delays))
    };
    let (w, h, delays) = pass()?;
    if w as u64 * h as u64 > MAX_PIXELS {
        bail!("image is too large — shrink it first ({}×{})", w, h);
    }

    let t = decode_scale(w, h, delays.len());
    let (cw, ch) = scaled_dims(w, h, t);

    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
        .map_err(|e| anyhow!("'{path}': not a readable GIF: {e}"))?;
    let mut frames = Vec::with_capacity(delays.len());
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| anyhow!("'{path}': GIF decode failed: {e}"))?;
        let img = frame.into_buffer();
        let small = if img.width() == cw && img.height() == ch {
            img
        } else {
            image::imageops::resize(&img, cw, ch, image::imageops::FilterType::Triangle)
        };
        frames.push(small.into_raw());
    }
    if frames.len() != delays.len() {
        bail!("'{path}': GIF frame count changed between decode passes");
    }

    let mut ends = Vec::with_capacity(delays.len());
    let mut acc = 0.0;
    for d in &delays {
        acc += d;
        ends.push(acc);
    }
    let bounds = content_bounds(&frames, cw, ch);
    Ok(GifClip {
        frames,
        cw,
        ch,
        ends,
        bounds,
        desc: format!(
            "{}×{}, {} frames, {:.0}×{:.0} sampled",
            w,
            h,
            delays.len(),
            cw,
            ch
        ),
    })
}

/// Still image → single looping frame.
fn decode_still(bytes: &[u8], path: &str) -> Result<GifClip> {
    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow!("'{path}': cannot decode image: {e}"))?
        .decode()
        .map_err(|e| anyhow!("'{path}': cannot decode image: {e}"))?;
    let (w, h) = (img.width(), img.height());
    if w as u64 * h as u64 > MAX_PIXELS {
        bail!("image is too large — shrink it first ({}×{})", w, h);
    }
    let rgba = img.to_rgba8();
    let t = decode_scale(w, h, 1);
    let (cw, ch) = scaled_dims(w, h, t);
    let small = if cw == w && ch == h {
        rgba
    } else {
        image::imageops::resize(&rgba, cw, ch, image::imageops::FilterType::Triangle)
    };
    let raw = small.into_raw();
    let frames = vec![raw];
    let bounds = content_bounds(&frames, cw, ch);
    Ok(GifClip {
        frames,
        cw,
        ch,
        ends: vec![1000.0],
        bounds,
        desc: format!("{}×{} still, {:.0}×{:.0} sampled", w, h, cw, ch),
    })
}

// ── Content bounds (`C3` port) ─────────────────────────────────────────

/// `[x0,y0,x1,y1]` normalized bounds of pixels differing from the corner
/// background by > tolerance/255, scanned over the first ≤8 frames.
/// Returns the full frame when content can't be isolated.
fn content_bounds(frames: &[Vec<u8>], w: u32, h: u32) -> [f64; 4] {
    if frames.is_empty() || w == 0 || h == 0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let px = |f: &Vec<u8>, x: u32, y: u32| -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [f[i], f[i + 1], f[i + 2]]
    };
    let bg = px(&frames[0], 0, 0);
    // Bail early if the four corners disagree — no clean background.
    for (cx, cy) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        let p = px(&frames[0], cx, cy);
        if (0..3).any(|i| (p[i] as i32 - bg[i] as i32).abs() > 12) {
            return [0.0, 0.0, 1.0, 1.0];
        }
    }
    let step = (w.min(h) / 200).max(1);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for f in frames.iter().take(8) {
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let p = px(f, x, y);
                let d = (p[0] as i32 - bg[0] as i32)
                    .abs()
                    .max((p[1] as i32 - bg[1] as i32).abs())
                    .max((p[2] as i32 - bg[2] as i32).abs());
                if d > 12 {
                    x0 = x0.min(x);
                    x1 = x1.max(x + 1);
                    y0 = y0.min(y);
                    y1 = y1.max(y + 1);
                }
                x += step;
            }
            y += step;
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    [
        x0 as f64 / w as f64,
        y0 as f64 / h as f64,
        x1 as f64 / w as f64,
        y1 as f64 / h as f64,
    ]
}

// ── Placement math (`R3`/`Yy` ports) ───────────────────────────────────

struct DestRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// Port of `R3`: destination rect for a source rect on a W×H canvas.
fn placement_rect(src_w: f64, src_h: f64, p: &Placement, w: f64, h: f64) -> DestRect {
    let d = match p.fit {
        Fit::Stretch => 1.0,
        Fit::Cover => (w / src_w).max(h / src_h),
        Fit::Contain => (w / src_w).min(h / src_h),
    };
    let w2 = (if p.fit == Fit::Stretch { w } else { src_w * d }) * (p.zoom / 100.0);
    let h2 = (if p.fit == Fit::Stretch { h } else { src_h * d }) * (p.zoom / 100.0);
    DestRect {
        x: (w - w2) / 2.0 + (p.x / 100.0) * w,
        y: (h - h2) / 2.0 + (p.y / 100.0) * h,
        w: w2,
        h: h2,
    }
}

/// CSS `saturate(s)` matrix on RGB (alpha untouched).
fn saturate(rgb: [f32; 3], s: f64) -> [f32; 3] {
    if (s - 1.0).abs() < 1e-9 {
        return rgb;
    }
    let s = s as f32;
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    [
        (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ]
}

/// Bilinear RGBA sample at fractional coords, clamped to the image.
fn sample_rgba(img: &[u8], w: u32, h: u32, x: f64, y: f64) -> [f32; 4] {
    let w = w as f64;
    let h = h as f64;
    let x = x.clamp(0.0, w - 1.0);
    let y = y.clamp(0.0, h - 1.0);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w as u32 - 1);
    let y1 = (y0 + 1).min(h as u32 - 1);
    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;
    let at = |x: u32, y: u32| -> [f32; 4] {
        let i = ((y * w as u32 + x) * 4) as usize;
        [
            img[i] as f32,
            img[i + 1] as f32,
            img[i + 2] as f32,
            img[i + 3] as f32,
        ]
    };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [0f32; 4];
    for c in 0..4 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bot = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bot - top) * fy;
    }
    out
}

// ── LED sampler (`O3` port) ────────────────────────────────────────────

/// Canvas-pixel-space sample region for one slot (precomputed — the
/// canvas size is fixed for the whole run).
#[derive(Clone, Copy)]
struct SamplePoint {
    cx: i64,
    cy: i64,
    x0: i64,
    x1: i64,
    y0: i64,
    y1: i64,
    /// `point` mode or a ≤4-pixel region → read the centre pixel only.
    point: bool,
}

fn sample_points(points: &[LedPoint], w: u32, h: u32, point_mode: bool) -> Vec<SamplePoint> {
    points
        .iter()
        .map(|p| {
            let cx = (p.x * w as f64).round().clamp(0.0, (w - 1) as f64) as i64;
            let cy = (p.y * h as f64).round().clamp(0.0, (h - 1) as f64) as i64;
            let rx = ((p.rx * w as f64).round() as i64).max(1);
            let ry = ((p.ry * h as f64).round() as i64).max(1);
            let x0 = (cx - rx).max(0);
            let x1 = (cx + rx + 1).min(w as i64);
            let y0 = (cy - ry).max(0);
            let y1 = (cy + ry + 1).min(h as i64);
            SamplePoint {
                cx,
                cy,
                x0,
                x1,
                y0,
                y1,
                point: point_mode || (x1 - x0) * (y1 - y0) <= 4,
            }
        })
        .collect()
}

fn channel_median(hist: &[u32; 256]) -> u8 {
    let total: u64 = hist.iter().map(|&v| v as u64).sum();
    let mid = (total.saturating_sub(1)) / 2;
    let mut acc = 0u64;
    for (i, &v) in hist.iter().enumerate() {
        acc += v as u64;
        if acc > mid {
            return i as u8;
        }
    }
    0
}

/// Sample one rendered RGBA canvas frame at every LED point →
/// interleaved 378-byte cmd-8 frame. Port of `sampleSlots` + the frame
/// packing in `renderGifFrame`.
fn sample_frame(
    canvas: &[u8],
    w: u32,
    points: &[SamplePoint],
    brightness: f64,
    black_level: f64,
) -> Vec<u8> {
    let brightness = brightness.clamp(0.0, 1.0) as f32;
    let black_level = black_level.clamp(0.0, 96.0) as i32;
    let mut out = vec![0u8; khub_light::GIF_FRAME_BYTES];
    let at = |x: i64, y: i64| -> [u8; 3] {
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        [canvas[i], canvas[i + 1], canvas[i + 2]]
    };
    for (s, p) in points.iter().enumerate() {
        let (mut ri, mut gi, mut bi) = (0u8, 0u8, 0u8);
        if p.point {
            let c = at(p.cx, p.cy);
            ri = c[0];
            gi = c[1];
            bi = c[2];
        } else {
            // Median per channel, then the in-region pixel nearest to it
            // (tie-break: nearest to region centre).
            let mut hr = [0u32; 256];
            let mut hg = [0u32; 256];
            let mut hb = [0u32; 256];
            for y in p.y0..p.y1 {
                for x in p.x0..p.x1 {
                    let c = at(x, y);
                    hr[c[0] as usize] += 1;
                    hg[c[1] as usize] += 1;
                    hb[c[2] as usize] += 1;
                }
            }
            let (mr, mg, mb) = (
                channel_median(&hr),
                channel_median(&hg),
                channel_median(&hb),
            );
            let mut best: Option<(i64, i64)> = None;
            let mut best_d = i64::MAX;
            let mut best_k = i64::MAX;
            for y in p.y0..p.y1 {
                for x in p.x0..p.x1 {
                    let c = at(x, y);
                    let dr = c[0] as i64 - mr as i64;
                    let dg = c[1] as i64 - mg as i64;
                    let db = c[2] as i64 - mb as i64;
                    let d = dr * dr + dg * dg + db * db;
                    let kx = x - p.cx;
                    let ky = y - p.cy;
                    let k = kx * kx + ky * ky;
                    if d < best_d || (d == best_d && k < best_k) {
                        best_d = d;
                        best_k = k;
                        best = Some((x, y));
                    }
                }
            }
            if let Some((x, y)) = best {
                let c = at(x, y);
                ri = c[0];
                gi = c[1];
                bi = c[2];
            }
        }
        let mx = ri.max(gi).max(bi) as i32;
        if mx <= black_level {
            continue; // stays [0,0,0]
        }
        let base = s * 3;
        for (i, c) in [ri, gi, bi].iter().enumerate() {
            let v = (*c as f32 / 255.0 * brightness).clamp(0.0, 1.0);
            out[base + i] = (v * 255.0).round() as u8;
        }
    }
    out
}

// ── Frame renderer (`Yy` + `M3` ports) ─────────────────────────────────

/// Port of `M3`: frame index for a position in cumulative `ends`.
fn frame_at(ends: &[f64], elapsed_ms: f64) -> usize {
    let Some(&total) = ends.last() else {
        return 0;
    };
    if total <= 0.0 {
        return 0;
    }
    let t = elapsed_ms.rem_euclid(total);
    // First index with t < ends[i] (binary search, web `gifFrameAt`).
    let mut lo = 0usize;
    let mut hi = ends.len() - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if t < ends[mid] {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// Live frame source plugged into `kb916_light::run_stream`: renders the
/// clip frame selected by wall-clock elapsed × speed, placed and sampled
/// onto the 126 slots.
pub(crate) struct GifStream {
    clip: GifClip,
    placement: Placement,
    points: Vec<SamplePoint>,
    canvas_w: u32,
    canvas_h: u32,
    brightness: f64,
    /// Session speed multiplier (elapsed ×= speed).
    speed: f64,
    /// Reused render buffer (canvas RGBA).
    canvas: Vec<u8>,
}

impl GifStream {
    pub fn new(clip: GifClip, placement: Placement, brightness: f64, speed: f64) -> Self {
        let (bw, bh) = k1pro_layout::layout_bounds();
        let canvas_w = CANVAS_W;
        // Web: H = round(600 * bounds.height / bounds.width).
        let canvas_h = ((CANVAS_W as f64 * bh as f64 / bw as f64).round() as u32).max(1);
        let points = k1pro_layout::led_points();
        let point_mode = placement.sampling == Sampling::Point;
        let points = sample_points(&points, canvas_w, canvas_h, point_mode);
        let canvas = vec![0u8; (canvas_w * canvas_h * 4) as usize];
        GifStream {
            clip,
            placement,
            points,
            canvas_w,
            canvas_h,
            brightness,
            speed,
            canvas,
        }
    }

    /// Render the frame at `elapsed` (wall time since stream start) →
    /// 378-byte cmd-8 frame.
    pub fn frame(&mut self, elapsed: Duration) -> Vec<u8> {
        let elapsed_ms = elapsed.as_secs_f64() * 1000.0 * self.speed;
        let idx = frame_at(&self.clip.ends, elapsed_ms);
        self.render(idx)
    }

    /// `Yy` port: black bg → optional trim src rect → placement dest
    /// rect → mirror/flipY → saturate → bilinear draw, composited over
    /// black. Result lands in `self.canvas` (RGBA, alpha 255).
    fn render(&mut self, idx: usize) -> Vec<u8> {
        let clip = &self.clip;
        let p = &self.placement;
        let (w, h) = (self.canvas_w, self.canvas_h);
        // Black background.
        for px in self.canvas.chunks_exact_mut(4) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 255;
        }
        // Source rect: full frame, or content bounds when `trim`.
        let [bx0, by0, bx1, by1] = if p.trim {
            clip.bounds
        } else {
            [0.0, 0.0, 1.0, 1.0]
        };
        let sx = bx0 * clip.cw as f64;
        let sy = by0 * clip.ch as f64;
        let sw = ((bx1 - bx0) * clip.cw as f64).max(1.0);
        let sh = ((by1 - by0) * clip.ch as f64).max(1.0);
        let r = placement_rect(sw, sh, p, w as f64, h as f64);
        let img = &clip.frames[idx.min(clip.frames.len() - 1)];

        // Mirror/flipY around the rect centre (Yy translate+scale port):
        // canvas local → pre-flip coord via sign, then map to src u,v.
        let sx_sign = if p.mirror { -1.0 } else { 1.0 };
        let sy_sign = if p.flip_y { -1.0 } else { 1.0 };
        let sat = p.saturation / 100.0;

        // Integer bounds of the dest rect ∩ canvas.
        let x_start = (r.x.floor() as i64).max(0);
        let x_end = ((r.x + r.w).ceil() as i64).min(w as i64);
        let y_start = (r.y.floor() as i64).max(0);
        let y_end = ((r.y + r.h).ceil() as i64).min(h as i64);
        if x_end <= x_start || y_end <= y_start {
            return sample_frame(
                &self.canvas,
                w,
                &self.points,
                self.brightness,
                p.black_level,
            );
        }
        let cx = r.x + r.w / 2.0;
        let cy = r.y + r.h / 2.0;
        for dy in y_start..y_end {
            for dx in x_start..x_end {
                // Pixel-centre sampling like canvas drawImage.
                let lx = dx as f64 + 0.5 - cx;
                let ly = dy as f64 + 0.5 - cy;
                let u = (lx * sx_sign) / r.w + 0.5;
                let v = (ly * sy_sign) / r.h + 0.5;
                if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                    continue; // outside the drawn image → stays black
                }
                let src_x = sx + u * sw;
                let src_y = sy + v * sh;
                let px = sample_rgba(img, clip.cw, clip.ch, src_x, src_y);
                // saturate filter applies to the source, then alpha
                // composites over the black background.
                let rgb = saturate([px[0], px[1], px[2]], sat);
                let a = px[3] / 255.0;
                let i = ((dy as u32 * w + dx as u32) * 4) as usize;
                self.canvas[i] = (rgb[0] * a).round().clamp(0.0, 255.0) as u8;
                self.canvas[i + 1] = (rgb[1] * a).round().clamp(0.0, 255.0) as u8;
                self.canvas[i + 2] = (rgb[2] * a).round().clamp(0.0, 255.0) as u8;
                self.canvas[i + 3] = 255;
            }
        }
        sample_frame(
            &self.canvas,
            w,
            &self.points,
            self.brightness,
            p.black_level,
        )
    }
}

// ── CLI ────────────────────────────────────────────────────────────────

pub(crate) struct GifArgs<'a> {
    pub file: &'a str,
    pub fit: Option<&'a str>,
    pub zoom: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub mirror: bool,
    pub flip_y: bool,
    pub trim: bool,
    pub sampling: Option<&'a str>,
    pub saturation: Option<f64>,
    pub black_level: Option<f64>,
    pub brightness: f64,
    pub speed: f64,
    pub placement_file: Option<&'a str>,
    pub fps: f64,
    pub duration: Option<f64>,
    pub frames: Option<u64>,
}

fn parse_fit(s: &str) -> Result<Fit> {
    match s {
        "contain" => Ok(Fit::Contain),
        "cover" => Ok(Fit::Cover),
        "stretch" => Ok(Fit::Stretch),
        other => bail!("unknown --fit '{other}' (want contain|cover|stretch)"),
    }
}

fn parse_sampling(s: &str) -> Result<Sampling> {
    match s {
        "median" => Ok(Sampling::Median),
        "point" => Ok(Sampling::Point),
        other => bail!("unknown --sampling '{other}' (want median|point)"),
    }
}

/// `omg kb916 light gif <file>` — decode + stream with snapshot/restore.
pub fn cmd(cli: &Cli, args: GifArgs<'_>) -> Result<std::process::ExitCode> {
    // ── Validate everything before touching the device ────────────────
    if !(1.0..=240.0).contains(&args.fps) || !args.fps.is_finite() {
        bail!("--fps must be in 1.0..=240.0, got {}", args.fps);
    }
    // 30 fps is the verified cmd-8 ceiling (AGENTS.md incident log);
    // warn only above it (unlike `light stream`'s conservative >10).
    if args.fps > 30.0 {
        eprintln!(
            "warning: --fps {} is above the verified 30 fps cmd-8 ceiling; \
             sustained streaming at this rate may wedge the keyboard \
             (AGENTS.md incident log)",
            args.fps
        );
    }
    if args.duration.is_some_and(|d| d <= 0.0) {
        bail!("--duration must be positive seconds");
    }
    if !(0.0..=100.0).contains(&args.brightness) || !args.brightness.is_finite() {
        bail!("--brightness must be in 0..=100, got {}", args.brightness);
    }
    if !(0.25..=3.0).contains(&args.speed) || !args.speed.is_finite() {
        bail!("--speed must be in 0.25..=3.0, got {}", args.speed);
    }

    // ── Placement: file → CLI flags → T3 clamps ───────────────────────
    let mut placement = match args.placement_file {
        Some(f) => {
            let text = if f == "-" {
                std::io::read_to_string(std::io::stdin())
                    .map_err(|e| anyhow!("cannot read placement from stdin: {e}"))?
            } else {
                std::fs::read_to_string(f)
                    .map_err(|e| anyhow!("cannot read placement '{f}': {e}"))?
            };
            serde_json::from_str::<Placement>(&text)
                .map_err(|e| anyhow!("invalid placement JSON '{f}': {e}"))?
        }
        None => Placement::default(),
    };
    if let Some(f) = args.fit {
        placement.fit = parse_fit(f)?;
    }
    if let Some(z) = args.zoom {
        placement.zoom = z;
    }
    if let Some(x) = args.x {
        placement.x = x;
    }
    if let Some(y) = args.y {
        placement.y = y;
    }
    if args.mirror {
        placement.mirror = true;
    }
    if args.flip_y {
        placement.flip_y = true;
    }
    if args.trim {
        placement.trim = true;
    }
    if let Some(s) = args.sampling {
        placement.sampling = parse_sampling(s)?;
    }
    if let Some(s) = args.saturation {
        placement.saturation = s;
    }
    if let Some(b) = args.black_level {
        placement.black_level = b;
    }
    let clamped = clamp_placement(&mut placement);
    if !clamped.is_empty() {
        eprintln!(
            "warning: placement values clamped to vendor ranges ({} )",
            clamped.join(", ")
        );
    }

    // ── Decode (before any device access) ─────────────────────────────
    let clip = decode_clip(args.file)?;
    let desc = clip.desc.clone();
    if !cli.json {
        println!(
            "gif: {} — {}, placement fit={:?} zoom={} sampling={:?}",
            args.file, desc, placement.fit, placement.zoom as i64, placement.sampling,
        );
    }

    let stream = GifStream::new(clip, placement, args.brightness / 100.0, args.speed);
    let source = kb916_light::FrameSource::Gif { stream };
    kb916_light::run_stream(
        cli,
        source,
        args.fps,
        args.duration,
        args.frames,
        Some(serde_json::json!({ "file": args.file, "clip": desc })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement() -> Placement {
        Placement::default()
    }

    #[test]
    fn placement_rect_contain_cover_stretch() {
        // W=600 H=233 (the real canvas for the K1 Pro layout).
        let p = placement();
        // contain, 100×100 src: d = min(6, 2.33) = 2.33 → 233×233 centred.
        let r = placement_rect(100.0, 100.0, &p, 600.0, 233.0);
        assert!((r.w - 233.0).abs() < 0.01 && (r.h - 233.0).abs() < 0.01);
        assert!((r.x - 183.5).abs() < 0.01 && (r.y - 0.0).abs() < 0.01);
        // cover: d = max(6, 2.33) = 6 → 600×600, vertically centred.
        let p = Placement {
            fit: Fit::Cover,
            ..placement()
        };
        let r = placement_rect(100.0, 100.0, &p, 600.0, 233.0);
        assert!((r.w - 600.0).abs() < 0.01 && (r.h - 600.0).abs() < 0.01);
        assert!((r.x - 0.0).abs() < 0.01 && (r.y + 183.5).abs() < 0.01);
        // stretch: exactly the canvas.
        let p = Placement {
            fit: Fit::Stretch,
            ..placement()
        };
        let r = placement_rect(100.0, 100.0, &p, 600.0, 233.0);
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 600.0, 233.0));
    }

    #[test]
    fn placement_rect_zoom_and_offset() {
        // contain 100×100 at zoom 200 → 466×466, x offset +50% → +300.
        let p = Placement {
            zoom: 200.0,
            x: 50.0,
            ..placement()
        };
        let r = placement_rect(100.0, 100.0, &p, 600.0, 233.0);
        assert!((r.w - 466.0).abs() < 0.01);
        assert!((r.x - (67.0 + 300.0)).abs() < 0.01);
    }

    #[test]
    fn placement_clamps_vendor_ranges() {
        let mut p = Placement {
            zoom: 999.0,
            x: -999.0,
            y: 42.0,
            saturation: 500.0,
            black_level: 200.0,
            ..placement()
        };
        let clamped = clamp_placement(&mut p);
        assert_eq!(p.zoom, 500.0);
        assert_eq!(p.x, -250.0);
        assert_eq!(p.y, 42.0);
        assert_eq!(p.saturation, 200.0);
        assert_eq!(p.black_level, 96.0);
        assert_eq!(clamped.len(), 4);
    }

    #[test]
    fn placement_json_vendor_shape() {
        // Vendor saved-placement shape (camelCase).
        let p: Placement = serde_json::from_str(
            r#"{"fit":"cover","zoom":150,"x":10,"y":-5,"mirror":true,
                "flipY":true,"trim":true,"sampling":"point",
                "saturation":80,"blackLevel":40}"#,
        )
        .unwrap();
        assert_eq!(p.fit, Fit::Cover);
        assert!(p.mirror && p.flip_y && p.trim);
        assert_eq!(p.sampling, Sampling::Point);
        assert_eq!(p.black_level, 40.0);
        // Empty object → vendor defaults.
        let p: Placement = serde_json::from_str("{}").unwrap();
        assert_eq!(p, placement());
    }

    #[test]
    fn frame_at_selects_by_cumulative_ends() {
        let ends = [100.0, 250.0, 600.0];
        assert_eq!(frame_at(&ends, 0.0), 0);
        assert_eq!(frame_at(&ends, 99.9), 0);
        assert_eq!(frame_at(&ends, 100.0), 1);
        assert_eq!(frame_at(&ends, 249.9), 1);
        assert_eq!(frame_at(&ends, 250.0), 2);
        assert_eq!(frame_at(&ends, 599.9), 2);
        // Loops: elapsed mod total.
        assert_eq!(frame_at(&ends, 600.0), 0);
        assert_eq!(frame_at(&ends, 700.0), 1);
        assert_eq!(frame_at(&ends, -50.0), 2);
        assert_eq!(frame_at(&[], 10.0), 0);
    }

    #[test]
    fn content_bounds_finds_bright_rect() {
        // 10×10 black frame with a white rect x∈[2,6) y∈[3,7).
        let (w, h) = (10u32, 10u32);
        let mut f = vec![0u8; (w * h * 4) as usize];
        for y in 3..7u32 {
            for x in 2..6u32 {
                let i = ((y * w + x) * 4) as usize;
                f[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let b = content_bounds(&[f], w, h);
        assert!((b[0] - 0.2).abs() < 1e-9);
        assert!((b[1] - 0.3).abs() < 1e-9);
        assert!((b[2] - 0.6).abs() < 1e-9);
        assert!((b[3] - 0.7).abs() < 1e-9);
    }

    #[test]
    fn content_bounds_uniform_returns_full() {
        let f = vec![7u8; 4 * 4 * 4];
        assert_eq!(content_bounds(&[f], 4, 4), [0.0, 0.0, 1.0, 1.0]);
    }

    fn solid_clip(w: u32, h: u32, rgb: [u8; 3]) -> GifClip {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for c in px.chunks_exact_mut(4) {
            c[0] = rgb[0];
            c[1] = rgb[1];
            c[2] = rgb[2];
            c[3] = 255;
        }
        GifClip {
            frames: vec![px],
            cw: w,
            ch: h,
            ends: vec![1000.0],
            bounds: [0.0, 0.0, 1.0, 1.0],
            desc: String::new(),
        }
    }

    #[test]
    fn stretch_solid_frame_lights_all_slots() {
        let clip = solid_clip(8, 8, [255, 0, 0]);
        let p = Placement {
            fit: Fit::Stretch,
            ..placement()
        };
        let mut s = GifStream::new(clip, p, 1.0, 1.0);
        let f = s.frame(Duration::ZERO);
        assert_eq!(f.len(), khub_light::GIF_FRAME_BYTES);
        // Every slot samples red (mx 255 > blackLevel 24, brightness 1).
        for slot in f.chunks_exact(3) {
            assert_eq!(slot, &[255, 0, 0]);
        }
    }

    #[test]
    fn sampler_black_level_and_brightness() {
        // 4×4 dark-grey canvas; one point covering the centre.
        let mut canvas = vec![0u8; 4 * 4 * 4];
        for c in canvas.chunks_exact_mut(4) {
            c[0] = 10;
            c[1] = 10;
            c[2] = 10;
            c[3] = 255;
        }
        let pts = vec![SamplePoint {
            cx: 2,
            cy: 2,
            x0: 0,
            x1: 4,
            y0: 0,
            y1: 4,
            point: false,
        }];
        // max channel 10 ≤ blackLevel 24 → black.
        let f = sample_frame(&canvas, 4, &pts, 1.0, 24.0);
        assert!(f.iter().all(|&b| b == 0));
        // blackLevel 0 → lit; brightness 0.5 → round(10/255·0.5·255)=5.
        let f = sample_frame(&canvas, 4, &pts, 0.5, 0.0);
        assert_eq!(&f[0..3], &[5, 5, 5]);
        // point mode ignores region size.
        let pts_point = vec![SamplePoint {
            point: true,
            ..pts[0]
        }];
        let f = sample_frame(&canvas, 4, &pts_point, 1.0, 0.0);
        assert_eq!(&f[0..3], &[10, 10, 10]);
    }

    #[test]
    fn sampler_median_picks_region_colour() {
        // 8×8 canvas, left half red right half blue; a wide region point
        // in median mode must pick one of the two (never a blend).
        let (w, h) = (8u32, 8u32);
        let mut canvas = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if x < 4 {
                    canvas[i..i + 3].copy_from_slice(&[255, 0, 0]);
                } else {
                    canvas[i..i + 3].copy_from_slice(&[0, 0, 255]);
                }
                canvas[i + 3] = 255;
            }
        }
        // Region straddling the boundary: median of each channel —
        // r median 0 or 255? hist r: 32×0? Actually half red half blue:
        // r: 32 px @255? no — left 32 px red (r=255), right 32 blue (r=0)
        // → median r: total 64, mid 31, acc@0 =32 >31 → 0. g all 0 → 0.
        // b: 32 @255? right half b=255 → acc@0=32>31 → 0? b hist: 32 px
        // @0 (left), 32 @255 (right) → median 0. So median=(0,0,0); the
        // nearest pixel to black is… distance ties everywhere? dr² for
        // red px =255², blue=255², neither is 0. Both equal → tie-break
        // nearest centre wins. Deterministic either way: result must be
        // pure red or pure blue, never purple.
        let pts = vec![SamplePoint {
            cx: 3,
            cy: 4,
            x0: 0,
            x1: 8,
            y0: 0,
            y1: 8,
            point: false,
        }];
        let f = sample_frame(&canvas, w, &pts, 1.0, 0.0);
        let c = &f[0..3];
        assert!(
            c == [255, 0, 0].as_slice() || c == [0, 0, 255].as_slice(),
            "got {c:?}"
        );
    }

    fn tiny_gif() -> Vec<u8> {
        // 8×8 two-frame GIF via the `gif` crate: red @100 ms, green @50 ms.
        let mut buf = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut buf, 8, 8, &[]).unwrap();
            enc.set_repeat(gif::Repeat::Infinite).unwrap();
            for (rgb, delay_cs) in [([255u8, 0, 0], 10u16), ([0, 255, 0], 5)] {
                let mut px = vec![0u8; 8 * 8 * 4];
                for c in px.chunks_exact_mut(4) {
                    c[0] = rgb[0];
                    c[1] = rgb[1];
                    c[2] = rgb[2];
                    c[3] = 255;
                }
                let mut frame = gif::Frame::from_rgba_speed(8, 8, &mut px, 10);
                frame.delay = delay_cs;
                enc.write_frame(&frame).unwrap();
            }
            // Trailer is written when the encoder drops.
        }
        buf
    }

    #[test]
    fn decode_gif_two_frames_delays() {
        let bytes = tiny_gif();
        let clip = decode_gif(&bytes, "tiny.gif").unwrap();
        assert_eq!(clip.frames.len(), 2);
        assert_eq!(clip.ends, vec![100.0, 150.0]);
        // Frame 0 is red, frame 1 green (composited full-canvas RGBA).
        assert_eq!(&clip.frames[0][0..4], &[255, 0, 0, 255]);
        assert_eq!(&clip.frames[1][0..4], &[0, 255, 0, 255]);
        // gifFrameAt timing: 120 ms lands in frame 1.
        assert_eq!(frame_at(&clip.ends, 120.0), 1);
        assert_eq!(frame_at(&clip.ends, 160.0), 0); // loops at 150 ms
    }

    #[test]
    fn decode_still_png_single_frame() {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([9, 8, 7, 255]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let clip = decode_still(buf.get_ref(), "t.png").unwrap();
        assert_eq!(clip.frames.len(), 1);
        assert_eq!(clip.ends, vec![1000.0]);
        assert_eq!(&clip.frames[0][0..4], &[9, 8, 7, 255]);
    }

    #[test]
    fn gif_stream_speed_scales_elapsed() {
        // Two frames: red [0,100ms), green [100,200ms).
        let mut red = solid_clip(8, 8, [255, 0, 0]);
        let green = solid_clip(8, 8, [0, 255, 0]);
        red.frames.push(green.frames.into_iter().next().unwrap());
        red.ends = vec![100.0, 200.0];
        let p = Placement {
            fit: Fit::Stretch,
            ..placement()
        };
        // speed 2.0: 60 ms wall → 120 ms clip time → green frame.
        let mut s = GifStream::new(red, p, 1.0, 2.0);
        let f = s.frame(Duration::from_millis(60));
        assert_eq!(&f[0..3], &[0, 255, 0]);
        // 10 ms wall → 20 ms → red.
        let f = s.frame(Duration::from_millis(10));
        assert_eq!(&f[0..3], &[255, 0, 0]);
    }
}
