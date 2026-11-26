/**
 * Smoke test for the shared device-config validator (AJV Draft 2020-12
 * against schemas/device-config.schema.json — the same artifact the Rust
 * CLI embeds). Run: `npm run smoke-device-config` or via `npm run smoke`.
 */

declare const process: { exit(code?: number): never };

import { validateDeviceConfig, deviceConfigSchema } from "../src/device-config.ts";
// Shared cross-implementation fixtures — the same cases.json is run by
// schemas/test_schema.py and the Rust config.rs test.
import fixtures from "../../schemas/fixtures/cases.json" with { type: "json" };

const KHUB = { protocol: "khub916", vid: 0x1e7d, pid: 0xf501 };
const MOUSE = { protocol: "compx", vid: 0x3554, pid: 0xf54b };
const HE = { protocol: "he-v2", vid: 0x2e1a, pid: 0x0001 };

const doc = (device: object, extra: object = {}) => ({
  schema_version: 1,
  device,
  ...extra,
});

const VALID: Array<[string, unknown]> = [
  ["khub settings only", doc(KHUB, { settings: { sleep_seconds: 30 } })],
  ["mouse settings replace", doc(MOUSE, {
    settings: {
      mode: "replace",
      report_rate: 1000, stage_count: 2, current_stage: 0,
      dpi: Array(8).fill(800), dpi_colors: Array(8).fill("#ff0000"),
      lod: 1, debounce: 4, motion_sync: true, sleep: 30,
      angle_snap: false, ripple: false, light_off_moving: false,
      sensor_mode: 0,
      buttons: Array(6).fill({ kind: "mouse", param: "left" }),
    },
  })],
  ["macro steps", doc(KHUB, {
    macros: [{ index: 0, steps: [{ action: "down", key: "KeyA", delay_ms: 8 }] }],
  })],
  ["macro case-insensitive name", doc(KHUB, {
    macros: [{ index: 0, steps: [{ action: "down", key: "keya" }] }],
  })],
];

const INVALID: Array<[string, unknown]> = [
  ["unknown top-level field", doc(KHUB, { settings: { sleep_seconds: 1 }, bogus: 1 })],
  ["macro delete not in v1", doc(KHUB, { macros: [{ index: 0, delete: true }] })],
  ["delay_ms + duration both", doc(KHUB, {
    macros: [{ index: 0, steps: [{ action: "down", key: "KeyA", delay_ms: 1, duration: 1 }] }],
  })],
  ["macro unknown key name", doc(KHUB, {
    macros: [{ index: 0, steps: [{ action: "down", key: "NotAKey" }] }],
  })],
  ["macro Fn not encodable", doc(KHUB, {
    macros: [{ index: 0, steps: [{ action: "down", key: "Fn" }] }],
  })],
  ["mouse param 65536", doc(MOUSE, {
    settings: { mode: "patch", buttons: Array(6).fill({ kind: "dpi", param: "65536" }) },
  })],
  ["khub value u32 overflow", doc(KHUB, {
    keymap: { layer: 0, system: 0, mode: "patch",
      entries: [{ index: 0, value: "4294967296" }] },
  })],
  ["empty settings patch", doc(MOUSE, { settings: { mode: "patch" } })],
  ["wrong protocol section", doc(MOUSE, {
    macros: [{ index: 0, steps: [{ action: "down", key: "KeyA" }] }],
  })],
  ["he keymap apply-shape ok but section gated", doc(HE, { keymap: { mode: "replace" } })],
];

let failures = 0;
for (const [name, d] of VALID) {
  const r = validateDeviceConfig(d);
  if (!r.valid) {
    failures++;
    console.error(`FAIL valid case '${name}':`, r.issues);
  }
}
for (const [name, d] of INVALID) {
  const r = validateDeviceConfig(d);
  if (r.valid) {
    failures++;
    console.error(`FAIL invalid case '${name}' passed validation`);
  }
}
for (const c of fixtures.valid) {
  const r = validateDeviceConfig(c.doc);
  if (!r.valid) {
    failures++;
    console.error(`FAIL fixture valid '${c.name}':`, r.issues);
  }
}
for (const c of fixtures.invalid) {
  const r = validateDeviceConfig(c.doc);
  if (r.valid) {
    failures++;
    console.error(`FAIL fixture invalid '${c.name}' passed validation`);
  }
}
if (typeof deviceConfigSchema() !== "object") {
  failures++;
  console.error("FAIL schema artifact not exposed");
}
if (failures) {
  console.error(`${failures} smoke-device-config failure(s)`);
  process.exit(1);
}
console.log(
  `smoke-device-config: ${VALID.length} valid + ${INVALID.length} invalid cases + ` +
    `${fixtures.valid.length} valid + ${fixtures.invalid.length} shared fixtures OK`,
);
