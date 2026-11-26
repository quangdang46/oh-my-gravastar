/**
 * Music page — mic-reactive keyboard lighting via khub916 cmd-8 frame
 * streaming (bead oh-my-gravastar-8r8).
 *
 * Pipeline mirrors `omg kb916 light music` (crates/omg/src/kb916_music.rs)
 * and the `omg-music` crate, adapted to the browser:
 *
 *   getUserMedia({audio}) → AudioContext → AnalyserNode(fftSize 2048)
 *   → getByteFrequencyData → MusicAnalyzer (log bands + beat)
 *   → SpectrumViz → 378-byte interleaved frame → session.writeGifFrame
 *   at ~30 ms pacing (GIF_FRAME_PERIOD_MS).
 *
 * The same page also hosts **pixel art** (proto/pixelart.ts, port of
 * crates/music/src/pixelart.rs): text/bitmap → per-slot RGB → the same
 * cmd-8 stream, no microphone involved.
 *
 * Restore contract (same as the CLI): on start we snapshot the profile
 * blob (cmd 132) and the per-mode light-data blob (cmd 138); on stop —
 * user, navigation, or disconnect — we write both back with the same
 * verified `writeProfile`/`writeLightData` paths the Lighting page uses.
 *
 * Only the microphone is available in a browser page (no system audio);
 * the UI says so plainly.
 */

import { app, emit, setStatus, subscribe } from './state.ts';
import { K1_PRO_BLOCKS, keyIndexOf, layoutBounds } from './k1pro-layout.ts';
import {
  PixelArt,
  RGB_WHITE,
  type KeyPoint,
  type Motion,
  type PaletteName,
  type Rgb,
} from './proto/pixelart.ts';
import {
  DEFAULT_FRAME_PARAMS,
  GIF_FRAME_BYTES,
  GIF_FRAME_PERIOD_MS,
  KEY_SLOTS,
  MusicAnalyzer,
  SpectrumViz,
  silentBands,
  type Bands,
  type FrameParams,
} from './proto/music.ts';
import {
  contentBounds,
  k1proLedPoints,
  k1proSlotPoints,
  sampleSlots,
  type LedPoint,
  type Rgba,
} from './proto/gifmap.ts';

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = '',
  text = '',
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text) e.textContent = text;
  return e;
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

interface AudioRig {
  ctx: AudioContext;
  stream: MediaStream;
  source: MediaStreamAudioSourceNode;
  analyser: AnalyserNode;
}

/**
 * Per-slot key geometry for the pixel-art engine: slot `n` (keymap
 * index, `pos = 8 + n*4`) → physical key centre in px from the vendor
 * layout table. Non-physical slots (`display:false`) get a linear
 * spread like the Rust engine's unplaceable fallback.
 */
function k1proGeometry(): KeyPoint[] {
  const pts: (KeyPoint | null)[] = new Array(KEY_SLOTS).fill(null);
  for (const b of K1_PRO_BLOCKS) {
    const n = keyIndexOf(b.pos);
    if (n < 0 || n >= KEY_SLOTS) continue;
    pts[n] = b.display
      ? { x: b.x + b.width / 2, y: b.y + b.height / 2 }
      : null;
  }
  return pts.map((p, i) => p ?? { x: (i / (KEY_SLOTS - 1)) * 16.5, y: 3 });
}

/** Float32 RGB triples (0..1) → 378-byte interleaved cmd-8 payload. */
function packPixelFrame(rgb: Float32Array): Uint8Array {
  const out = new Uint8Array(GIF_FRAME_BYTES);
  const n = Math.min(KEY_SLOTS, Math.floor(rgb.length / 3));
  for (let i = 0; i < n; i++) {
    out[i * 3] = Math.min(255, Math.max(0, Math.round((rgb[i * 3] ?? 0) * 255)));
    out[i * 3 + 1] = Math.min(
      255,
      Math.max(0, Math.round((rgb[i * 3 + 1] ?? 0) * 255)),
    );
    out[i * 3 + 2] = Math.min(
      255,
      Math.max(0, Math.round((rgb[i * 3 + 2] ?? 0) * 255)),
    );
  }
  return out;
}

/** Float RGB (0..1) → `#rrggbb` for the colour picker. */
function rgbToHex(c: Rgb): string {
  const h = (v: number) =>
    Math.round(Math.min(1, Math.max(0, v)) * 255)
      .toString(16)
      .padStart(2, '0');
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`;
}

function hexToRgb(hex: string): Rgb {
  const n = parseInt(hex.replace('#', ''), 16);
  return { r: ((n >> 16) & 0xff) / 255, g: ((n >> 8) & 0xff) / 255, b: (n & 0xff) / 255 };
}

// ── GIF / image → per-key RGB (Keyboard Studio parity) ────────────────
//
// Decode an animated GIF (ImageDecoder) or static image, place it with
// a fit/zoom/offset transform, sample each LED point's neighbourhood
// (proto/gifmap.ts), and stream the 378-byte cmd-8 frames — the same
// transport music/pixel-art use.

interface GifFrame {
  bitmap: ImageBitmap;
  /** Frame duration in ms (≥20, vendor floor). */
  duration: number;
}

interface GifClip {
  name: string;
  width: number;
  height: number;
  frames: GifFrame[];
  /** Cumulative end times (ms) for frame scheduling. */
  ends: number[];
  /** Normalized content bounds `[x0,y0,x1,y1]` for `trim`. */
  bounds: [number, number, number, number];
}

/**
 * Per-GIF placement — port of the reference app's persisted `Yl`
 * defaults + `T3` validation ranges. Session controls (brightness,
 * speed, play/pause) are deliberately NOT here: the reference keeps
 * them out of the saved placement too.
 */
interface GifPlacement {
  fit: 'contain' | 'cover' | 'stretch';
  /** Percent, 25–500. */
  zoom: number;
  /** Percent offsets, −250…250 (of canvas size). */
  x: number;
  y: number;
  mirror: boolean;
  flipY: boolean;
  trim: boolean;
  sampling: 'median' | 'point';
  /** 0–200 percent → ctx.filter saturate. */
  saturation: number;
  /** 0–96 black gate (max channel ≤ this → black). */
  blackLevel: number;
}

const GIF_DEFAULT_PLACEMENT: GifPlacement = {
  fit: 'contain',
  zoom: 100,
  x: 0,
  y: 0,
  mirror: false,
  flipY: false,
  trim: false,
  sampling: 'median',
  saturation: 100,
  blackLevel: 24,
};

const GIF_MAX_BYTES = 40 * 1024 * 1024;
const GIF_MAX_FRAMES = 3000;
const GIF_CANVAS_W = 600;

function detectImageType(head: Uint8Array): string {
  const ascii = (n: number) => String.fromCharCode(...head.subarray(0, n));
  if (ascii(6).startsWith('GIF8')) return 'image/gif';
  if (head[0] === 0x89 && head[1] === 0x50 && head[2] === 0x4e && head[3] === 0x47)
    return 'image/png';
  if (head[0] === 0xff && head[1] === 0xd8) return 'image/jpeg';
  if (ascii(4) === 'RIFF' && ascii(12).slice(8, 12) === 'WEBP') return 'image/webp';
  if (ascii(2) === 'BM') return 'image/bmp';
  throw new Error('Choose a GIF, PNG, JPG, WebP or BMP image.');
}

/**
 * Decode a GIF (animated, via ImageDecoder) or a still image into a
 * GifClip. Frames are downscaled so the longest side ≤ 640 px and total
 * pixels stay bounded — sampling happens on the rendered canvas anyway.
 */
async function decodeGifClip(file: File): Promise<GifClip> {
  if (file.size > GIF_MAX_BYTES) throw new Error('Image must be smaller than 40 MB.');
  const buf = await file.arrayBuffer();
  const type = detectImageType(new Uint8Array(buf, 0, 16));

  const scale = (w: number, h: number, frames: number) =>
    Math.min(1, 640 / w, 320 / h, Math.sqrt((64 * 1024 * 1024) / (4 * frames * w * h)));

  if (typeof ImageDecoder !== 'undefined' && (await ImageDecoder.isTypeSupported(type))) {
    const dec = new ImageDecoder({ data: buf, type, preferAnimation: true });
    try {
      await dec.tracks.ready;
      await dec.completed;
      const count = dec.tracks.selectedTrack?.frameCount ?? 0;
      if (!count || count > GIF_MAX_FRAMES)
        throw new Error(`GIF needs 1–${GIF_MAX_FRAMES} frames, got ${count}.`);
      const first = (await dec.decode({ frameIndex: 0 })).image;
      const sw = first.displayWidth;
      const sh = first.displayHeight;
      if (sw * sh > 16_000_000) {
        first.close();
        throw new Error('Image is too large — shrink it first.');
      }
      const t = scale(sw, sh, count);
      const cw = Math.max(1, Math.floor(sw * t));
      const ch = Math.max(1, Math.floor(sh * t));
      const canvas = document.createElement('canvas');
      canvas.width = cw;
      canvas.height = ch;
      const ctx = canvas.getContext('2d')!;
      const frames: GifFrame[] = [];
      const ends: number[] = [];
      const boundsFrames: Rgba[] = [];
      let acc = 0;
      for (let i = 0; i < count; i++) {
        const img =
          i === 0 ? first : (await dec.decode({ frameIndex: i })).image;
        ctx.fillStyle = '#000';
        ctx.fillRect(0, 0, cw, ch);
        ctx.drawImage(img, 0, 0, cw, ch);
        // Vendor floor: duration µs → ms, minimum 20 ms.
        const dur = Math.max(20, (img.duration ?? 100_000) / 1000); // VideoFrame.duration is µs, nullable
        frames.push({ bitmap: await createImageBitmap(canvas), duration: dur });
        acc += dur;
        ends.push(acc);
        if (i < 8) {
          boundsFrames.push({
            data: ctx.getImageData(0, 0, cw, ch).data,
            width: cw,
            height: ch,
          });
        }
        if (i > 0) img.close();
      }
      first.close();
      return {
        name: file.name,
        width: sw,
        height: sh,
        frames,
        ends,
        bounds: contentBounds(boundsFrames),
      };
    } finally {
      dec.close();
    }
  }

  // Static image fallback.
  const bmp = await createImageBitmap(file);
  try {
    if (bmp.width * bmp.height > 16_000_000)
      throw new Error('Image is too large — shrink it first.');
    const t = scale(bmp.width, bmp.height, 1);
    const cw = Math.max(1, Math.floor(bmp.width * t));
    const ch = Math.max(1, Math.floor(bmp.height * t));
    const canvas = document.createElement('canvas');
    canvas.width = cw;
    canvas.height = ch;
    const ctx = canvas.getContext('2d')!;
    ctx.drawImage(bmp, 0, 0, cw, ch);
    const px = ctx.getImageData(0, 0, cw, ch);
    return {
      name: file.name,
      width: bmp.width,
      height: bmp.height,
      frames: [{ bitmap: await createImageBitmap(canvas), duration: 1000 }],
      ends: [1000],
      bounds: contentBounds([{ data: px.data, width: cw, height: ch }]),
    };
  } finally {
    bmp.close();
  }
}

/** Port of the reference `R3`: destination rect for a source rect. */
function placementRect(
  srcW: number,
  srcH: number,
  p: GifPlacement,
  W: number,
  H: number,
): { x: number; y: number; width: number; height: number } {
  const d =
    p.fit === 'stretch'
      ? 1
      : (p.fit === 'cover' ? Math.max : Math.min)(W / srcW, H / srcH);
  const w = (p.fit === 'stretch' ? W : srcW * d) * (p.zoom / 100);
  const h = (p.fit === 'stretch' ? H : srcH * d) * (p.zoom / 100);
  return {
    x: (W - w) / 2 + (p.x / 100) * W,
    y: (H - h) / 2 + (p.y / 100) * H,
    width: w,
    height: h,
  };
}

/**
 * Port of the reference `Yy`: black background, optional trim to the
 * content bounds, fit/zoom/offset via `placementRect`, mirror/flipY
 * around the rect centre, saturation filter (brightness stays 1 for
 * GIF frames — the reference applies session brightness at sampling).
 */
function drawGifPlacement(
  ctx: CanvasRenderingContext2D,
  frame: CanvasImageSource & { width: number; height: number },
  p: GifPlacement,
  bounds: [number, number, number, number],
  W: number,
  H: number,
): void {
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.filter = 'none';
  ctx.fillStyle = '#000';
  ctx.fillRect(0, 0, W, H);
  const [bx0, by0, bx1, by1] = p.trim ? bounds : [0, 0, 1, 1];
  const sx = bx0 * frame.width;
  const sy = by0 * frame.height;
  const sw = Math.max(1, (bx1 - bx0) * frame.width);
  const sh = Math.max(1, (by1 - by0) * frame.height);
  const r = placementRect(sw, sh, p, W, H);
  ctx.save();
  ctx.translate(r.x + r.width / 2, r.y + r.height / 2);
  ctx.scale(p.mirror ? -1 : 1, p.flipY ? -1 : 1);
  ctx.filter = p.saturation === 100 ? 'none' : `saturate(${p.saturation / 100})`;
  ctx.drawImage(frame, sx, sy, sw, sh, -r.width / 2, -r.height / 2, r.width, r.height);
  ctx.restore();
}

/** Port of the reference `M3`: frame index for a position in `ends`. */
function gifFrameAt(ends: readonly number[], elapsedMs: number): number {
  const total = ends.at(-1);
  if (!total || ends.length === 0) return 0;
  const t = ((elapsedMs % total) + total) % total;
  let lo = 0;
  let hi = ends.length - 1;
  while (lo < hi) {
    const mid = Math.floor((lo + hi) / 2);
    if (t < ends[mid]) hi = mid;
    else lo = mid + 1;
  }
  return lo;
}

/**
 * Render + sample one clip frame at `elapsedMs` (already speed-scaled
 * by the caller, like the reference's `k.elapsed`) → 378-byte frame.
 */
function renderGifFrame(
  clip: GifClip,
  elapsedMs: number,
  p: GifPlacement,
  brightness01: number,
  points: readonly LedPoint[],
): Uint8Array {
  const bounds = layoutBounds(K1_PRO_BLOCKS);
  const W = GIF_CANVAS_W;
  const H = Math.max(1, Math.round((GIF_CANVAS_W * bounds.height) / bounds.width));
  const cv = document.createElement('canvas');
  cv.width = W;
  cv.height = H;
  const ctx = cv.getContext('2d', { willReadFrequently: true })!;
  const idx = gifFrameAt(clip.ends, elapsedMs);
  drawGifPlacement(ctx, clip.frames[idx].bitmap, p, clip.bounds, W, H);
  const img = ctx.getImageData(0, 0, W, H);
  const rgb = sampleSlots(
    { data: img.data, width: W, height: H },
    points,
    {
      sampling: p.sampling,
      brightness: Math.min(1, Math.max(0, brightness01)),
      blackLevel: p.blackLevel,
    },
  );
  const out = new Uint8Array(GIF_FRAME_BYTES);
  const n = Math.min(KEY_SLOTS, Math.floor(rgb.length / 3));
  for (let i = 0; i < n; i++) {
    out[i * 3] = Math.min(255, Math.max(0, Math.round((rgb[i * 3] ?? 0) * 255)));
    out[i * 3 + 1] = Math.min(255, Math.max(0, Math.round((rgb[i * 3 + 1] ?? 0) * 255)));
    out[i * 3 + 2] = Math.min(255, Math.max(0, Math.round((rgb[i * 3 + 2] ?? 0) * 255)));
  }
  return out;
}

/**
 * Port of the reference `fE` (calibration/"Kiểm tra LED"): one slot lit
 * at intensity 160, every other slot black.
 */
function checkLedFrame(slot: number): Uint8Array {
  const out = new Uint8Array(GIF_FRAME_BYTES);
  out.fill(160, slot * 3, slot * 3 + 3);
  return out;
}

const GIF_PLACEMENT_KEY = (name: string) => `omg:gifmap:${name}`;

/**
 * Port of the reference `T3`: clamp a persisted/raw placement into the
 * valid ranges — unknown fit/sampling values and non-finite numbers
 * fall back to the defaults rather than propagating.
 */
function sanitizeGifPlacement(raw: unknown): GifPlacement {
  const p: GifPlacement = { ...GIF_DEFAULT_PLACEMENT };
  if (!raw || typeof raw !== 'object') return p;
  const o = raw as Record<string, unknown>;
  if (o.fit === 'contain' || o.fit === 'cover' || o.fit === 'stretch') p.fit = o.fit;
  if (o.sampling === 'median' || o.sampling === 'point') p.sampling = o.sampling;
  const num = (v: unknown, lo: number, hi: number): number | null =>
    typeof v === 'number' && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : null;
  const zoom = num(o.zoom, 25, 500);
  if (zoom !== null) p.zoom = zoom;
  const x = num(o.x, -250, 250);
  if (x !== null) p.x = x;
  const y = num(o.y, -250, 250);
  if (y !== null) p.y = y;
  const saturation = num(o.saturation, 0, 200);
  if (saturation !== null) p.saturation = saturation;
  const blackLevel = num(o.blackLevel, 0, 96);
  if (blackLevel !== null) p.blackLevel = blackLevel;
  if (typeof o.mirror === 'boolean') p.mirror = o.mirror;
  if (typeof o.flipY === 'boolean') p.flipY = o.flipY;
  if (typeof o.trim === 'boolean') p.trim = o.trim;
  return p;
}

function loadGifPlacement(name: string): GifPlacement {
  try {
    const raw = localStorage.getItem(GIF_PLACEMENT_KEY(name));
    return sanitizeGifPlacement(raw ? JSON.parse(raw) : null);
  } catch {
    return { ...GIF_DEFAULT_PLACEMENT };
  }
}

function saveGifPlacement(name: string, p: GifPlacement): void {
  try {
    localStorage.setItem(GIF_PLACEMENT_KEY(name), JSON.stringify(p));
  } catch {
    /* private mode / quota — placement just won't persist */
  }
}

/** What the stream saved so stop() can put the lighting back. */
interface SavedLighting {
  profile: Uint8Array;
  lightData: Uint8Array;
}

export class MusicPage {
  readonly el: HTMLElement;

  private running = false;
  /** True during the async mic+snapshot phase before streaming begins. */
  private starting = false;
  /** Set by stop() while start() is still in its async setup phase. */
  private stopRequested = false;
  private pumpDone: Promise<void> | null = null;
  /** Dedupes the restore path shared by pump-exit and stop(). */
  private finishPromise: Promise<void> | null = null;
  /** Set by stop({restore:false}) before the pump finishes. */
  private wantRestore = true;
  private rig: AudioRig | null = null;
  private analyzer: MusicAnalyzer | null = null;
  private viz = new SpectrumViz();
  private params: FrameParams = { ...DEFAULT_FRAME_PARAMS };
  private bands: Bands = silentBands();
  private freqData: Uint8Array<ArrayBuffer> | null = null;
  private saved: SavedLighting | null = null;
  private framesSent = 0;
  private startedAt = 0;

  // Pixel-art stream (no mic needed — same cmd-8 path as music mode).
  private pixelRunning = false;
  private pixelStarting = false;
  private pixelStopRequested = false;
  private pixelPumpDone: Promise<void> | null = null;
  private pixelFinishPromise: Promise<void> | null = null;
  private pixelWantRestore = true;
  private pixelViz: PixelArt | null = null;
  private pixelSaved: SavedLighting | null = null;
  private pixelFramesSent = 0;
  private pixelStartedAt = 0;
  private pixelText = 'OMG';
  private pixelMotion: Motion = 'scroll_left';
  private pixelPalette: PaletteName | null = null;
  private pixelColor: Rgb = { ...RGB_WHITE };
  private pixelSpeed = 6;
  private pixelStartBtn: HTMLButtonElement | null = null;
  private pixelStopBtn: HTMLButtonElement | null = null;
  private pixelStatus: HTMLElement | null = null;

  // GIF / image stream — port of the reference app's GIF mode
  // (decode → placement → sample → cmd-8), plus its "Kiểm tra LED"
  // calibration mode. Same snapshot/restore contract as pixel art.
  private gifRunning = false;
  private gifStarting = false;
  private gifStopRequested = false;
  private gifPumpDone: Promise<void> | null = null;
  private gifFinishPromise: Promise<void> | null = null;
  private gifWantRestore = true;
  private gifClip: GifClip | null = null;
  private gifPlacement: GifPlacement = { ...GIF_DEFAULT_PLACEMENT };
  private gifBrightness = 100;
  private gifSpeed = 1;
  private gifPlaying = true;
  private gifMode: 'gif' | 'check' = 'gif';
  private gifCheckSlot = 0;
  private gifSaved: SavedLighting | null = null;
  private gifPoints: LedPoint[] | null = null;
  private gifElapsed = 0;
  private gifLastTick = 0;
  private gifFramesSent = 0;
  /** Preview clock — independent of the stream so the preview animates
   *  even while the pump is paused or not running. */
  private gifPreviewElapsed = 0;
  private gifPreviewLast = 0;
  private gifPreviewRaf = 0;
  private gifStartBtn: HTMLButtonElement | null = null;
  private gifStopBtn: HTMLButtonElement | null = null;
  private gifStatus: HTMLElement | null = null;
  private gifPreview: HTMLCanvasElement | null = null;
  private gifFileInput: HTMLInputElement | null = null;
  private gifFileLabel: HTMLElement | null = null;
  private gifCheckLabel: HTMLElement | null = null;
  private gifCheckBtn: HTMLButtonElement | null = null;
  private gifPlayBtn: HTMLButtonElement | null = null;

  // DOM
  private runCard: HTMLElement | null = null;
  private startBtn: HTMLButtonElement | null = null;
  private stopBtn: HTMLButtonElement | null = null;
  private pageStatus: HTMLElement | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private beatDot: HTMLElement | null = null;
  private meterRaf = 0;
  private logLines: string[] = [];

  private readonly unsub: () => void;

  constructor(root: HTMLElement) {
    this.el = el('div', 'music-page');
    root.append(this.el);
    this.render();
    this.unsub = subscribe(() => this.render());
  }

  destroy(): void {
    this.unsub();
    // Navigation away must not leave the keyboard in music mode — stop
    // (and restore) in the background even though this page is detached.
    if (this.running || this.starting || this.pumpDone) void this.stop();
    if (this.pixelRunning || this.pixelStarting || this.pixelPumpDone)
      void this.stopPixel();
    if (this.gifRunning || this.gifStarting || this.gifPumpDone)
      void this.stopGif();
    if (this.gifPreviewRaf) {
      cancelAnimationFrame(this.gifPreviewRaf);
      this.gifPreviewRaf = 0;
    }
    if (this.meterRaf) cancelAnimationFrame(this.meterRaf);
    this.el.remove();
  }

  // ── Rendering ───────────────────────────────────────────────────────

  private render(): void {
    // Device gone mid-stream: nothing to restore to — tear down now.
    if (
      !app.connected &&
      (this.running ||
        this.starting ||
        this.pumpDone ||
        this.pixelRunning ||
        this.pixelStarting ||
        this.pixelPumpDone ||
        this.gifRunning ||
        this.gifStarting ||
        this.gifPumpDone)
    ) {
      void this.stop({
        restore: false,
        reason: 'The keyboard disconnected — music mode stopped.',
      });
      void this.stopPixel({ restore: false });
      void this.stopGif({ restore: false });
    }
    this.el.replaceChildren();
    if (!app.connected || !app.snapshot) {
      this.runCard = null;
      const c = el('section', 'card');
      c.append(el('h2', '', 'Music lighting'));
      const body = el('div', 'card-body');
      body.append(
        el(
          'p',
          'hint',
          'Make the keyboard lights dance to your music. The page listens ' +
            'through your computer\'s microphone and sends live colour frames ' +
            'to the keyboard — no extra software needed.',
        ),
      );
      body.append(el('p', 'hint warn', 'Connect a keyboard first (top-right → Connect).'));
      c.append(body);
      this.el.append(c);
      return;
    }

    this.runCard = this.renderControls();
    this.el.append(this.runCard);
    this.el.append(this.renderPixelCard());
    this.el.append(this.renderGifCard());
    this.renderMeter();
    if (this.logLines.length) {
      const pre = el('pre', 'sec-log');
      pre.textContent = this.logLines.join('\n');
      this.el.append(pre);
    }
    this.syncButtons();
  }

  private renderControls(): HTMLElement {
    const c = el('section', 'card');
    c.append(el('h2', '', 'Music lighting'));
    const body = el('div', 'card-body');

    const row = el('div', 'sec-actions');
    this.startBtn = el('button', 'sec-btn', 'Start music mode');
    this.startBtn.type = 'button';
    this.stopBtn = el('button', 'sec-btn sec-btn-danger', 'Stop');
    this.stopBtn.type = 'button';
    this.startBtn.addEventListener('click', () => void this.start());
    this.stopBtn.addEventListener('click', () => void this.stop());
    row.append(this.startBtn, this.stopBtn);
    body.append(row);

    // Sliders reuse the lighting page's markup/classes.
    const sliders = el('div', 'light-sliders');
    sliders.append(
      this.makeSlider('Sensitivity', 1, 100, Math.round(this.params.sensitivity * 10), (v) => {
        this.params.sensitivity = v / 10;
      }),
    );
    sliders.append(
      this.makeSlider('Brightness', 0, 100, Math.round(this.params.brightness * 100), (v) => {
        this.params.brightness = v / 100;
      }),
    );
    body.append(sliders);

    this.pageStatus = el('p', 'hint');
    body.append(this.pageStatus);

    body.append(
      el(
        'p',
        'hint',
        'Browsers can only hear the microphone — play music out loud near the ' +
          'mic (or use speakers). When you press Stop, the keyboard goes back ' +
          'to whatever lighting it had before.',
      ),
    );
    c.append(body);
    return c;
  }

  private makeSlider(
    label: string,
    min: number,
    max: number,
    initial: number,
    onInput: (v: number) => void,
  ): HTMLElement {
    const wrap = el('div', 'light-slider');
    wrap.append(el('label', 'cp-label', label));
    const range = document.createElement('input');
    range.type = 'range';
    range.min = String(min);
    range.max = String(max);
    range.step = '1';
    range.value = String(initial);
    const valEl = el('span', 'light-val mono', String(initial));
    range.addEventListener('input', () => {
      const v = Number(range.value);
      valEl.textContent = label === 'Sensitivity' ? (v / 10).toFixed(1) : String(v);
      onInput(v);
    });
    wrap.append(range, valEl);
    return wrap;
  }

  // ── Pixel art card ──────────────────────────────────────────────────

  private renderPixelCard(): HTMLElement {
    const c = el('section', 'card');
    c.append(el('h2', '', 'Pixel art'));
    const body = el('div', 'card-body');
    body.append(
      el(
        'p',
        'hint',
        'Spell text or scroll art across the keys — no microphone needed. ' +
          'Same live-frame stream as music mode; Stop restores your lighting.',
      ),
    );

    const textRow = el('div', 'light-slider');
    textRow.append(el('label', 'cp-label', 'Text'));
    const textInput = document.createElement('input');
    textInput.type = 'text';
    textInput.value = this.pixelText;
    textInput.maxLength = 64;
    textInput.className = 'cp-input';
    textInput.addEventListener('input', () => {
      this.pixelText = textInput.value;
      this.pixelViz?.setText(textInput.value);
    });
    textRow.append(textInput);
    body.append(textRow);

    const grid = el('div', 'light-sliders');
    grid.append(this.makeSelect('Motion', this.pixelMotion, {
      static: 'Static',
      scroll_left: 'Scroll left',
      scroll_right: 'Scroll right',
    } as const, (v) => {
      this.pixelMotion = v;
      if (this.pixelViz) this.pixelViz.motion = v;
    }));
    grid.append(this.makeSelect('Palette', this.pixelPalette ?? 'none', {
      none: 'Solid colour',
      rainbow: 'Rainbow',
      heat: 'Heat',
      ocean: 'Ocean',
      sunset: 'Sunset',
      neon: 'Neon',
      mono: 'Mono',
    } as const, (v) => {
      this.pixelPalette = v === 'none' ? null : v;
      if (this.pixelViz) this.pixelViz.palette = this.pixelPalette;
    }));
    grid.append(
      this.makeSlider('Speed', 1, 30, this.pixelSpeed, (v) => {
        this.pixelSpeed = v;
        if (this.pixelViz) this.pixelViz.speed = v;
      }),
    );
    body.append(grid);

    const colourRow = el('div', 'light-slider');
    colourRow.append(el('label', 'cp-label', 'Colour'));
    const colourInput = document.createElement('input');
    colourInput.type = 'color';
    colourInput.value = rgbToHex(this.pixelColor);
    colourInput.addEventListener('input', () => {
      this.pixelColor = hexToRgb(colourInput.value);
      if (this.pixelViz) this.pixelViz.color = this.pixelColor;
    });
    colourRow.append(colourInput);
    body.append(colourRow);

    const row = el('div', 'sec-actions');
    this.pixelStartBtn = el('button', 'sec-btn', 'Start pixel art');
    this.pixelStartBtn.type = 'button';
    this.pixelStopBtn = el('button', 'sec-btn sec-btn-danger', 'Stop');
    this.pixelStopBtn.type = 'button';
    this.pixelStartBtn.addEventListener('click', () => void this.startPixel());
    this.pixelStopBtn.addEventListener('click', () => void this.stopPixel());
    row.append(this.pixelStartBtn, this.pixelStopBtn);
    body.append(row);

    this.pixelStatus = el('p', 'hint');
    body.append(this.pixelStatus);
    c.append(body);
    this.syncPixelButtons();
    return c;
  }

  private makeSelect<K extends string>(
    label: string,
    current: K,
    options: Record<K, string>,
    onChange: (v: K) => void,
  ): HTMLElement {
    const wrap = el('div', 'light-slider');
    wrap.append(el('label', 'cp-label', label));
    const sel = document.createElement('select');
    sel.className = 'cp-input';
    for (const value of Object.keys(options) as K[]) {
      const o = document.createElement('option');
      o.value = value;
      o.textContent = options[value];
      o.selected = value === current;
      sel.append(o);
    }
    sel.addEventListener('change', () => onChange(sel.value as K));
    wrap.append(sel);
    return wrap;
  }

  private syncPixelButtons(): void {
    if (!this.pixelStartBtn || !this.pixelStopBtn) return;
    this.pixelStartBtn.hidden = this.pixelRunning;
    this.pixelStopBtn.hidden = !this.pixelRunning && !this.pixelStarting;
    this.pixelStartBtn.disabled =
      app.busy || this.pixelStarting || this.running || this.gifRunning || this.gifStarting;
  }

  private setPixelStatus(msg: string, isError = false): void {
    if (this.pixelStatus) {
      this.pixelStatus.textContent = msg;
      this.pixelStatus.classList.toggle('warn', isError);
    }
  }

  // ── Pixel-art start / stop ──────────────────────────────────────────

  private async startPixel(): Promise<void> {
    if (
      this.pixelRunning ||
      this.pixelStarting ||
      this.running ||
      this.gifRunning ||
      this.gifStarting ||
      app.busy
    )
      return;
    const c = app.connected;
    if (!c) {
      setStatus('Connect a keyboard first.', true);
      return;
    }
    this.pixelStarting = true;
    this.pixelStopRequested = false;
    this.setPixelStatus('Saving your current lighting…');
    this.syncPixelButtons();
    try {
      const [profile, lightData] = await Promise.all([
        c.session.profile(),
        c.session.lightColorData(),
      ]);
      this.pixelSaved = { profile: profile.raw, lightData };
    } catch (e) {
      this.setPixelStatus(
        `Could not read the keyboard's lighting settings: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
      this.pixelStarting = false;
      this.syncPixelButtons();
      return;
    }
    if (this.pixelStopRequested) {
      this.pixelSaved = null;
      this.pixelStarting = false;
      this.syncPixelButtons();
      return;
    }

    const viz = new PixelArt({
      art: { kind: 'text', text: this.pixelText },
      cols: 21,
      rows: 6,
      motion: this.pixelMotion,
      speed: this.pixelSpeed,
      color: this.pixelColor,
      palette: this.pixelPalette,
    });
    viz.setGeometry(k1proGeometry());
    this.pixelViz = viz;
    this.pixelRunning = true;
    this.pixelWantRestore = true;
    this.pixelFinishPromise = null;
    this.pixelStartedAt = performance.now();
    this.pixelFramesSent = 0;
    app.musicActive = true;
    emit();
    setStatus('Pixel art is on — the keyboard shows your text.');
    this.setPixelStatus('Running… press Stop to restore your lighting.');
    this.pixelStarting = false;
    this.log(`Pixel art started ("${this.pixelText}").`);
    this.syncPixelButtons();
    this.pixelPumpDone = this.pixelPump();
  }

  private async pixelPump(): Promise<void> {
    const c = app.connected;
    const viz = this.pixelViz;
    if (!c || !viz) {
      this.pixelRunning = false;
      return;
    }
    let streamError: string | null = null;
    let last = performance.now();
    while (this.pixelRunning) {
      const tickStart = performance.now();
      try {
        const rgb = viz.frame((tickStart - last) / 1000);
        last = tickStart;
        await c.session.writeGifFrame(packPixelFrame(rgb));
        this.pixelFramesSent++;
      } catch (e) {
        streamError = e instanceof Error ? e.message : String(e);
        break;
      }
      const elapsed = performance.now() - tickStart;
      const wait = GIF_FRAME_PERIOD_MS - elapsed;
      await sleep(wait > 0 ? wait : 0);
    }
    this.pixelRunning = false;
    if (streamError) {
      this.log(`Pixel-art stream stopped: ${streamError}`);
      setStatus(`Pixel art stopped — ${streamError}`, true);
    }
    await this.ensurePixelFinish();
  }

  async stopPixel(opts: { restore?: boolean } = {}): Promise<void> {
    if (opts.restore === false) this.pixelWantRestore = false;
    this.pixelStopRequested = true;
    this.pixelRunning = false;
    await this.pixelPumpDone?.catch(() => {});
    this.pixelPumpDone = null;
    await this.ensurePixelFinish();
  }

  private ensurePixelFinish(): Promise<void> {
    if (!this.pixelFinishPromise) {
      this.pixelFinishPromise = this.doPixelFinish(this.pixelWantRestore);
    }
    return this.pixelFinishPromise;
  }

  private async doPixelFinish(restore: boolean): Promise<void> {
    this.pixelViz = null;
    app.musicActive = this.running || this.gifRunning; // other streams may still be live
    emit();
    this.syncPixelButtons();
    const c = app.connected;
    const saved = this.pixelSaved;
    this.pixelSaved = null;
    if (!restore || !c || !saved) {
      if (!this.running && !this.gifRunning) setStatus('Pixel art off.');
      return;
    }
    const results: string[] = [];
    try {
      const out = await c.session.writeProfile(saved.profile);
      results.push(`lighting settings restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore lighting settings: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    try {
      const out = await c.session.writeLightData(saved.lightData);
      results.push(`effect colours restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore effect colours: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    const failed = results.some((r) => r.startsWith('could not'));
    if (!this.running && !this.gifRunning) {
      setStatus(
        failed
          ? `Pixel art off — ${results.join('; ')}.`
          : 'Pixel art off — your lighting is back.',
        failed,
      );
    }
    for (const r of results) this.log(r);
    if (this.el.isConnected) this.render();
  }

  // ── GIF / image card ────────────────────────────────────────────────

  private renderGifCard(): HTMLElement {
    const c = el('section', 'card');
    c.append(el('h2', '', 'GIF / image'));
    const body = el('div', 'card-body');
    body.append(
      el(
        'p',
        'hint',
        'Pick a GIF or image, place it over the keyboard layout, then Start ' +
          'to stream live colours — the same cmd-8 stream as music and pixel ' +
          'art. Only works while the keyboard is connected over USB/WiFi. ' +
          'Stop restores your lighting.',
      ),
    );

    // File picker — hidden input + button, vendor "Choose GIF / image".
    const fileRow = el('div', 'sec-actions');
    this.gifFileInput = document.createElement('input');
    this.gifFileInput.type = 'file';
    this.gifFileInput.accept = 'image/gif,image/webp,image/png,image/jpeg,image/bmp';
    this.gifFileInput.hidden = true;
    this.gifFileInput.addEventListener('change', () => {
      const f = this.gifFileInput?.files?.[0];
      if (f) void this.loadGifFile(f);
      if (this.gifFileInput) this.gifFileInput.value = '';
    });
    const pickBtn = el('button', 'sec-btn', 'Choose GIF / image');
    pickBtn.type = 'button';
    pickBtn.addEventListener('click', () => this.gifFileInput?.click());
    fileRow.append(pickBtn);
    body.append(fileRow);
    this.gifFileLabel = el('p', 'hint');
    body.append(this.gifFileLabel);

    // Preview canvas — layout aspect ratio, drag = move, wheel = zoom
    // (vendor "Drag to move · Scroll to zoom").
    const bounds = layoutBounds(K1_PRO_BLOCKS);
    const pw = GIF_CANVAS_W;
    const ph = Math.max(1, Math.round((GIF_CANVAS_W * bounds.height) / bounds.width));
    this.gifPreview = document.createElement('canvas');
    this.gifPreview.className = 'gif-preview';
    this.gifPreview.width = pw;
    this.gifPreview.height = ph;
    this.gifPreview.tabIndex = 0;
    let dragLast: { x: number; y: number } | null = null;
    this.gifPreview.addEventListener('pointerdown', (e) => {
      dragLast = { x: e.clientX, y: e.clientY };
      this.gifPreview?.setPointerCapture(e.pointerId);
    });
    this.gifPreview.addEventListener('pointermove', (e) => {
      if (!dragLast) return;
      const rect = this.gifPreview?.getBoundingClientRect();
      if (!rect || rect.width === 0 || rect.height === 0) return;
      const dx = e.clientX - dragLast.x;
      const dy = e.clientY - dragLast.y;
      dragLast = { x: e.clientX, y: e.clientY };
      // Vendor x/y are percentages of the canvas.
      this.updateGifPlacement((p) => {
        p.x += (dx / rect.width) * 100;
        p.y += (dy / rect.height) * 100;
      });
    });
    const endDrag = (): void => {
      dragLast = null;
    };
    this.gifPreview.addEventListener('pointerup', endDrag);
    this.gifPreview.addEventListener('pointercancel', endDrag);
    this.gifPreview.addEventListener(
      'wheel',
      (e) => {
        e.preventDefault();
        this.updateGifPlacement((p) => {
          p.zoom = Math.min(500, Math.max(25, Math.round(p.zoom * Math.exp(-e.deltaY * 0.002))));
        });
      },
      { passive: false },
    );
    body.append(this.gifPreview);
    body.append(el('p', 'hint', 'Drag to move · Scroll to zoom · dots mark LED sampling points.'));
    this.drawGifPreview();
    this.startGifPreviewLoop();

    // Placement controls — vendor "GIF placement" group.
    const grid = el('div', 'light-sliders');
    grid.append(
      this.makeSelect('Image fit', this.gifPlacement.fit, {
        contain: 'Contain',
        cover: 'Cover',
        stretch: 'Stretch',
      } as const, (v) => this.updateGifPlacement((p) => (p.fit = v))),
      this.makeSlider('Scale', 25, 500, Math.round(this.gifPlacement.zoom), (v) =>
        this.updateGifPlacement((p) => (p.zoom = v)),
      ),
      this.makeSlider('Left / right', -250, 250, Math.round(this.gifPlacement.x), (v) =>
        this.updateGifPlacement((p) => (p.x = v)),
      ),
      this.makeSlider('Up / down', -250, 250, Math.round(this.gifPlacement.y), (v) =>
        this.updateGifPlacement((p) => (p.y = v)),
      ),
      this.makeSelect('Color sampling', this.gifPlacement.sampling, {
        median: 'Key background color',
        point: 'LED center point',
      } as const, (v) => this.updateGifPlacement((p) => (p.sampling = v))),
      this.makeSlider('Saturation', 0, 200, Math.round(this.gifPlacement.saturation), (v) =>
        this.updateGifPlacement((p) => (p.saturation = v)),
      ),
      this.makeSlider('Dark cutoff', 0, 96, Math.round(this.gifPlacement.blackLevel), (v) =>
        this.updateGifPlacement((p) => (p.blackLevel = v)),
      ),
    );
    body.append(grid);
    body.append(
      el(
        'p',
        'hint',
        'Key background sampling reduces bright text artifacts. For small ' +
          'dots, choose LED center point. Raise the cutoff to remove dark ' +
          'backgrounds; lower it to preserve dim detail.',
      ),
    );

    const checks = el('div', 'gif-checks');
    const mkCheck = (label: string, get: () => boolean, set: (v: boolean) => void) => {
      const row = el('label', 'gif-check');
      const cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = get();
      cb.addEventListener('change', () => {
        set(cb.checked);
        this.drawGifPreview();
      });
      row.append(cb, el('span', '', label));
      checks.append(row);
    };
    mkCheck('Mirror horizontally', () => this.gifPlacement.mirror, (v) =>
      this.updateGifPlacement((p) => (p.mirror = v)),
    );
    mkCheck('Flip vertically', () => this.gifPlacement.flipY, (v) =>
      this.updateGifPlacement((p) => (p.flipY = v)),
    );
    mkCheck('Trim empty borders', () => this.gifPlacement.trim, (v) =>
      this.updateGifPlacement((p) => (p.trim = v)),
    );
    body.append(checks);

    // Session controls (vendor keeps these out of the saved placement):
    // brightness, speed, play/pause.
    const session = el('div', 'light-sliders');
    session.append(
      this.makeSlider('Brightness', 0, 100, this.gifBrightness, (v) => {
        this.gifBrightness = v;
        this.drawGifPreview();
      }),
    );
    // Speed needs 0.05 steps — makeSlider is integer-only.
    const speedWrap = el('div', 'light-slider');
    speedWrap.append(el('label', 'cp-label', 'Speed'));
    const speedRange = document.createElement('input');
    speedRange.type = 'range';
    speedRange.min = '0.25';
    speedRange.max = '3';
    speedRange.step = '0.05';
    speedRange.value = String(this.gifSpeed);
    const speedVal = el('span', 'light-val mono', `${this.gifSpeed.toFixed(2)}×`);
    speedRange.addEventListener('input', () => {
      this.gifSpeed = Number(speedRange.value);
      speedVal.textContent = `${this.gifSpeed.toFixed(2)}×`;
    });
    speedWrap.append(speedRange, speedVal);
    session.append(speedWrap);
    body.append(session);

    const row = el('div', 'sec-actions');
    this.gifPlayBtn = el('button', 'sec-btn', this.gifPlaying ? 'Pause' : 'Play');
    this.gifPlayBtn.type = 'button';
    this.gifPlayBtn.addEventListener('click', () => {
      this.gifPlaying = !this.gifPlaying;
      if (this.gifPlayBtn) this.gifPlayBtn.textContent = this.gifPlaying ? 'Pause' : 'Play';
    });
    row.append(this.gifPlayBtn);
    body.append(row);

    // Check-LEDs (vendor calibration mode): while streaming, the stream
    // sends one lit slot instead of GIF frames.
    const checkRow = el('div', 'sec-actions');
    const checkBtn = el('button', 'sec-btn', 'Check LEDs');
    checkBtn.type = 'button';
    this.gifCheckBtn = checkBtn;
    checkBtn.addEventListener('click', () => {
      this.gifMode = this.gifMode === 'check' ? 'gif' : 'check';
      this.syncGifButtons();
      if (this.gifMode === 'check') {
        this.setGifStatus(`Lighting LED slot ${this.gifCheckSlot}. All other slots are off.`);
      } else {
        this.setGifStatus('GIF frames resumed.');
      }
    });
    const prevBtn = el('button', 'sec-btn', '‹');
    prevBtn.type = 'button';
    prevBtn.title = 'Previous slot';
    prevBtn.addEventListener('click', () => this.stepGifCheckSlot(-1));
    const nextBtn = el('button', 'sec-btn', '›');
    nextBtn.type = 'button';
    nextBtn.title = 'Next slot';
    nextBtn.addEventListener('click', () => this.stepGifCheckSlot(1));
    this.gifCheckLabel = el('span', 'hint mono', '');
    checkRow.append(checkBtn, prevBtn, this.gifCheckLabel, nextBtn);
    body.append(checkRow);
    body.append(
      el('p', 'hint', 'Lights one LED at a time — step through to verify positions.'),
    );

    const startRow = el('div', 'sec-actions');
    this.gifStartBtn = el('button', 'sec-btn', 'Play on keyboard');
    this.gifStartBtn.type = 'button';
    this.gifStopBtn = el('button', 'sec-btn sec-btn-danger', 'Stop');
    this.gifStopBtn.type = 'button';
    this.gifStartBtn.addEventListener('click', () => void this.startGif());
    this.gifStopBtn.addEventListener('click', () => void this.stopGif());
    startRow.append(this.gifStartBtn, this.gifStopBtn);
    body.append(startRow);

    this.gifStatus = el('p', 'hint');
    body.append(this.gifStatus);
    body.append(
      el(
        'p',
        'hint',
        'Keep this page open while streaming. Placement is saved on this ' +
          'device; GIFs are not saved to keyboard memory.',
      ),
    );
    c.append(body);
    this.syncGifButtons();
    this.syncGifCheckLabel();
    return c;
  }

  /** Mutate the placement, clamp it, persist and repaint the preview. */
  private updateGifPlacement(mut: (p: GifPlacement) => void): void {
    mut(this.gifPlacement);
    this.gifPlacement = sanitizeGifPlacement(this.gifPlacement);
    if (this.gifClip) saveGifPlacement(this.gifClip.name, this.gifPlacement);
    this.drawGifPreview();
  }

  private stepGifCheckSlot(delta: number): void {
    this.gifCheckSlot = Math.min(KEY_SLOTS - 1, Math.max(0, this.gifCheckSlot + delta));
    this.syncGifCheckLabel();
    if (this.gifMode === 'check' && this.gifRunning) {
      this.setGifStatus(`Lighting LED slot ${this.gifCheckSlot}. All other slots are off.`);
    }
  }

  private syncGifCheckLabel(): void {
    if (this.gifCheckLabel) {
      this.gifCheckLabel.textContent = `LED slot ${this.gifCheckSlot} / ${KEY_SLOTS - 1}`;
    }
  }

  private setGifStatus(msg: string, isError = false): void {
    if (this.gifStatus) {
      this.gifStatus.textContent = msg;
      this.gifStatus.classList.toggle('warn', isError);
    }
  }

  private syncGifButtons(): void {
    if (this.gifStartBtn) this.gifStartBtn.hidden = this.gifRunning;
    if (this.gifStopBtn) {
      this.gifStopBtn.hidden = !this.gifRunning && !this.gifStarting;
    }
    if (this.gifPlayBtn) this.gifPlayBtn.disabled = !this.gifRunning;
    if (this.gifCheckBtn) this.gifCheckBtn.disabled = !this.gifRunning;
    if (this.gifStartBtn) {
      this.gifStartBtn.disabled =
        app.busy || this.gifStarting || this.running || this.pixelRunning;
    }
  }

  /**
   * Preview render: draw the current frame through the placement
   * transform, then overlay one dot per LED slot coloured by the same
   * sampling the stream uses — vendor "dots mark LED sampling points".
   */
  private drawGifPreview(): void {
    const cv = this.gifPreview;
    const clip = this.gifClip;
    if (!cv) return;
    const ctx = cv.getContext('2d');
    if (!ctx) return;
    if (!clip) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = '#000';
      ctx.fillRect(0, 0, cv.width, cv.height);
      if (this.gifFileLabel) this.gifFileLabel.textContent = '';
      return;
    }
    const idx = gifFrameAt(clip.ends, this.gifPreviewElapsed);
    drawGifPlacement(ctx, clip.frames[idx].bitmap, this.gifPlacement, clip.bounds, cv.width, cv.height);
    const img = ctx.getImageData(0, 0, cv.width, cv.height);
    const points = this.gifPoints ?? k1proLedPoints();
    const rgb = sampleSlots(
      { data: img.data, width: cv.width, height: cv.height },
      points,
      {
        sampling: this.gifPlacement.sampling,
        brightness: Math.min(1, Math.max(0, this.gifBrightness / 100)),
        blackLevel: this.gifPlacement.blackLevel,
      },
    );
    const bounds = layoutBounds(K1_PRO_BLOCKS);
    if (bounds.width === 0 || bounds.height === 0) return;
    const slots = k1proSlotPoints();
    ctx.fillStyle = 'rgba(255,255,255,0.9)';
    for (let s = 0; s < slots.length && s * 3 + 2 < rgb.length; s++) {
      const sp = slots[s];
      const x = (sp.x / bounds.width) * cv.width;
      const y = (sp.y / bounds.height) * cv.height;
      const r = Math.round(Math.min(1, rgb[s * 3] ?? 0) * 255);
      const g = Math.round(Math.min(1, rgb[s * 3 + 1] ?? 0) * 255);
      const b = Math.round(Math.min(1, rgb[s * 3 + 2] ?? 0) * 255);
      ctx.beginPath();
      ctx.arc(x, y, 3, 0, Math.PI * 2);
      ctx.fillStyle = `rgb(${r},${g},${b})`;
      ctx.fill();
      ctx.lineWidth = 1;
      ctx.strokeStyle = 'rgba(0,0,0,0.6)';
      ctx.stroke();
    }
  }

  /** rAF loop keeping the preview animating (vendor "Preview playing"). */
  private startGifPreviewLoop(): void {
    if (this.gifPreviewRaf) return;
    const tick = (t: number): void => {
      const dt = this.gifPreviewLast ? t - this.gifPreviewLast : 0;
      this.gifPreviewLast = t;
      if (this.gifPlaying && this.gifClip) this.gifPreviewElapsed += dt * this.gifSpeed;
      this.drawGifPreview();
      this.gifPreviewRaf =
        this.gifClip && this.gifPreview?.isConnected ? requestAnimationFrame(tick) : 0;
    };
    this.gifPreviewRaf = requestAnimationFrame(tick);
  }

  private async loadGifFile(file: File): Promise<void> {
    if (this.gifRunning || this.gifStarting) {
      this.setGifStatus('Stop the stream first.', true);
      return;
    }
    this.setGifStatus('Decoding…');
    const old = this.gifClip;
    if (old) for (const f of old.frames) f.bitmap.close();
    this.gifClip = null;
    this.drawGifPreview();
    try {
      this.gifClip = await decodeGifClip(file);
    } catch (e) {
      this.setGifStatus(e instanceof Error ? e.message : String(e), true);
      return;
    }
    this.gifPlacement = loadGifPlacement(this.gifClip.name);
    this.gifPoints = k1proLedPoints();
    this.gifElapsed = 0;
    this.gifPreviewElapsed = 0;
    this.gifMode = 'gif';
    this.gifCheckSlot = 0;
    this.syncGifCheckLabel();
    const clip = this.gifClip;
    if (this.gifFileLabel) {
      // Vendor label format: "{name} · {w} × {h} px · {n} frames".
      this.gifFileLabel.textContent =
        `${clip.name} · ${clip.width} × ${clip.height} px · ` +
        `${clip.frames.length} frame${clip.frames.length === 1 ? '' : 's'}`;
    }
    this.drawGifPreview();
    this.startGifPreviewLoop();
    this.setGifStatus('Ready — press Play on keyboard to stream it.');
  }

  // ── GIF start / stop ────────────────────────────────────────────────

  private async startGif(): Promise<void> {
    if (
      this.gifRunning ||
      this.gifStarting ||
      this.running ||
      this.pixelRunning ||
      app.busy
    )
      return;
    const c = app.connected;
    if (!c) {
      setStatus('Connect a keyboard first.', true);
      return;
    }
    if (!this.gifClip || !this.gifPoints) {
      this.setGifStatus('Choose a GIF or image first.', true);
      return;
    }
    this.gifStarting = true;
    this.gifStopRequested = false;
    this.gifMode = 'gif';
    this.setGifStatus('Saving your current lighting…');
    this.syncGifButtons();
    try {
      const [profile, lightData] = await Promise.all([
        c.session.profile(),
        c.session.lightColorData(),
      ]);
      this.gifSaved = { profile: profile.raw, lightData };
    } catch (e) {
      this.setGifStatus(
        `Could not read the keyboard's lighting settings: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
      this.gifStarting = false;
      this.syncGifButtons();
      return;
    }
    if (this.gifStopRequested) {
      this.gifSaved = null;
      this.gifStarting = false;
      this.syncGifButtons();
      return;
    }

    this.gifRunning = true;
    this.gifWantRestore = true;
    this.gifFinishPromise = null;
    this.gifElapsed = 0;
    this.gifLastTick = performance.now();
    this.gifFramesSent = 0;
    app.musicActive = true;
    emit();
    setStatus('GIF mode is on — the keyboard shows your image.');
    this.setGifStatus('Streaming… press Stop to restore your lighting.');
    this.gifStarting = false;
    this.log(`GIF stream started ("${this.gifClip?.name}").`);
    this.syncGifButtons();
    this.gifPumpDone = this.gifPump();
  }

  private async gifPump(): Promise<void> {
    const c = app.connected;
    const clip = this.gifClip;
    const points = this.gifPoints;
    if (!c || !clip || !points) {
      this.gifRunning = false;
      return;
    }
    let streamError: string | null = null;
    while (this.gifRunning) {
      const tickStart = performance.now();
      try {
        let frame: Uint8Array;
        if (this.gifMode === 'check') {
          frame = checkLedFrame(this.gifCheckSlot);
        } else {
          // Vendor: elapsed advances by dt × speed only while playing;
          // paused holds the same frame.
          const dt = tickStart - this.gifLastTick;
          if (this.gifPlaying) this.gifElapsed += dt * this.gifSpeed;
          frame = renderGifFrame(
            clip,
            this.gifElapsed,
            this.gifPlacement,
            this.gifBrightness / 100,
            points,
          );
        }
        this.gifLastTick = tickStart;
        await c.session.writeGifFrame(frame);
        this.gifFramesSent++;
      } catch (e) {
        streamError = e instanceof Error ? e.message : String(e);
        break;
      }
      const elapsed = performance.now() - tickStart;
      const wait = GIF_FRAME_PERIOD_MS - elapsed;
      await sleep(wait > 0 ? wait : 0);
    }
    this.gifRunning = false;
    if (streamError) {
      this.log(`GIF stream stopped: ${streamError}`);
      setStatus(`GIF mode stopped — ${streamError}`, true);
    }
    await this.ensureGifFinish();
  }

  async stopGif(opts: { restore?: boolean; reason?: string } = {}): Promise<void> {
    if (opts.restore === false) this.gifWantRestore = false;
    if (opts.reason) this.setGifStatus(opts.reason, true);
    this.gifStopRequested = true;
    this.gifRunning = false;
    await this.gifPumpDone?.catch(() => {});
    this.gifPumpDone = null;
    await this.ensureGifFinish();
  }

  private ensureGifFinish(): Promise<void> {
    if (!this.gifFinishPromise) {
      this.gifFinishPromise = this.doGifFinish(this.gifWantRestore);
    }
    return this.gifFinishPromise;
  }

  private async doGifFinish(restore: boolean): Promise<void> {
    // The clip stays loaded — the preview keeps working after Stop.
    app.musicActive = this.running || this.pixelRunning;
    emit();
    this.syncGifButtons();
    const c = app.connected;
    const saved = this.gifSaved;
    this.gifSaved = null;
    if (!restore || !c || !saved) {
      if (!this.running && !this.pixelRunning) setStatus('GIF mode off.');
      return;
    }
    const results: string[] = [];
    try {
      const out = await c.session.writeProfile(saved.profile);
      results.push(`lighting settings restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore lighting settings: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    try {
      const out = await c.session.writeLightData(saved.lightData);
      results.push(`effect colours restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore effect colours: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    const failed = results.some((r) => r.startsWith('could not'));
    if (!this.running && !this.pixelRunning) {
      setStatus(
        failed
          ? `GIF mode off — ${results.join('; ')}.`
          : 'GIF mode off — your lighting is back.',
        failed,
      );
    }
    for (const r of results) this.log(r);
    if (this.el.isConnected) this.render();
  }

  private renderMeter(): void {
    const c = el('section', 'card');
    c.append(el('h2', '', 'Live level'));
    const body = el('div', 'card-body');
    const row = el('div', 'music-meter-row');
    this.canvas = document.createElement('canvas');
    this.canvas.className = 'music-meter';
    this.canvas.width = 640;
    this.canvas.height = 72;
    this.beatDot = el('span', 'music-beat', '');
    this.beatDot.title = 'Beat detected';
    const stats = el('span', 'music-stats mono', '');
    stats.dataset.role = 'stats';
    row.append(this.canvas, this.beatDot, stats);
    body.append(row);
    body.append(
      el(
        'p',
        'hint',
        'Bass · mids · treble — the bars show what the keyboard is hearing right now.',
      ),
    );
    c.append(body);
    this.el.append(c);
    this.drawMeter();
    this.loopMeter();
  }

  // ── Meter drawing ───────────────────────────────────────────────────

  private loopMeter(): void {
    if (this.meterRaf) return;
    const tick = (): void => {
      this.drawMeter();
      this.meterRaf =
        this.running || this.bands.level > 0.01 ? requestAnimationFrame(tick) : 0;
    };
    this.meterRaf = requestAnimationFrame(tick);
  }

  private drawMeter(): void {
    const cv = this.canvas;
    if (!cv) return;
    const ctx = cv.getContext('2d');
    if (!ctx) return;
    const w = cv.width;
    const h = cv.height;
    ctx.clearRect(0, 0, w, h);
    const bands: [string, number][] = [
      ['Bass', this.bands.bass],
      ['Mids', this.bands.mid],
      ['Treble', this.bands.treble],
    ];
    const barW = w / bands.length;
    bands.forEach(([label, v], i) => {
      const x = i * barW + barW * 0.15;
      const bw = barW * 0.7;
      const bh = Math.max(2, Math.min(1, v) * (h - 8));
      ctx.fillStyle = 'rgba(255,255,255,0.14)';
      ctx.fillRect(x, 4, bw, h - 8);
      // Accent #c6f54e (--accent) — canvas can't read CSS vars.
      const grad = ctx.createLinearGradient(0, h - bh, 0, h);
      grad.addColorStop(0, '#d4ff6a');
      grad.addColorStop(1, 'rgba(212,255,106,0.35)');
      ctx.fillStyle = grad;
      ctx.fillRect(x, h - 4 - bh, bw, bh);
      ctx.fillStyle = 'rgba(255,255,255,0.55)';
      ctx.font = '10px system-ui, sans-serif';
      ctx.fillText(label, x, h - 1);
    });
    // Overall level as a thin line near the top.
    ctx.fillStyle = 'rgba(255,255,255,0.35)';
    ctx.fillRect(0, 2, w * Math.min(1, this.bands.level), 2);
    const stats = this.el.querySelector<HTMLElement>('[data-role="stats"]');
    if (stats) {
      stats.textContent = this.running
        ? `${((performance.now() - this.startedAt) / 1000).toFixed(1)}s · ${this.framesSent} frames`
        : '';
    }
    this.beatDot?.classList.toggle('on', this.bands.beat);
  }

  private syncButtons(): void {
    if (!this.startBtn || !this.stopBtn) return;
    this.startBtn.hidden = this.running;
    this.stopBtn.hidden = !this.running && !this.starting;
    this.startBtn.disabled =
      app.busy || this.starting || this.pixelRunning || this.gifRunning || this.gifStarting;
  }

  private setPageStatus(msg: string, isError = false): void {
    if (this.pageStatus) {
      this.pageStatus.textContent = msg;
      this.pageStatus.classList.toggle('warn', isError);
    }
  }

  private log(msg: string): void {
    this.logLines.push(msg);
    if (this.logLines.length > 12) this.logLines.shift();
  }

  // ── Start / stop ────────────────────────────────────────────────────

  private async start(): Promise<void> {
    if (
      this.running ||
      this.starting ||
      this.pixelRunning ||
      this.gifRunning ||
      this.gifStarting ||
      app.busy
    )
      return;
    const c = app.connected;
    if (!c) {
      setStatus('Connect a keyboard first.', true);
      return;
    }
    if (typeof navigator === 'undefined' || !navigator.mediaDevices?.getUserMedia) {
      this.setPageStatus(
        'This browser cannot use the microphone here — open the page over ' +
          'http://localhost or an https:// address and try again.',
        true,
      );
      return;
    }
    this.starting = true;
    this.stopRequested = false;
    this.setPageStatus('Asking for microphone permission…');
    this.syncButtons();
    try {
      // Mic first (like the CLI opens audio before touching the device):
      // a permission denial should surface before any keyboard write.
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: false,
          noiseSuppression: false,
          autoGainControl: false,
        },
      });
      const ctx = new AudioContext();
      if (ctx.state === 'suspended') await ctx.resume();
      const source = ctx.createMediaStreamSource(stream);
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 2048;
      analyser.smoothingTimeConstant = 0; // we smooth ourselves (dsp.rs)
      source.connect(analyser);
      this.rig = { ctx, stream, source, analyser };
      this.freqData = new Uint8Array(analyser.frequencyBinCount);
      this.analyzer = new MusicAnalyzer(ctx.sampleRate, analyser.fftSize);
    } catch (e) {
      const name = e instanceof DOMException ? e.name : '';
      const msg =
        name === 'NotAllowedError' || name === 'SecurityError'
          ? 'Microphone access was blocked. Allow the microphone for this ' +
            'site (click the camera/mic icon next to the address bar) and try again.'
          : name === 'NotFoundError' || name === 'OverconstrainedError'
            ? 'No microphone was found — plug one in or check your sound settings.'
            : `Could not open the microphone: ${e instanceof Error ? e.message : String(e)}`;
      this.setPageStatus(msg, true);
      this.teardownAudio();
      this.starting = false;
      this.syncButtons();
      return;
    }

    try {
      // Snapshot lighting for restore BEFORE streaming (CLI parity:
      // profile + cmd-138 light data, fresh reads not the page snapshot).
      this.setPageStatus('Saving your current lighting…');
      const [profile, lightData] = await Promise.all([
        c.session.profile(),
        c.session.lightColorData(),
      ]);
      this.saved = { profile: profile.raw, lightData };
    } catch (e) {
      this.teardownAudio();
      this.setPageStatus(
        `Could not read the keyboard's lighting settings: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
      this.starting = false;
      this.syncButtons();
      return;
    }

    // Stop was pressed while the mic/snapshot awaits were in flight.
    if (this.stopRequested) {
      this.teardownAudio();
      this.saved = null;
      this.starting = false;
      this.syncButtons();
      return;
    }

    this.running = true;
    this.wantRestore = true;
    this.finishPromise = null;
    this.startedAt = performance.now();
    this.framesSent = 0;
    this.bands = silentBands();
    this.viz = new SpectrumViz(this.params);
    app.musicActive = true;
    emit();
    setStatus('Music mode is on — the keyboard lights follow your microphone.');
    this.setPageStatus('Listening… press Stop to restore your lighting.');
    this.syncButtons();
    this.log('Music mode started.');
    this.starting = false;

    // Frame loop: analyse → render → fire-and-forget write, paced at
    // ~30 ms like the vendor command queue (GIF_FRAME_PERIOD_MS).
    this.pumpDone = this.pump();
  }

  private async pump(): Promise<void> {
    const c = app.connected;
    const rig = this.rig;
    const freqData = this.freqData;
    const analyzer = this.analyzer;
    if (!c || !rig || !freqData || !analyzer) {
      this.running = false;
      return;
    }
    const t0 = performance.now();
    let streamError: string | null = null;
    while (this.running) {
      const tickStart = performance.now();
      try {
        rig.analyser.getByteFrequencyData(freqData);
        this.bands = analyzer.analyze(freqData);
        const frame = this.viz.frame(this.bands, (tickStart - t0) / 1000);
        await c.session.writeGifFrame(frame);
        this.framesSent++;
      } catch (e) {
        streamError = e instanceof Error ? e.message : String(e);
        break;
      }
      // Pace to the vendor cadence; don't burst after a slow write.
      const elapsed = performance.now() - tickStart;
      const wait = GIF_FRAME_PERIOD_MS - elapsed;
      await sleep(wait > 0 ? wait : 0);
    }
    this.running = false;
    if (streamError) {
      this.log(`Stream stopped: ${streamError}`);
      setStatus(`Music mode stopped — ${streamError}`, true);
    }
    await this.ensureFinish();
  }

  /**
   * Stop the stream and restore the saved lighting. `restore: false` is
   * used when the device is already gone (disconnect) — there is nothing
   * to write back to.
   */
  async stop(opts: { restore?: boolean; reason?: string } = {}): Promise<void> {
    if (opts.restore === false) this.wantRestore = false;
    if (opts.reason) this.setPageStatus(opts.reason);
    this.stopRequested = true;
    this.running = false;
    await this.pumpDone?.catch(() => {});
    this.pumpDone = null;
    await this.ensureFinish();
  }

  /** Idempotent finish: tear down audio, clear the flag, restore lighting. */
  private ensureFinish(): Promise<void> {
    if (!this.finishPromise) {
      this.finishPromise = this.doFinish(this.wantRestore);
    }
    return this.finishPromise;
  }

  private async doFinish(restore: boolean): Promise<void> {
    this.teardownAudio();
    app.musicActive = this.pixelRunning || this.gifRunning;
    emit();
    this.syncButtons();
    const c = app.connected;
    const saved = this.saved;
    this.saved = null;
    if (!restore || !c || !saved) {
      if (!this.pixelRunning && !this.gifRunning) setStatus('Music mode off.');
      return;
    }
    // Restore pre-run lighting (verified writes, CLI order: profile then
    // light data). Best-effort: report failures, never throw into the UI.
    const results: string[] = [];
    try {
      const out = await c.session.writeProfile(saved.profile);
      results.push(`lighting settings restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore lighting settings: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    try {
      const out = await c.session.writeLightData(saved.lightData);
      results.push(`effect colours restored${out.verified ? ' (checked)' : ''}`);
    } catch (e) {
      results.push(
        `could not restore effect colours: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
    const failed = results.some((r) => r.startsWith('could not'));
    if (!this.pixelRunning && !this.gifRunning) {
      setStatus(
        failed
          ? `Music mode off — ${results.join('; ')}.`
          : 'Music mode off — your lighting is back.',
        failed,
      );
    }
    for (const r of results) this.log(r);
    // The page may have been re-rendered while the writes were in flight.
    if (this.el.isConnected) this.render();
  }

  private teardownAudio(): void {
    const rig = this.rig;
    this.rig = null;
    this.analyzer = null;
    this.freqData = null;
    if (!rig) return;
    try {
      rig.source.disconnect();
    } catch {
      /* already disconnected */
    }
    for (const t of rig.stream.getTracks()) t.stop();
    void rig.ctx.close().catch(() => {});
  }
}

// ── Registry (same singleton-per-mount pattern as MacroPage/AdvPage) ───

let musicPage: MusicPage | null = null;

export function renderMusicPage(root: HTMLElement): void {
  if (musicPage && musicPage.el.isConnected) return;
  musicPage?.destroy();
  musicPage = new MusicPage(root);
}

export function destroyMusicPage(): void {
  musicPage?.destroy();
  musicPage = null;
}
