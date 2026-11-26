/**
 * Desired-state config → web apply lowering (docs/device-config-schema.md).
 *
 * Layer 2/3 for the Custom config page: takes a document that already
 * passed `validateDeviceConfig` (layer 1, shared AJV artifact) plus the
 * connected khub916 session and produces:
 *
 *   - `planConfigSections` — the same capability gate the Rust CLI's
 *     `plan_sections` applies (semantic layer, per implementation);
 *   - `configSemanticErrors` — the doc-internal checks the schema cannot
 *     express (keymap slot conflicts, duplicates);
 *   - `configToOps` — `PlannedOp[]` for `runTransaction`, so every write
 *     goes through policy classification + approval + journal + stop-on-
 *     first-failure, with a `SnapshotStore` restore point captured by the
 *     caller before the transaction starts.
 *
 * Only the khub916 wired subset is lowered — the same sections the Rust
 * `omg config apply` executes today (keymap patch/replace, macros +
 * binds, settings.sleep_seconds). Everything else reports `not_applied`
 * with the same reasons, never a silent skip.
 */

import type { PlannedOp, WriteOutcomeLike } from './safety/transaction.ts';
import type { DeviceSnapshot, SnapshotSectionName } from './safety/snapshots.ts';
import {
  parseMacroRegion,
  buildMacroRegion,
  parseStepsJson,
  macroKeyValue,
  type MacroEntry,
  type StepKind,
} from './proto/macro916.ts';
import { profileWithSleepSeconds } from './proto/khub916.ts';

/** Structural subset of `Khub916Session` this module needs (hid.ts). */
export interface ConfigSession {
  keyValues(arg: number): Promise<number[]>;
  writeKeymap(layer: number, system: number, values: readonly number[]): Promise<WriteOutcomeLike>;
  macroRegion(): Promise<Uint8Array>;
  writeMacroRegion(data: Uint8Array): Promise<WriteOutcomeLike>;
  profile(): Promise<{ raw: Uint8Array }>;
  writeProfile(profile: Uint8Array): Promise<WriteOutcomeLike>;
}

export interface ConfigSectionPlan {
  name: string;
  status: 'planned' | 'not_applied';
  detail: string;
}

const SECTION_KEYS = ['keymap', 'lighting', 'settings', 'macros', 'advanced_keys'] as const;

function keymapArg(layer: number, system: number): number {
  return (layer & 3) | ((system & 1) << 2);
}

/**
 * Capability gate — mirrors `plan_sections` in crates/omg/src/config.rs.
 * `protocol` is the document's `device.protocol`; the connected-device
 * check is the caller's job (protocol + vid/pid already validated
 * structurally, matched here against the live device).
 */
export function planConfigSections(doc: Record<string, unknown>): ConfigSectionPlan[] {
  const protocol = (doc.device as Record<string, unknown> | undefined)?.protocol;
  const out: ConfigSectionPlan[] = [];
  for (const key of SECTION_KEYS) {
    const section = doc[key];
    if (section === undefined || section === null || typeof section !== 'object') continue;
    const mode = (section as Record<string, unknown>).mode;
    const modeStr = typeof mode === 'string' ? mode : '';
    let ok = false;
    let why = 'no apply path for this section/protocol';
    if (protocol === 'khub916') {
      if (key === 'keymap' && (modeStr === 'patch' || modeStr === 'replace')) {
        ok = true;
      } else if (key === 'macros' && modeStr === '') {
        ok = true;
      } else if (key === 'settings' && modeStr === '') {
        ok = true;
      } else if (key === 'lighting') {
        why = 'lighting apply lowering not yet wired to desired-state';
      } else if (key === 'advanced_keys') {
        why = 'advanced_keys apply lowering not yet wired to desired-state';
      } else {
        why = 'no apply path for this section/protocol';
      }
    } else if (protocol === 'compx') {
      why =
        modeStr === 'patch'
          ? 'patch mode is schema-valid but apply-rejected: no read-merge path yet'
          : 'the web app has no Compx mouse write path for desired-state docs';
    } else if (protocol === 'he-v2') {
      why = 'he-v2 apply lowering not yet implemented';
    } else if (protocol === 'sparkplayjoy') {
      why = 'sparkplayjoy is read-only';
    }
    out.push({ name: key, status: ok ? 'planned' : 'not_applied', detail: ok ? 'supported' : why });
  }
  return out;
}

/**
 * Doc-internal semantic checks the schema cannot express — mirrors
 * `semantic_errors` in config.rs: duplicate patch indices, duplicate
 * macro indices, and (arg, slot) conflicts between every implicit
 * keymap writer — `keymap.entries`/`keymap.values` (replace),
 * `macros[].bind`, and `advanced_keys` (always arg 0).
 */
export function configSemanticErrors(doc: Record<string, unknown>): string[] {
  const errs: string[] = [];
  const protocol = (doc.device as Record<string, unknown> | undefined)?.protocol;
  if (
    protocol === 'sparkplayjoy' &&
    ['keymap', 'lighting', 'settings', 'macros', 'advanced_keys'].some((k) => k in doc)
  ) {
    errs.push('sparkplayjoy has no writable config sections');
  }
  const targets = new Map<string, string>();
  const claim = (arg: number, slot: number, who: string): void => {
    const k = `${arg}|${slot}`;
    const prev = targets.get(k);
    if (prev !== undefined) {
      errs.push(`keymap slot conflict: ${who} and ${prev} both target arg ${arg} slot ${slot}`);
    }
    targets.set(k, who);
  };
  const km = doc.keymap as Record<string, unknown> | undefined;
  if (km && typeof km === 'object') {
    const layer = typeof km.layer === 'number' ? km.layer : 0;
    const system = typeof km.system === 'number' ? km.system : 0;
    const arg = keymapArg(layer, system);
    const entries = km.entries;
    if (Array.isArray(entries)) {
      const seen = new Set<number>();
      for (const e of entries) {
        const idx = (e as Record<string, unknown>).index;
        if (typeof idx === 'number') {
          if (seen.has(idx)) errs.push(`keymap patch: duplicate index ${idx}`);
          seen.add(idx);
          claim(arg, idx, 'keymap.entries');
        }
      }
    }
    // Replace writes the whole table — every position is a target.
    if (km.mode === 'replace' && Array.isArray(km.values)) {
      for (let slot = 0; slot < km.values.length; slot++) {
        claim(arg, slot, 'keymap.values');
      }
    }
  }
  const macros = doc.macros;
  if (Array.isArray(macros)) {
    // Two entries resolving to the same region slot would overwrite each
    // other — explicit numeric duplicates are detectable here; "next"
    // collisions depend on the live region and are rejected in applyMacros.
    const idxSeen = new Set<number>();
    macros.forEach((m, i) => {
      const rec = m as Record<string, unknown>;
      const idx = rec.index;
      if (typeof idx === 'number' && Number.isInteger(idx)) {
        if (idxSeen.has(idx)) errs.push(`macros: duplicate index ${idx}`);
        idxSeen.add(idx);
      }
      const b = rec.bind as Record<string, unknown> | undefined;
      if (b && typeof b === 'object') {
        const layer = typeof b.layer === 'number' ? b.layer : 0;
        const system = typeof b.system === 'number' ? b.system : 0;
        const key = typeof b.key === 'number' ? b.key : Number.MAX_SAFE_INTEGER;
        claim(keymapArg(layer, system), key, `macros[${i}].bind`);
      }
    });
  }
  // advanced_keys entries implicitly write keymap arg 0 at their slot(s)
  // (D7): mt→key, socd→key_a+key_b, tgl→key+toggle.
  if (Array.isArray(doc.advanced_keys)) {
    doc.advanced_keys.forEach((e, i) => {
      const rec = e as Record<string, unknown>;
      const fields: Record<string, string[]> = {
        mt: ['key'],
        tgl: ['key', 'toggle'],
        socd: ['key_a', 'key_b'],
      };
      for (const f of fields[String(rec.kind)] ?? []) {
        const slot = rec[f];
        if (typeof slot === 'number') claim(0, slot, `advanced_keys[${i}]`);
      }
    });
  }
  return errs;
}

/**
 * Snapshot sections each planned op needs for a proven rollback. A
 * desired-state apply must NOT proceed when any of these is missing or
 * failed to capture — the user was promised a restore point.
 */
export const NEEDED_SNAPSHOT: Record<string, SnapshotSectionName[]> = {
  keymap: ['keymap'],
  // macros writes the region AND tag-3 bind slots inside keymap tables.
  macros: ['macro', 'keymap'],
  // settings.sleep_seconds rewrites the profile blob.
  settings: ['profile'],
};

/** Deterministic JSON: object keys sorted recursively (arrays keep order). */
function canonicalJson(v: unknown): string {
  if (v === null || typeof v !== 'object') return JSON.stringify(v);
  if (Array.isArray(v)) return `[${v.map(canonicalJson).join(',')}]`;
  const o = v as Record<string, unknown>;
  return `{${Object.keys(o)
    .sort()
    .map((k) => `${JSON.stringify(k)}:${canonicalJson(o[k])}`)
    .join(',')}}`;
}

/**
 * sha256 (hex) of the canonical apply plan — the web counterpart of
 * `plan_hash` in crates/omg/src/config.rs. Binds the confirm-dialog
 * approval to exactly the previewed document, section plan and device
 * identity; any change produces a different hash.
 */
export async function configPlanHash(
  doc: Record<string, unknown>,
  plans: readonly ConfigSectionPlan[],
  device?: { vendorId?: number; productId?: number; usagePage?: number },
): Promise<string> {
  const payload = canonicalJson({
    doc,
    device: device ?? null,
    sections: plans.map((p) => ({ name: p.name, status: p.status, detail: p.detail })),
  });
  const digest = await crypto.subtle.digest(
    'SHA-256',
    new TextEncoder().encode(payload),
  );
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Sections the ops require that the snapshot failed to capture —
 * non-empty means abort before any write. Pure so the gate is smoke-
 * testable without a DOM.
 */
export function missingSnapshotSections(
  ops: readonly PlannedOp[],
  snap: Pick<DeviceSnapshot, 'sections' | 'errors'>,
): SnapshotSectionName[] {
  const needed = new Set<SnapshotSectionName>();
  for (const op of ops) {
    for (const s of NEEDED_SNAPSHOT[op.section ?? ''] ?? []) needed.add(s);
  }
  return [...needed].filter(
    (s) => snap.sections[s] === undefined || snap.errors?.[s] !== undefined,
  );
}

// ── Lowering ────────────────────────────────────────────────────────────

/** `parse_kb916_key_value` port: 0x hex / decimal / none|empty|- → u32. */
function parseKeyValue(s: string): number {
  const t = s.trim();
  if (t === 'none' || t === 'empty' || t === '-') return 0;
  const hex = t.startsWith('0x') || t.startsWith('0X') ? t.slice(2) : null;
  const n = Number.parseInt(hex ?? t, hex !== null ? 16 : 10);
  if (Number.isNaN(n) || n < 0 || n > 0xffff_ffff || !Number.isInteger(n)) {
    throw new Error(`bad key value '${s}' (want 0x hex, decimal, or 'none')`);
  }
  return n >>> 0;
}

function keymapLayerSystem(km: Record<string, unknown>): { layer: number; system: number } {
  return {
    layer: typeof km.layer === 'number' ? km.layer : 0,
    system: typeof km.system === 'number' ? km.system : 0,
  };
}

/** Steps that cannot survive a region rebuild — mirrors the Rust
 *  `check_region_preservable` guard (StepKind::Other = numeric kind). */
function hasOpaqueSteps(e: MacroEntry): string | null {
  for (let i = 0; i < e.steps.length; i++) {
    const k: StepKind = e.steps[i]!.kind;
    if (typeof k === 'number') return `entry "${e.name}" step ${i} (kind ${k})`;
  }
  return null;
}

async function applyKeymapPatch(
  session: ConfigSession,
  km: Record<string, unknown>,
): Promise<WriteOutcomeLike> {
  const { layer, system } = keymapLayerSystem(km);
  const arg = keymapArg(layer, system);
  const edits = new Map<number, number>();
  for (const e of (km.entries as Record<string, unknown>[] | undefined) ?? []) {
    const idx = e.index as number;
    edits.set(idx, parseKeyValue(String(e.value)));
  }
  const base = await session.keyValues(arg);
  if (base.length === 0) {
    throw new Error(`keymap read for arg ${arg} returned no entries — refusing to write`);
  }
  const values = base.slice();
  for (const [idx, v] of edits) {
    if (idx >= values.length) {
      throw new Error(`keymap index ${idx} out of range: arg has ${values.length} slots`);
    }
    values[idx] = v;
  }
  return session.writeKeymap(layer, system, values);
}

async function applyKeymapReplace(
  session: ConfigSession,
  km: Record<string, unknown>,
): Promise<WriteOutcomeLike> {
  const { layer, system } = keymapLayerSystem(km);
  const arg = keymapArg(layer, system);
  const wants = ((km.values as unknown[] | undefined) ?? []).map((v) =>
    parseKeyValue(String(v)),
  );
  const base = await session.keyValues(arg);
  if (base.length === 0) {
    throw new Error(`keymap read for arg ${arg} returned no entries — refusing to write`);
  }
  if (wants.length !== base.length) {
    throw new Error(
      `keymap replace: config has ${wants.length} values but device reports ${base.length} slots`,
    );
  }
  return session.writeKeymap(layer, system, wants);
}

/** Resolve `"next"` indices in document order (appends), like config.rs. */
function resolveMacroIndices(macros: Record<string, unknown>[], count: number): number[] {
  let next = count;
  return macros.map((m) => {
    const idx = m.index;
    if (idx === 'next') {
      const s = next;
      next += 1;
      return s;
    }
    if (typeof idx === 'number' && Number.isInteger(idx)) return idx;
    throw new Error('macros[]: bad index');
  });
}

async function applyMacros(
  session: ConfigSession,
  macros: Record<string, unknown>[],
): Promise<WriteOutcomeLike> {
  const raw = await session.macroRegion();
  const entries = parseMacroRegion(raw);
  const slots = resolveMacroIndices(macros, entries.length);

  // Two doc entries resolving to the same slot would overwrite each
  // other in order — reject instead of silently picking a winner.
  if (new Set(slots).size !== slots.length) {
    throw new Error('macros: duplicate resolved index — entries would overwrite each other');
  }

  // Fail closed: untouched entries must be preservable (no opaque steps).
  const replaced = new Set(slots.filter((s) => s < entries.length));
  for (let i = 0; i < entries.length; i++) {
    if (replaced.has(i)) continue;
    const bad = hasOpaqueSteps(entries[i]!);
    if (bad) {
      throw new Error(
        `refusing to rebuild macro region: ${bad} uses an unrecognised step kind ` +
          'that cannot round-trip — edit it in the Macro Studio or leave it untouched',
      );
    }
  }

  for (let i = 0; i < macros.length; i++) {
    const m = macros[i]!;
    const slot = slots[i]!;
    const payload = { name: m.name, steps: m.steps };
    const input = parseStepsJson(JSON.stringify(payload));
    const name =
      (typeof m.name === 'string' ? m.name : null) ??
      input.name ??
      (slot < entries.length ? entries[slot]!.name : `macro${slot}`);
    if (slot === entries.length) {
      entries.push({ name, steps: input.steps });
    } else if (slot > entries.length) {
      throw new Error(`macros[${i}]: index ${slot} out of range (${entries.length} entries)`);
    } else {
      entries[slot] = { name, steps: input.steps };
    }
  }

  // Bind preflight — BEFORE the region write (mirrors config.rs): every
  // condition that can fail must pass while nothing has been written,
  // so a bad bind can never leave a rewritten region behind a failed op.
  interface BindPlan {
    layer: number;
    system: number;
    key: number;
    value: number;
  }
  const bindPlans: BindPlan[] = [];
  {
    const argCache = new Map<number, number[]>();
    for (let i = 0; i < macros.length; i++) {
      const b = macros[i]!.bind as Record<string, unknown> | undefined;
      if (!b || typeof b !== 'object') continue;
      const slot = slots[i]!;
      if (slot > 0xff) {
        throw new Error(`macros[${i}]: index ${slot} does not fit the tag-3 u8 field`);
      }
      const layer = typeof b.layer === 'number' ? b.layer : 0;
      const system = typeof b.system === 'number' ? b.system : 0;
      const key = typeof b.key === 'number' ? b.key : Number.MAX_SAFE_INTEGER;
      const mtype = typeof b.macro_type === 'number' ? b.macro_type : 0;
      const loops = typeof b.loop_count === 'number' ? b.loop_count : 0;
      if (loops < 1) throw new Error(`macros[${i}].bind: loop_count must be >= 1`);
      const arg = keymapArg(layer, system);
      if (!argCache.has(arg)) argCache.set(arg, await session.keyValues(arg));
      const base = argCache.get(arg)!;
      if (key >= base.length) {
        throw new Error(`macros[${i}].bind: key slot ${key} out of range`);
      }
      bindPlans.push({ layer, system, key, value: macroKeyValue(mtype, loops, slot) });
    }
  }

  const region = buildMacroRegion(entries);
  const out = await session.writeMacroRegion(region);
  // An unverified region write must not be followed by bind writes —
  // the transaction records `unverified` and (stopOnUnverified) halts.
  if (out.verified === false) {
    return out;
  }

  // binds: tag-3 keymap writes (macroKeyValue port of the vendor bind).
  for (const bp of bindPlans) {
    const arg = keymapArg(bp.layer, bp.system);
    const base = await session.keyValues(arg);
    const values = base.slice();
    values[bp.key] = bp.value;
    await session.writeKeymap(bp.layer, bp.system, values);
  }
  return out;
}

async function applySleepSeconds(
  session: ConfigSession,
  st: Record<string, unknown>,
): Promise<WriteOutcomeLike> {
  const secs = st.sleep_seconds;
  if (typeof secs !== 'number' || !Number.isInteger(secs)) {
    throw new Error('settings.sleep_seconds must be an integer');
  }
  const p = await session.profile();
  const blob = profileWithSleepSeconds(p.raw, secs);
  return session.writeProfile(blob);
}

/**
 * Lower a validated khub916 doc into transaction ops. Order matches the
 * Rust apply: keymap → macros (incl. binds) → settings. Ops for sections
 * the plan marked `not_applied` are never produced — the caller reports
 * them separately.
 */
export function configToOps(
  doc: Record<string, unknown>,
  session: ConfigSession,
): PlannedOp[] {
  const protocol = (doc.device as Record<string, unknown> | undefined)?.protocol;
  if (protocol !== 'khub916') return [];
  const ops: PlannedOp[] = [];

  const km = doc.keymap as Record<string, unknown> | undefined;
  if (km && typeof km === 'object') {
    const mode = km.mode;
    ops.push({
      type: 'keymap_write',
      section: 'keymap',
      label: `keymap layer ${km.layer ?? 0} system ${km.system ?? 0} (${String(mode)})`,
      payload: km,
      apply: () =>
        mode === 'replace' ? applyKeymapReplace(session, km) : applyKeymapPatch(session, km),
    });
  }

  const macros = doc.macros;
  if (Array.isArray(macros) && macros.length > 0) {
    ops.push({
      type: 'macro_write',
      section: 'macros',
      label: `macros (${macros.length} entr${macros.length === 1 ? 'y' : 'ies'})`,
      payload: macros,
      apply: () => applyMacros(session, macros as Record<string, unknown>[]),
    });
  }

  const st = doc.settings as Record<string, unknown> | undefined;
  if (st && typeof st === 'object' && st.sleep_seconds !== undefined) {
    ops.push({
      type: 'settings_write',
      section: 'settings',
      label: `settings.sleep_seconds = ${String(st.sleep_seconds)}`,
      payload: st,
      apply: () => applySleepSeconds(session, st),
    });
  }
  return ops;
}
