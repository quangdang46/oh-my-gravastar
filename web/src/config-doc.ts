/**
 * Desired-state document builders for the Custom config page
 * (docs/device-config-schema.md).
 *
 * `buildStateDoc` mirrors `omg config get` on the CLI: it assembles a
 * schema-valid document from an already-read device snapshot (profile,
 * keymap table, macro region) so the user can hand a *real* current
 * state — not a hand-written draft — to the "Copy AI instructions"
 * flow. `FIELD_GUIDE` is the field-by-field reference inlined into that
 * prompt so the external chatbot knows what every path means (the JSON
 * Schema artifact itself carries almost no descriptions).
 *
 * Pure module — no `app`/HID imports — so `scripts/test-config-doc.ts`
 * can exercise it under Node's native TS runner.
 */

import type { ProfileInfo } from './proto/khub916.ts';
import { parseMacroRegion, type MacroEntry, type MacroStep } from './proto/macro916.ts';
import { decodeLightState } from './proto/light.ts';
import { validateDeviceConfig, type DeviceConfigIssue } from './device-config.ts';

/** Everything `buildStateDoc` needs — assembled from `app` by the page. */
export interface ConfigStateInput {
  /** USB vendor id (decimal, `HIDDevice.vendorId`). */
  vid: number;
  /** USB product id (decimal, `HIDDevice.productId`). */
  pid: number;
  /** HID usage page when known (`HIDDevice.usagePage`), else null. */
  usagePage: number | null;
  /** cmd-132 profile blob wrapper (`snapshot.profile`). */
  profile: ProfileInfo;
  /**
   * Raw u32 key values for the selected (layer, system) table, or null
   * when the table was not read. Emitted as a `replace` keymap section.
   */
  keyValues: readonly number[] | null;
  /** Keymap layer 0–3 written into the doc's `keymap.layer`. */
  layer: number;
  /** Keymap system/layout bit 0–1 written into `keymap.system`. */
  system: number;
  /** cmd-133 macro region bytes (`snapshot.macro`), or null if unread. */
  macroRegion: Uint8Array | null;
}

export type ConfigStateResult =
  | { ok: true; doc: Record<string, unknown>; warnings: string[] }
  | { ok: false; error: string };

/** u32 → the `khubKeyValue` hex-string grammar (`0x%08X`). */
function keyValueString(v: number): string {
  return `0x${(v >>> 0).toString(16).padStart(8, '0').toUpperCase()}`;
}

/**
 * One macro step → schema `macroStep`, or null when the step kind is
 * opaque (`StepKind` numeric = wire `Other`; mirrors Rust
 * `StepKind::Other` — never rendered as a named kind, §Safety 7).
 */
function macroStepJson(s: MacroStep): Record<string, unknown> | null {
  if (typeof s.kind === 'number') return null;
  const step: Record<string, unknown> = {
    action: s.action === 0 ? 'down' : 'up',
    key: `0x${s.usage.toString(16).padStart(2, '0').toUpperCase()}`,
    kind: s.kind,
  };
  if (s.delayMs > 0) step.delay_ms = s.delayMs;
  return step;
}

/**
 * One parsed macro entry → schema `macroEntry` (`{index, name?, steps}`),
 * or null when any step is opaque or the entry has no steps. `index` is
 * the entry's position in the region — the same slot `config apply`
 * patch writes target.
 */
function macroEntryJson(index: number, e: MacroEntry): Record<string, unknown> | null {
  const steps: Record<string, unknown>[] = [];
  for (const s of e.steps) {
    const j = macroStepJson(s);
    if (j === null) return null;
    steps.push(j);
  }
  if (steps.length === 0) return null;
  const out: Record<string, unknown> = { index, steps };
  if (e.name) out.name = e.name;
  return out;
}

/**
 * Assemble the current-state desired doc. Sections the snapshot cannot
 * represent are omitted with a warning — the result is always run
 * through `validateDeviceConfig` before being returned, so a bug here
 * fails loudly instead of emitting an invalid doc.
 */
export function buildStateDoc(input: ConfigStateInput): ConfigStateResult {
  const warnings: string[] = [];
  const doc: Record<string, unknown> = {
    schema_version: 1,
    device: {
      protocol: 'khub916',
      vid: input.vid,
      pid: input.pid,
      ...(input.usagePage && input.usagePage > 0 ? { usage_page: input.usagePage } : {}),
    },
  };

  // settings — only sleep_seconds has a schema field today.
  if (input.profile.sleepSeconds !== null && input.profile.sleepSeconds !== undefined) {
    doc.settings = { sleep_seconds: input.profile.sleepSeconds };
  }

  // lighting — sparse scalars only (same fields `omg config get` emits).
  const ls = decodeLightState(input.profile.raw);
  if (ls) {
    const lighting: Record<string, unknown> = { mode: ls.mode };
    if (ls.speed <= 20) lighting.speed = ls.speed;
    else warnings.push(`light.speed=${ls.speed} outside schema range — omitted`);
    if (ls.brightness <= 20) lighting.brightness = ls.brightness;
    else warnings.push(`light.brightness=${ls.brightness} outside schema range — omitted`);
    if (ls.colorModel === 1 || ls.colorModel === 2) lighting.color_model = ls.colorModel;
    else warnings.push(`light.color_model=${ls.colorModel} outside schema range — omitted`);
    doc.lighting = lighting;
  } else {
    warnings.push('profile blob did not decode a light state — lighting section omitted');
  }

  // keymap — one (layer, system) table as a full `replace`.
  if (input.keyValues && input.keyValues.length > 0) {
    doc.keymap = {
      layer: input.layer,
      system: input.system,
      mode: 'replace',
      values: input.keyValues.map(keyValueString),
    };
  } else {
    warnings.push('keymap table not read — keymap section omitted');
  }

  // macros — patch entries; opaque-step entries skipped with a warning.
  if (input.macroRegion && input.macroRegion.length > 0) {
    try {
      const entries = parseMacroRegion(input.macroRegion);
      const skipped: number[] = [];
      const arr = entries
        .map((e, i) => {
          const j = macroEntryJson(i, e);
          if (j === null) skipped.push(i);
          return j;
        })
        .filter((j): j is Record<string, unknown> => j !== null);
      if (skipped.length > 0) {
        warnings.push(
          `macro entries ${skipped.join(',')} contain step kinds the config format cannot represent — omitted`,
        );
      }
      if (arr.length > 0) doc.macros = arr;
    } catch (e) {
      warnings.push(`macro region did not decode — macros section omitted (${(e as Error).message})`);
    }
  }

  const check = validateDeviceConfig(doc);
  if (!check.valid) {
    const detail = check.issues
      .slice(0, 5)
      .map((i: DeviceConfigIssue) => `${i.path}: ${i.message}`)
      .join('; ');
    return { ok: false, error: `built doc failed schema validation: ${detail}` };
  }
  return { ok: true, doc, warnings };
}

/**
 * Field-by-field reference inlined into the "Copy AI instructions"
 * prompt. The schema artifact has almost no `description` fields, so
 * without this the chatbot only sees JSON shapes, not semantics.
 * Keep in sync with `schemas/device-config.schema.json`.
 */
export const FIELD_GUIDE: readonly string[] = [
  'Field reference (path — meaning):',
  '- schema_version: always 1.',
  '- device.protocol: "khub916" | "he-v2" | "compx" | "sparkplayjoy" — must match the connected device.',
  '- device.vid / device.pid: USB vendor/product ids in decimal (e.g. K1 Pro 9610/268).',
  '- device.usage_page: HID usage page, optional (khub916 wired: 65280).',
  '- device.model: optional human model string.',
  '- keymap.layer: 0-3 (which of the 4 key layers). keymap.system: 0-1 (layout/system bank).',
  '- keymap.mode: "patch" changes only listed slots via entries[{index,value}]; "replace" rewrites the whole table via values[] (full array required).',
  '- keymap slot index: 0-127, physical position in the layer table.',
  '- key value grammar (keymap values, macro keys): "none" | "empty" | "-" | "0x" + hex (e.g. "0x00070004") | decimal u32. 0x00000000 = empty slot.',
  '- lighting (khub916 sparse patch — set only the fields you want to change):',
  '  - mode: effect id 0-65535 (0 = off; see docs/lighting-schema.md for the mode table).',
  '  - speed: 0-20. brightness: 0-20.',
  '  - color_model: 1 = monochrome, 2 = colour mixing.',
  '  - color: "#rrggbb" for the active mode.',
  '  - keys: per-key colours — {preset:"solid"|"gradient"|"rainbow", color, to, entries:[{index|pos,color}], colors:[126 colors]}.' ,
  '- settings.sleep_seconds: 0-255, seconds until keyboard sleep (khub916 only write path today).',
  '- macros: patch-only array of {index, name?, steps[], bind?}. index = slot in the macro region (or "next" for append, which forbids bind).',
  '  - step: {action:"down"|"up" (or 0|1), key:"0xNN" usage byte or key name, kind:"key"|"modifier"|"mouse", delay_ms:0-1048575}.',
  '  - Do NOT invent step kinds — unknown wire kinds cannot round-trip.',
  '- advanced_keys (khub916 patch-only): [{kind:"mt",key,tap,hold,time_ms(0-1000,step10)} | {kind:"socd",key_a,key_b,direction} | {kind:"tgl",key,toggle}] — tap/hold/toggle use the same value grammar as keymap.',
];
