/**
 * Unit tests for `src/config-doc.ts` — the web mirror of `omg config get`:
 * current device state → schema-valid desired-state doc, plus the
 * FIELD_GUIDE inlined into the "Copy AI instructions" prompt.
 *
 * Runs under Node's native TS stripping: `node scripts/test-config-doc.ts`
 *
 * No DOM, no device: inputs are synthetic `ConfigStateInput` values.
 */

declare const process: { exit(code?: number): never };

import { buildStateDoc, FIELD_GUIDE } from '../src/config-doc.ts';
import { validateDeviceConfig } from '../src/device-config.ts';
import type { ProfileInfo } from '../src/proto/khub916.ts';
import type { ConfigStateInput } from '../src/config-doc.ts';

let failures = 0;
function check(name: string, cond: boolean, detail = ''): void {
  if (cond) {
    console.log(`  ok  ${name}`);
  } else {
    failures++;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

/** 128-byte profile blob: mode 2, brightness 10, speed 1, colorModel 2, sleep 30. */
function fakeProfile(): ProfileInfo {
  const raw = new Uint8Array(128);
  raw.fill(0);
  raw[9] = 0;
  raw[10] = 2; // mode low byte (breathing, non-special)
  raw[36] = 0xff;
  raw[37] = 0xff; // FF FF pair → base = 38
  // decode: idx=raw[10]=2 → brightAt = 38 + 2*2 - 2 = 40, packedAt = 41
  raw[40] = 10; // brightness
  raw[41] = 0x17; // packed: speed 1, low nibble 7 → mixing
  raw[24] = 30; // sleep_seconds
  raw[126] = 0x5a;
  raw[127] = 0xa5;
  return { raw, lightMode: 2, sleepSeconds: 30 };
}

function baseInput(over: Partial<ConfigStateInput> = {}): ConfigStateInput {
  return {
    vid: 9610,
    pid: 268,
    usagePage: 65280,
    profile: fakeProfile(),
    keyValues: [0x00000029, 0x00000035, 0x0000002b],
    layer: 0,
    system: 0,
    macroRegion: null,
    ...over,
  };
}

// ── buildStateDoc ──────────────────────────────────────────────────────

{
  const r = buildStateDoc(baseInput());
  check('build ok', r.ok === true, r.ok ? '' : r.error);
  if (r.ok) {
    const d = r.doc;
    check('schema_version 1', d.schema_version === 1);
    check('device block', JSON.stringify(d.device) === JSON.stringify({
      protocol: 'khub916', vid: 9610, pid: 268, usage_page: 65280,
    }));
    check('settings.sleep_seconds', (d.settings as any).sleep_seconds === 30);
    const light = d.lighting as any;
    check('lighting.mode', light?.mode === 2);
    check('lighting.brightness', light?.brightness === 10);
    check('lighting.speed', light?.speed === 1);
    check('lighting.color_model', light?.color_model === 2);
    const km = d.keymap as any;
    check('keymap replace', km?.mode === 'replace' && km.layer === 0 && km.system === 0);
    check('keymap values hex', km?.values?.[0] === '0x00000029' && km?.values?.[2] === '0x0000002B');
    check('no macros section when region null', !('macros' in d));
    check('warnings mention nothing fatal', r.warnings.every((w) => !w.includes('failed')));
    const v = validateDeviceConfig(d);
    check('doc validates', v.valid, JSON.stringify(v.issues.slice(0, 3)));
  }
}

{
  // Out-of-range light fields are dropped with a warning, doc stays valid.
  const raw = new Uint8Array(128);
  raw[9] = 0; raw[10] = 2;
  raw[36] = 0xff; raw[37] = 0xff; // base = 38 → brightAt 40, packedAt 41
  raw[40] = 99; // brightness > 20
  raw[41] = 0x17; // speed 1, low nibble 7 → mixing
  raw[24] = 5;
  const r = buildStateDoc(baseInput({ profile: { raw, lightMode: 2, sleepSeconds: 5 } }));
  check('oor build ok', r.ok === true, r.ok ? '' : r.error);
  if (r.ok) {
    const light = r.doc.lighting as any;
    check('oor brightness omitted', light && !('brightness' in light));
    check('oor warning emitted', r.warnings.some((w) => w.includes('brightness')));
    check('oor doc still valid', validateDeviceConfig(r.doc).valid);
  }
}

{
  // Missing keymap read → section omitted + warning, still valid.
  const r = buildStateDoc(baseInput({ keyValues: null }));
  check('no-keymap build ok', r.ok === true, r.ok ? '' : r.error);
  if (r.ok) {
    check('keymap omitted', !('keymap' in r.doc));
    check('keymap warning', r.warnings.some((w) => w.includes('keymap')));
    check('still valid', validateDeviceConfig(r.doc).valid);
  }
}

{
  // usagePage 0/absent → usage_page omitted (schema-optional).
  const r = buildStateDoc(baseInput({ usagePage: null }));
  check('no usage_page', r.ok === true && !('usage_page' in (r.doc.device as any)));
}

// ── FIELD_GUIDE ────────────────────────────────────────────────────────

check('FIELD_GUIDE non-empty', FIELD_GUIDE.length > 10);
for (const needle of ['schema_version', 'keymap.mode', 'sleep_seconds', 'macros', 'advanced_keys', 'lighting']) {
  check(`guide covers ${needle}`, FIELD_GUIDE.some((l) => l.includes(needle)));
}

if (failures > 0) {
  console.error(`\n${failures} failure(s)`);
  process.exit(1);
}
console.log('\nall config-doc tests passed');
