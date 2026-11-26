/**
 * Unit tests for `src/proto/gifmap.ts` — GIF/image → per-key RGB
 * sampler for the khub916 cmd-8 stream. Node native TS, no DOM.
 *
 *   node scripts/test-gifmap.ts
 */

declare const process: { exit(code?: number): never };

import {
  KEY_SLOTS,
  k1proLedPoints,
  contentBounds,
  sampleSlots,
  type Rgba,
} from '../src/proto/gifmap.ts';

let failures = 0;
function check(name: string, cond: boolean, detail = ''): void {
  if (cond) console.log(`  ok  ${name}`);
  else {
    failures++;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
  }
}
const near = (a: number, b: number, eps = 0.02) => Math.abs(a - b) <= eps;

// ── k1proLedPoints ─────────────────────────────────────────────────────

{
  const pts = k1proLedPoints();
  check('126 points', pts.length === KEY_SLOTS);
  check('all normalized', pts.every((p) => p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1));
  check('radii positive', pts.every((p) => p.rx > 0 && p.ry > 0));
  // Slot 0 = ESC top-left → near top-left corner.
  check('slot0 top-left', pts[0].x < 0.1 && pts[0].y < 0.15, JSON.stringify(pts[0]));
}

// ── contentBounds ──────────────────────────────────────────────────────

function solid(w: number, h: number, rgb: [number, number, number]): Rgba {
  const d = new Uint8ClampedArray(w * h * 4);
  for (let i = 0; i < w * h; i++) {
    d[i * 4] = rgb[0];
    d[i * 4 + 1] = rgb[1];
    d[i * 4 + 2] = rgb[2];
    d[i * 4 + 3] = 255;
  }
  return { data: d, width: w, height: h };
}

{
  // Black frame with a centred red square → bounds around the square.
  const w = 100, h = 100;
  const img = solid(w, h, [0, 0, 0]);
  for (let y = 40; y < 60; y++)
    for (let x = 30; x < 70; x++) {
      const i = (y * w + x) * 4;
      img.data[i] = 255; img.data[i + 1] = 0; img.data[i + 2] = 0;
    }
  const b = contentBounds([img]);
  check('bounds found', b[0] > 0.2 && b[0] < 0.4 && b[2] > 0.6 && b[2] < 0.8, JSON.stringify(b));
  check('bounds y', b[1] > 0.3 && b[3] < 0.7, JSON.stringify(b));
  // Uniform frame → full bounds.
  check('uniform → full', JSON.stringify(contentBounds([solid(50, 50, [9, 9, 9])])) === '[0,0,1,1]');
}

// ── sampleSlots ────────────────────────────────────────────────────────

{
  // 100×100 red frame → every slot red.
  const pts = k1proLedPoints();
  const img = solid(100, 100, [200, 30, 30]);
  const out = sampleSlots(img, pts);
  check('length', out.length === KEY_SLOTS * 3);
  let allRed = true;
  for (let s = 0; s < KEY_SLOTS; s++) {
    if (!(out[s * 3]! > 0.7 && out[s * 3 + 1]! < 0.2 && out[s * 3 + 2]! < 0.2)) allRed = false;
  }
  check('all slots red', allRed, `slot0=${out[0]},${out[1]},${out[2]}`);
}

{
  // Black frame → all zeros (blackLevel gate).
  const out = sampleSlots(solid(100, 100, [0, 0, 0]), k1proLedPoints());
  check('black stays black', out.every((v) => v === 0));
}

{
  // Dim grey below blackLevel → black; above → scaled by brightness.
  const dim = solid(100, 100, [10, 10, 10]);
  const out1 = sampleSlots(dim, k1proLedPoints());
  check('dim gated', out1.every((v) => v === 0));
  const mid = solid(100, 100, [100, 100, 100]);
  const out2 = sampleSlots(mid, k1proLedPoints(), { brightness: 0.5 });
  check('brightness scale', near(out2[0]!, 100 / 255 * 0.5, 0.01), String(out2[0]));
}

{
  // Median robustness: region mostly blue with one red outlier → blue wins.
  const w = 100, h = 100;
  const img = solid(w, h, [0, 0, 200]);
  const pts = [{ x: 0.5, y: 0.5, rx: 0.1, ry: 0.1 }];
  // paint a few red pixels inside the region
  for (let i = 0; i < 5; i++) {
    const idx = ((50 + i) * w + 50) * 4;
    img.data[idx] = 255; img.data[idx + 1] = 0; img.data[idx + 2] = 0;
  }
  const out = sampleSlots(img, pts, { sampling: 'median' });
  check('median picks blue', out[2]! > 0.6 && out[0]! < 0.2, `${out[0]},${out[1]},${out[2]}`);
  const outP = sampleSlots(img, pts, { sampling: 'point' });
  check('point may pick red', outP[0]! > 0.5 || outP[2]! > 0.6);
}

{
  // blackLevel=0 disables the gate.
  const out = sampleSlots(solid(100, 100, [5, 5, 5]), k1proLedPoints(), { blackLevel: 0 });
  check('gate disabled', out[0]! > 0);
}

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log('\nall gifmap tests passed');
