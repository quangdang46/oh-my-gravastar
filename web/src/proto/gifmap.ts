/**
 * GIF/image → per-key RGB frame sampler for the khub916 cmd-8 stream —
 * the web port of Keyboard Studio's preview pipeline (layout table →
 * normalized LED points → median/point sampling → 126×RGB frame).
 *
 * DOM-free core so `scripts/test-gifmap.ts` exercises it under Node;
 * the page glue (ImageDecoder, canvas transforms, HID writes) lives in
 * `../music.ts`.
 *
 * Pipeline mirrors the reference app:
 *   1. Placement transform (fit/zoom/x/y/mirror/flipY/trim) is applied
 *      by the caller when rendering the frame to a canvas; this module
 *      only sees the resulting pixels.
 *   2. `contentBounds` finds the non-background content rect once per
 *      clip (corner-colour diff scan, port of their `C3`).
 *   3. `sampleSlots` reads each LED point's neighbourhood: `median`
 *      mode takes the per-channel median of the region then picks the
 *      pixel nearest to it (robust against outlines/text); `point`
 *      reads the centre pixel. `blackLevel` gates near-black to [0,0,0],
 *      `brightness` scales linearly.
 */

import { K1_PRO_BLOCKS, layoutBounds, keyIndexOf } from '../k1pro-layout.ts';
import type { KeyPoint } from './pixelart.ts';

export const KEY_SLOTS = 126;

/**
 * Normalized LED sample point: centre (0..1 over layout bounds) + region
 * radii as fractions of canvas width/height. Mirrors the reference
 * sampler `O3`: x-radius = 0.35 × min(key width, one unit) of the grid
 * width, y-radius = 0.35 × one unit row of the grid height.
 */
export interface LedPoint {
  x: number;
  y: number;
  /** Region half-width as a fraction of canvas width. */
  rx: number;
  /** Region half-height as a fraction of canvas height. */
  ry: number;
}

export interface SampleOptions {
  /** 'median' = robust region colour (default); 'point' = centre pixel. */
  sampling?: 'median' | 'point';
  /** Linear brightness scale 0..1 (default 1). */
  brightness?: number;
  /** Max-channel gate: ≤ this → black (default 24, like the reference). */
  blackLevel?: number;
}

const UNIT = 44; // standard key size in layout units

/**
 * Per-slot LED sample points for the K1 Pro, derived from the vendor
 * layout table (`K1_PRO_BLOCKS`): slot `n` (`pos = 8 + n*4`) → key rect
 * centre normalised over the displayed-key bounds, radius ≈ 0.35 × key
 * pitch — the same neighbourhood fraction the reference app uses.
 * Non-physical slots get a linear spread like `k1proGeometry()`.
 */
export function k1proLedPoints(): LedPoint[] {
  const bounds = layoutBounds(K1_PRO_BLOCKS);
  const pts: (LedPoint | null)[] = new Array(KEY_SLOTS).fill(null);
  for (const b of K1_PRO_BLOCKS) {
    if (!b.display) continue;
    const n = keyIndexOf(b.pos);
    if (n < 0 || n >= KEY_SLOTS) continue;
    pts[n] = {
      x: (b.x + b.width / 2) / bounds.width,
      y: (b.y + b.height / 2) / bounds.height,
      rx: (0.35 * Math.min(b.width, UNIT)) / bounds.width,
      ry: (0.35 * UNIT) / bounds.height,
    };
  }
  // Non-physical slots: spread along the bottom edge so aux positions
  // still receive colour instead of staying black.
  return pts.map(
    (p, i) =>
      p ?? {
        x: (i / (KEY_SLOTS - 1)) * 0.9 + 0.05,
        y: 0.98,
        rx: 0.01,
        ry: 0.01,
      },
  );
}

/** Slot index → layout KeyPoint (centre, un-normalised px) for preview. */
export function k1proSlotPoints(): KeyPoint[] {
  const pts: (KeyPoint | null)[] = new Array(KEY_SLOTS).fill(null);
  for (const b of K1_PRO_BLOCKS) {
    const n = keyIndexOf(b.pos);
    if (n < 0 || n >= KEY_SLOTS || !b.display) continue;
    pts[n] = { x: b.x + b.width / 2, y: b.y + b.height / 2 };
  }
  return pts.map((p, i) => p ?? { x: (i / (KEY_SLOTS - 1)) * 16.5, y: 3 });
}

export type Rgba = { data: Uint8ClampedArray | Uint8Array; width: number; height: number };

/**
 * Content bounding box `[x0, y0, x1, y1]` (normalized) of pixels that
 * differ from the corner background colour — port of the reference
 * `C3`. Scans up to `maxFrames` frames worth of rows; returns the full
 * frame when content can't be isolated. `tolerance` ≈ their 12/255.
 */
export function contentBounds(
  frames: readonly Rgba[],
  tolerance = 12,
): [number, number, number, number] {
  if (frames.length === 0) return [0, 0, 1, 1];
  const w = frames[0].width;
  const h = frames[0].height;
  // Corner colour = first frame's top-left pixel (their `s` sample).
  const d0 = frames[0].data;
  const bg = [d0[0], d0[1], d0[2]];
  const corners = [0, (w - 1) * 4, (h - 1) * w * 4, (w * h - 1) * 4];
  // Bail early if the four corners disagree — no clean background.
  for (const c of corners) {
    const p = frames[0].data;
    if (bg.some((v, i) => Math.abs((p[c + i] ?? 0) - v) > tolerance)) {
      return [0, 0, 1, 1];
    }
  }
  let x0 = w;
  let y0 = h;
  let x1 = 0;
  let y1 = 0;
  const step = Math.max(1, Math.floor(Math.min(w, h) / 200)); // bound the scan
  for (const f of frames.slice(0, 8)) {
    const d = f.data;
    for (let y = 0; y < h; y += step) {
      for (let x = 0; x < w; x += step) {
        const i = (y * w + x) * 4;
        const r = d[i] ?? 0;
        const g = d[i + 1] ?? 0;
        const b = d[i + 2] ?? 0;
        if (
          Math.max(Math.abs(r - bg[0]), Math.abs(g - bg[1]), Math.abs(b - bg[2])) >
          tolerance
        ) {
          if (x < x0) x0 = x;
          if (x + 1 > x1) x1 = x + 1;
          if (y < y0) y0 = y;
          if (y + 1 > y1) y1 = y + 1;
        }
      }
    }
  }
  if (x1 <= x0 || y1 <= y0) return [0, 0, 1, 1];
  return [x0 / w, y0 / h, x1 / w, y1 / h];
}

function channelMedian(hist: Uint32Array): number {
  let acc = 0;
  const mid = Math.floor((hist.reduce((s, v) => s + v, 0) - 1) / 2);
  for (let i = 0; i < 256; i++) {
    acc += hist[i] ?? 0;
    if (acc > mid) return i;
  }
  return 0;
}

/**
 * Sample one frame's RGBA pixels at every LED point → interleaved
 * linear-RGB `Float32Array` of `3 * KEY_SLOTS` (same layout
 * `packPixelFrame` consumes). Coordinates are normalised 0..1 over the
 * rendered canvas — the caller's placement transform has already been
 * baked into the pixels.
 */
export function sampleSlots(
  img: Rgba,
  points: readonly LedPoint[],
  opts: SampleOptions = {},
): Float32Array {
  const { width: w, height: h, data } = img;
  const sampling = opts.sampling ?? 'median';
  const brightness = Math.min(1, Math.max(0, opts.brightness ?? 1));
  const blackLevel = Math.min(96, Math.max(0, opts.blackLevel ?? 24));
  const out = new Float32Array(points.length * 3);

  for (let s = 0; s < points.length; s++) {
    const p = points[s];
    const cx = Math.min(w - 1, Math.max(0, Math.round(p.x * w)));
    const cy = Math.min(h - 1, Math.max(0, Math.round(p.y * h)));
    const rx = Math.max(1, Math.round(p.rx * w));
    const ry = Math.max(1, Math.round(p.ry * h));
    const x0 = Math.max(0, cx - rx);
    const x1 = Math.min(w, cx + rx + 1);
    const y0 = Math.max(0, cy - ry);
    const y1 = Math.min(h, cy + ry + 1);

    let ri = 0;
    let gi = 0;
    let bi = 0;
    if (sampling === 'point' || (x1 - x0) * (y1 - y0) <= 4) {
      const i = (cy * w + cx) * 4;
      ri = data[i] ?? 0;
      gi = data[i + 1] ?? 0;
      bi = data[i + 2] ?? 0;
    } else {
      // Median per channel, then the in-region pixel nearest to it —
      // port of the reference `O3` median branch.
      const hr = new Uint32Array(256);
      const hg = new Uint32Array(256);
      const hb = new Uint32Array(256);
      for (let y = y0; y < y1; y++) {
        for (let x = x0; x < x1; x++) {
          const i = (y * w + x) * 4;
          hr[data[i] ?? 0] = (hr[data[i] ?? 0] ?? 0) + 1;
          hg[data[i + 1] ?? 0] = (hg[data[i + 1] ?? 0] ?? 0) + 1;
          hb[data[i + 2] ?? 0] = (hb[data[i + 2] ?? 0] ?? 0) + 1;
        }
      }
      const mr = channelMedian(hr);
      const mg = channelMedian(hg);
      const mb = channelMedian(hb);
      let best = -1;
      let bestD = Infinity;
      let bestK = Infinity;
      for (let y = y0; y < y1; y++) {
        for (let x = x0; x < x1; x++) {
          const i = (y * w + x) * 4;
          const dr = (data[i] ?? 0) - mr;
          const dg = (data[i + 1] ?? 0) - mg;
          const db = (data[i + 2] ?? 0) - mb;
          const d = dr * dr + dg * dg + db * db;
          const k = (x - cx) * (x - cx) + (y - cy) * (y - cy);
          if (d < bestD || (d === bestD && k < bestK)) {
            bestD = d;
            bestK = k;
            best = i;
          }
        }
      }
      if (best >= 0) {
        ri = data[best] ?? 0;
        gi = data[best + 1] ?? 0;
        bi = data[best + 2] ?? 0;
      }
    }
    const mx = Math.max(ri, gi, bi);
    if (mx <= blackLevel) continue; // stays [0,0,0]
    out[s * 3] = (ri / 255) * brightness;
    out[s * 3 + 1] = (gi / 255) * brightness;
    out[s * 3 + 2] = (bi / 255) * brightness;
  }
  return out;
}
