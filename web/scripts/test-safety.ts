/**
 * Unit tests for `src/safety/` — the Phase-0 safety layer
 * (vision-full-custom.md §4). Runs under Node's native TS stripping:
 *
 *   npm test        (node scripts/test-safety.ts)
 *
 * Everything is mocked: an in-memory KeyValueStorage stands in for
 * localStorage, a fake SnapshotSource stands in for Khub916Session reads,
 * and a recording RestoreWriter stands in for the verified write methods.
 * No device, no DOM, no network.
 */

declare const process: { exit(code?: number): never };

import {
  AuditLog,
  SnapshotStore,
  classify,
  dryRun,
  isWriteOp,
  memoryStorage,
  planRestore,
  restoreLast,
  restoreSnapshot,
  rollbackSections,
  runTransaction,
  snapshotBeforeWrite,
  validateKeymapEntry,
  validateLightConfig,
  validateMacroSteps,
  validateProfileJson,
  type KeyValueStorage,
  type PlannedOp,
  type RestoreWriter,
  type SnapshotSource,
} from '../src/safety/index.ts';

let failures = 0;
function check(name: string, cond: unknown, extra = ''): void {
  if (cond) {
    console.log(`  ok  ${name}`);
  } else {
    failures++;
    console.error(`FAIL  ${name} ${extra}`);
  }
}
function section(title: string): void {
  console.log(`\n== ${title}`);
}

// ── Shared fixtures ────────────────────────────────────────────────────

function freshStorage(): KeyValueStorage {
  return memoryStorage();
}

const mockSource: SnapshotSource = {
  keyValues: async (arg) => [0x00001000 + arg, 0x5aa5],
  lightColorData: async () => new Uint8Array([1, 2, 3, 4]),
  customLightColors: async () =>
    Array.from({ length: 126 }, (_, i) => ({ r: i & 0xff, g: 0, b: 0 })),
  profile: async () => ({ raw: new Uint8Array(128).fill(7) }),
  macroRegion: async () => new Uint8Array(512).fill(9),
};

/** Recording writer; `failOn` names a method that should throw. */
function mockWriter(failOn: string[] = []): {
  writer: RestoreWriter;
  calls: { name: string; args: unknown[] }[];
} {
  const calls: { name: string; args: unknown[] }[] = [];
  const rec =
    (name: string) =>
    (...args: unknown[]) => {
      calls.push({ name, args });
      if (failOn.includes(name)) return Promise.reject(new Error(`${name} exploded`));
      return Promise.resolve({ command: 0, verified: true });
    };
  return {
    calls,
    writer: {
      writeKeymap: rec('writeKeymap') as RestoreWriter['writeKeymap'],
      writeLightData: rec('writeLightData') as RestoreWriter['writeLightData'],
      writeCustomLightColors: rec('writeCustomLightColors') as RestoreWriter['writeCustomLightColors'],
      writeMacroRegion: rec('writeMacroRegion') as RestoreWriter['writeMacroRegion'],
      writeProfile: rec('writeProfile') as RestoreWriter['writeProfile'],
    },
  };
}

const approveAll = () => true;

// ══════════════════════════════════════════════════════════════════════
// validation.ts
// ══════════════════════════════════════════════════════════════════════

section('validation — profile JSON');

const goodProfile = {
  schema: 1,
  exported_at: '2026-01-01T00:00:00Z',
  host: 'test',
  devices: [
    {
      protocol: 'khub916',
      vid: 0x258a,
      pid: 0x010c,
      usage_page: 0xff00,
      model: 'K1 Pro',
      profiles: [
        {
          name: 'current',
          state: {
            device_info: { uuid: '0x001122334455' },
            keymap: { '0': ['0x00000004', 30], '7': ['0x5AA50001'] },
            profile: { bytes: 128, light_mode: 15, sleep_seconds: 30, hex: 'aabb' },
            power: { percent: 80, flags: 0 },
            light_colors: [
              { index: 0, pos: 8, r: 255, g: 0, b: 0 },
              { index: 125, pos: 508, r: 0, g: 0, b: 255 },
            ],
            light_data_hex: 'ff00',
            macro_hex: '0001',
          },
        },
      ],
    },
  ],
};

{
  const v = validateProfileJson(goodProfile);
  check('valid schema-1 khub916 profile passes', v.ok, v.errors.join('; '));
}
{
  const v = validateProfileJson(JSON.stringify({ ...goodProfile, schema: 2 }));
  check('schema 2 rejected', !v.ok && v.errors.some((e) => e.includes('schema')));
}
{
  const v = validateProfileJson('{ not json');
  check('invalid JSON rejected without throwing', !v.ok && v.errors[0]?.includes('invalid JSON'));
}
{
  const v = validateProfileJson({ schema: 1, devices: [] });
  check('empty devices rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).keymap = {
    '0': ['0xZZZZZZZZ'],
  };
  const v = validateProfileJson(bad);
  check('bad u32 hex keymap value rejected', !v.ok && v.errors.some((e) => e.includes('u32')));
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).keymap = {
    '0': [0x1_0000_0000],
  };
  const v = validateProfileJson(bad);
  check('keymap value > u32 rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).keymap = {
    '8': ['0x00000001'],
  };
  const v = validateProfileJson(bad);
  check('keymap arg 8 rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).keymap = {
    '0': Array.from({ length: 129 }, (_, i) => i),
  };
  const v = validateProfileJson(bad);
  check('keymap >128 values rejected', !v.ok && v.errors.some((e) => e.includes('128')));
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).light_colors = [
    { index: 3, r: 1, g: 2, b: 3 },
    { index: 3, r: 4, g: 5, b: 6 },
  ];
  const v = validateProfileJson(bad);
  check('duplicate colour index rejected', !v.ok && v.errors.some((e) => e.includes('duplicate')));
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).light_colors = [
    { index: 0, r: 300, g: 0, b: 0 },
  ];
  const v = validateProfileJson(bad);
  check('colour channel >255 rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).macro_hex = 'ab'.repeat(513);
  const v = validateProfileJson(bad);
  check('macro hex >512 bytes rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).light_data_hex = 'abc';
  const v = validateProfileJson(bad);
  check('odd-length hex rejected', !v.ok);
}
{
  const bad = structuredClone(goodProfile);
  (bad.devices[0]!.profiles![0]!.state as Record<string, unknown>).profile = { hex: 'zz' };
  const v = validateProfileJson(bad);
  check('bad profile hex rejected', !v.ok);
}

section('validation — keymap entry');

{
  const v = validateKeymapEntry({ arg: 2, values: ['0x00000004', 30, 0] });
  check('valid keymap entry passes', v.ok, v.errors.join('; '));
}
{
  const v = validateKeymapEntry({ arg: 9, values: [1] });
  check('keymap arg out of range rejected', !v.ok);
}
{
  const v = validateKeymapEntry({ arg: 0, values: [] });
  check('empty keymap values rejected', !v.ok);
}
{
  const v = validateKeymapEntry({ arg: 0, values: ['0x123456789'] });
  check('9-hex-digit u32 rejected', !v.ok);
}

section('validation — macro steps');

{
  const v = validateMacroSteps([
    { action: 'down', key: 'KeyA', delay_ms: 8 },
    { action: 'up', key: 'KeyA', delay_ms: 0 },
  ]);
  check('valid macro steps pass', v.ok, v.errors.join('; '));
}
{
  const v = validateMacroSteps({ name: 'jump', steps: [{ action: 'press', code: 4 }] });
  check('object form with code alias passes', v.ok, v.errors.join('; '));
}
{
  const v = validateMacroSteps([{ action: 'down', key: 'KeyA', delay_ms: 0x100000 }]);
  check('delay_ms over 20-bit max rejected', !v.ok && v.errors.some((e) => e.includes('20-bit')));
}
{
  const v = validateMacroSteps([{ action: 'sideways', key: 'KeyA' }]);
  check('bad action rejected', !v.ok);
}
{
  const v = validateMacroSteps([{ action: 'down', key: 'NotAKey' }]);
  check('unknown key name rejected', !v.ok);
}
{
  const v = validateMacroSteps([]);
  check('empty steps rejected', !v.ok);
}
{
  const v = validateMacroSteps('{oops');
  check('bad JSON rejected without throwing', !v.ok);
}
{
  const v = validateMacroSteps([{ action: 'down', key: 'Fn' }]);
  check('unencodable key (Fn) rejected', !v.ok);
}

section('validation — light config');

{
  const v = validateLightConfig({
    schema: 1,
    mode: 2,
    speed: 8,
    brightness: 5,
    color_model: 1,
    color: '#2244ff',
  });
  check('valid light config passes', v.ok, v.errors.join('; '));
}
{
  const v = validateLightConfig({});
  check('empty light config rejected', !v.ok && v.errors.some((e) => e.includes('nothing')));
}
{
  const v = validateLightConfig({ speed: 21 });
  check('speed >20 rejected', !v.ok);
}
{
  const v = validateLightConfig({ color_model: 3 });
  check('bad color_model rejected', !v.ok);
}
{
  const v = validateLightConfig({ mode: 70000 });
  check('mode >u16 rejected', !v.ok);
}
{
  const v = validateLightConfig({ keys: { colors: ['#ff0000'] } });
  check('keys.colors wrong length rejected', !v.ok && v.errors.some((e) => e.includes('126')));
}
{
  const v = validateLightConfig({ keys: { preset: 'strobe' } });
  check('bad preset rejected', !v.ok);
}
{
  const v = validateLightConfig({
    keys: {
      entries: [
        { index: 0, color: '#00ff00' },
        { pos: 0, color: '#0000ff' },
      ],
    },
  });
  check('duplicate entry index/pos rejected', !v.ok && v.errors.some((e) => e.includes('duplicate')));
}
{
  const v = validateLightConfig({ keys: { entries: [{ index: 200, color: '#fff' }] } });
  check('entry index >125 rejected', !v.ok);
}
{
  const v = validateLightConfig({ color: 'not-a-colour' });
  check('bad colour rejected', !v.ok);
}
{
  const v = validateLightConfig('{bad');
  check('bad light JSON rejected without throwing', !v.ok);
}

// ══════════════════════════════════════════════════════════════════════
// policy.ts
// ══════════════════════════════════════════════════════════════════════

section('policy — classification');

{
  const cases: [Parameters<typeof classify>[0]['type'], string, boolean][] = [
    ['read', 'L0', false],
    ['draft', 'L1', false],
    ['keymap_write', 'L2', true],
    ['light_write', 'L2', true],
    ['macro_write', 'L2', true],
    ['adv_write', 'L2', true],
    ['profile_restore', 'L2', true],
    ['settings_write', 'L2', true],
    ['polling', 'L3', true],
    ['calibration', 'L3', true],
    ['forbidden', 'L4', false],
  ];
  for (const [type, cls, approval] of cases) {
    const d = classify({ type });
    check(`${type} → ${cls} (approval=${approval})`, d.class === cls && d.requiresApproval === approval);
  }
  const f = classify({ type: 'forbidden' });
  check('forbidden is never allowed', f.allowed === false);
  const k = classify({ type: 'keymap_write', section: 'keymap' });
  check('L2 op is allowed but gated', k.allowed === true && k.requiresApproval === true);
  check('isWriteOp true for L2, false for L0', isWriteOp({ type: 'keymap_write' }) && !isWriteOp({ type: 'read' }));
}

// ══════════════════════════════════════════════════════════════════════
// snapshots.ts
// ══════════════════════════════════════════════════════════════════════

section('snapshots — capture / list / get / prune');

{
  const store = new SnapshotStore(freshStorage());
  const snap = await snapshotBeforeWrite(store, mockSource, 'before test write');
  check('capture stores a snapshot', typeof snap.id === 'string' && snap.label === 'before test write');
  check('capture got all four sections',
    snap.sections.keymap !== undefined &&
      snap.sections.lighting !== undefined &&
      snap.sections.macro !== undefined &&
      snap.sections.profile !== undefined);
  check('keymap captured 8 args', Object.keys(snap.sections.keymap ?? {}).length === 8);
  check('macro stored as 1024 hex chars', (snap.sections.macro ?? '').length === 1024);

  const got = store.get(snap.id);
  check('get() round-trips the snapshot', got?.id === snap.id && got.sections.profile === snap.sections.profile);

  const list = store.list();
  check('list() shows one complete snapshot', list.length === 1 && list[0]?.partial === false);

  const snap2 = await store.capture(mockSource, 'second');
  check('second capture listed newest-first', store.list()[0]?.id === snap2.id);

  const removed = store.prune({ maxCount: 1 });
  check('prune(maxCount=1) removes oldest', removed === 1 && store.list().length === 1 && store.get(snap.id) === null);
}

{
  // Partial capture: one section read fails → recorded, not fatal.
  const store = new SnapshotStore(freshStorage());
  const flaky: SnapshotSource = {
    ...mockSource,
    macroRegion: async () => {
      throw new Error('macro read timeout');
    },
  };
  const snap = await store.capture(flaky, 'partial');
  check('failed section recorded in errors', snap.errors?.macro?.includes('timeout') === true);
  check('other sections still captured', snap.sections.keymap !== undefined && snap.sections.profile !== undefined);
  check('partial flag on list()', store.list()[0]?.partial === true);
}

// ══════════════════════════════════════════════════════════════════════
// transaction.ts
// ══════════════════════════════════════════════════════════════════════

section('transaction — success / stop-on-failure / approval / dry-run');

{
  const ran: string[] = [];
  const ops: PlannedOp[] = [
    { type: 'read', label: 'r1', apply: async () => void ran.push('r1') },
    { type: 'keymap_write', section: 'keymap', label: 'k1', apply: async () => void ran.push('k1') },
    { type: 'light_write', section: 'lighting', label: 'l1', apply: async () => void ran.push('l1') },
  ];
  const tx = await runTransaction(ops, { approve: approveAll });
  check('all ops ran in order', ran.join(',') === 'r1,k1,l1');
  check('transaction ok + completed', tx.ok && tx.completed);
  check('counts all success', tx.counts.success === 3 && tx.counts.failed === 0);
}

{
  const ran: string[] = [];
  const ops: PlannedOp[] = [
    { type: 'keymap_write', section: 'keymap', label: 'k1', apply: async () => void ran.push('k1') },
    {
      type: 'macro_write',
      section: 'macro',
      label: 'm1',
      apply: async () => {
        ran.push('m1');
        throw new Error('device did not confirm');
      },
    },
    { type: 'light_write', section: 'lighting', label: 'l1', apply: async () => void ran.push('l1') },
  ];
  const tx = await runTransaction(ops, { approve: approveAll });
  check('stops on first failure — later op never ran', ran.join(',') === 'k1,m1');
  check('reports partial honestly', !tx.ok && !tx.completed);
  check('journal shows success then failed',
    tx.journal[0]?.result === 'success' && tx.journal[1]?.result === 'failed' && tx.journal[1]?.error?.includes('confirm'));
  check('counts: 1 success 1 failed', tx.counts.success === 1 && tx.counts.failed === 1);
}

{
  // L2 without approve callback → denied, stops.
  const ran: string[] = [];
  const ops: PlannedOp[] = [
    { type: 'keymap_write', apply: async () => void ran.push('x') },
    { type: 'read', apply: async () => void ran.push('y') },
  ];
  const tx = await runTransaction(ops);
  check('L2 op denied without approve callback', !tx.ok && ran.length === 0);
  check('denial journaled as failed', tx.journal[0]?.result === 'failed' && tx.journal[0]?.error?.includes('denied'));
}

{
  const ran: string[] = [];
  const ops: PlannedOp[] = [
    { type: 'calibration', apply: async () => void ran.push('c') },
    { type: 'read', apply: async () => void ran.push('r') },
  ];
  const tx = await runTransaction(ops, { approve: () => false });
  check('declined L3 approval stops transaction', ran.length === 0 && !tx.completed);
}

{
  const tx = await runTransaction(
    [{ type: 'forbidden', apply: async () => ({ verified: true }) }],
    { approve: approveAll },
  );
  check('L4 forbidden refused even with approval', !tx.ok && tx.journal[0]?.result === 'failed' && tx.journal[0]?.error?.includes('policy'));
}

{
  const ran: string[] = [];
  const tx = await runTransaction(
    [
      { type: 'adv_write', apply: async () => ({ verified: false }) },
      { type: 'read', apply: async () => void ran.push('after') },
    ],
    { approve: approveAll },
  );
  check('unverified does not stop the transaction', tx.journal[0]?.result === 'unverified' && ran.includes('after'));
  check('unverified counted separately, tx still ok', tx.ok && tx.counts.unverified === 1);
}

{
  // Desired-state applies (config apply) set stopOnUnverified: an
  // unconfirmed write must halt the apply and ok must be false — an
  // unverified state is not a success.
  const ran: string[] = [];
  const tx = await runTransaction(
    [
      { type: 'keymap_write', apply: async () => ({ verified: false }) },
      { type: 'macro_write', apply: async () => void ran.push('after') },
    ],
    { approve: approveAll, stopOnUnverified: true },
  );
  check('stopOnUnverified halts after unverified write', ran.length === 0 && !tx.completed);
  check('stopOnUnverified reports not-ok', !tx.ok);
  check('journal records the unverified write', tx.journal[0]?.result === 'unverified' && tx.counts.unverified === 1);
}

{
  let applied = 0;
  const ops: PlannedOp[] = [
    { type: 'keymap_write', apply: async () => void applied++ },
    { type: 'forbidden', apply: async () => void applied++ },
  ];
  const plan = dryRun(ops);
  check('dryRun never calls apply', applied === 0);
  check('dryRun reports classes + approvals', plan.ops[0]?.class === 'L2' && plan.requiresApproval === 1);
  check('dryRun reports blocked ops', plan.blocked === 1);
}

{
  // Audit integration.
  const store = freshStorage();
  const audit = new AuditLog(store);
  await runTransaction(
    [{ type: 'macro_write', section: 'macro', apply: async () => ({ verified: true }) }],
    { approve: approveAll, audit },
  );
  const entries = audit.list();
  check('audit log recorded the op', entries.length === 1 && entries[0]?.op === 'macro_write' && entries[0]?.class === 'L2' && entries[0]?.result === 'success');
}

// ══════════════════════════════════════════════════════════════════════
// rollback.ts
// ══════════════════════════════════════════════════════════════════════

section('rollback — restore snapshot, per-section results');

{
  const storage = freshStorage();
  const store = new SnapshotStore(storage);
  const snap = await store.capture(mockSource, 'pre-change');
  const { writer, calls } = mockWriter();

  const res = await restoreSnapshot(store, snap.id, { writer, approve: approveAll });
  check('restore reports all sections success', res.sections.every((s) => s.result === 'success'),
    JSON.stringify(res.sections));
  check('tx completed + ok', res.tx.ok && res.tx.completed);

  const names = calls.map((c) => c.name);
  check('keymap writes per arg (8 ops)', names.filter((n) => n === 'writeKeymap').length === 8);
  check('lighting wrote lightData + colours', names.includes('writeLightData') && names.includes('writeCustomLightColors'));
  check('macro + profile written', names.includes('writeMacroRegion') && names.includes('writeProfile'));

  // arg packing: arg = layer | system<<2 → writeKeymap(layer, system)
  const km0 = calls.find((c) => c.name === 'writeKeymap' && c.args[2] !== undefined);
  const arg7 = calls.filter((c) => c.name === 'writeKeymap')[7];
  check('arg 7 splits to layer 3 system 1', arg7 !== undefined && arg7.args[0] === 3 && arg7.args[1] === 1);
  check('keymap values round-tripped', JSON.stringify(arg7?.args[2]) === JSON.stringify([0x1007, 0x5aa5]));
  void km0;
}

{
  // Partial failure: macro write fails → profile (later) never runs.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-change');
  const { writer, calls } = mockWriter(['writeMacroRegion']);

  const res = await restoreSnapshot(store, snap.id, { writer, approve: approveAll });
  const bySection = Object.fromEntries(res.sections.map((s) => [s.section, s.result]));
  check('macro section failed', bySection.macro === 'failed');
  check('keymap+lighting succeeded before failure', bySection.keymap === 'success' && bySection.lighting === 'success');
  check('profile skipped — not reached', bySection.profile === 'skipped');
  check('tx reports incomplete', !res.tx.ok && !res.tx.completed);
  check('writeProfile never called', !calls.some((c) => c.name === 'writeProfile'));
}

{
  // Section subset + approval gate.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-change');
  const { writer, calls } = mockWriter();
  let approvals = 0;
  const res = await restoreSnapshot(store, snap.id, {
    writer,
    sections: ['macro'],
    approve: () => {
      approvals++;
      return true;
    },
  });
  check('subset restore only writes macro', calls.length === 1 && calls[0]?.name === 'writeMacroRegion');
  check('approval asked once for the gated op', approvals === 1);
  check('subset result has one section', res.sections.length === 1 && res.sections[0]?.section === 'macro');
}

{
  // Missing writer method → honest failure, not a crash.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-change');
  const res = await restoreSnapshot(store, snap.id, {
    writer: {},
    sections: ['profile'],
    approve: approveAll,
  });
  check('missing write path fails honestly', res.sections[0]?.result === 'failed' && res.tx.journal[0]?.error?.includes('no profile write path'));
}

{
  // planRestore → dryRun preview without writes.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-change');
  const { writer, calls } = mockWriter();
  const ops = planRestore(snap, writer);
  const plan = dryRun(ops);
  check('planRestore covers all sections', plan.ops.length === 12, `got ${plan.ops.length}`);
  check('dryRun preview made no calls', calls.length === 0);
}

{
  // restoreLast picks the newest snapshot.
  const store = new SnapshotStore(freshStorage());
  const first = await store.capture(mockSource, 'old');
  const second = await store.capture(mockSource, 'new');
  const { writer, calls } = mockWriter();
  const res = await restoreLast(store, { writer, sections: ['profile'], approve: approveAll });
  check('restoreLast used the newest snapshot', res?.snapshotId === second.id && res.snapshotId !== first.id);
  check('empty store → null', (await restoreLast(new SnapshotStore(freshStorage()), { writer, approve: approveAll })) === null);
}

// ══════════════════════════════════════════════════════════════════════
// rollback.ts — rollbackSections (SAFETY-SPEC §5 R2 partial-failure)
// ══════════════════════════════════════════════════════════════════════

section('rollbackSections — best-effort rollback of touched sections');

{
  // Happy path: every touched section writes back.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const { writer, calls } = mockWriter();
  const rb = await rollbackSections(snap, writer, ['keymap', 'lighting', 'macro', 'profile'], {
    approve: approveAll,
  });
  check('all four sections rolled back', rb.rolledBack === 4 && rb.failed === 0 && rb.unavailable.length === 0);
  check('rollback tx completed + ok', rb.tx.ok && rb.tx.completed);
  check('rollback wrote every section',
    calls.some((c) => c.name === 'writeKeymap') &&
      calls.some((c) => c.name === 'writeLightData') &&
      calls.some((c) => c.name === 'writeCustomLightColors') &&
      calls.some((c) => c.name === 'writeMacroRegion') &&
      calls.some((c) => c.name === 'writeProfile'));
}

{
  // Partial rollback: macro write-back fails → profile (later) never runs.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const { writer, calls } = mockWriter(['writeMacroRegion']);
  const rb = await rollbackSections(snap, writer, ['keymap', 'lighting', 'macro', 'profile'], {
    approve: approveAll,
  });
  const bySection = Object.fromEntries(rb.sections.map((s) => [s.section, s.result]));
  check('keymap+lighting rolled back before the failure', bySection.keymap === 'success' && bySection.lighting === 'success');
  check('macro rollback failed, profile skipped', bySection.macro === 'failed' && bySection.profile === 'skipped');
  check('counts honest: 2 rolled back, 1 failed', rb.rolledBack === 2 && rb.failed === 1);
  check('rollback tx incomplete', !rb.tx.ok && !rb.tx.completed);
  check('writeProfile never attempted during rollback', !calls.some((c) => c.name === 'writeProfile'));
}

{
  // Section absent from a partial snapshot → listed unavailable, others still roll back.
  const store = new SnapshotStore(freshStorage());
  const flaky: SnapshotSource = {
    ...mockSource,
    macroRegion: async () => {
      throw new Error('macro read timeout');
    },
  };
  const snap = await store.capture(flaky, 'partial pre-import');
  const { writer, calls } = mockWriter();
  const rb = await rollbackSections(snap, writer, ['macro', 'keymap'], { approve: approveAll });
  check('missing section listed as unavailable',
    rb.unavailable.length === 1 && rb.unavailable[0]?.section === 'macro');
  check('present section still rolled back', rb.rolledBack === 1 && rb.sections[0]?.section === 'keymap');
  check('no macro write attempted', !calls.some((c) => c.name === 'writeMacroRegion'));
  check('requested keeps canonical order regardless of arg order',
    rb.requested.join(',') === 'keymap,macro');
}

{
  // Dedupe + canonical ordering: profile requested twice and out of order.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const { writer, calls } = mockWriter();
  const rb = await rollbackSections(snap, writer, ['profile', 'keymap', 'profile'], {
    approve: approveAll,
  });
  check('requested deduped + ordered', rb.requested.join(',') === 'keymap,profile');
  const firstWrite = calls[0]?.name;
  check('keymap writes ran before profile write', firstWrite === 'writeKeymap' && calls.at(-1)?.name === 'writeProfile');
}

{
  // Empty request → no device traffic.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const { writer, calls } = mockWriter();
  const rb = await rollbackSections(snap, writer, [], { approve: approveAll });
  check('empty rollback makes no calls', calls.length === 0 && rb.rolledBack === 0);
}

{
  // Unverified write-back counts as rolled back but stays honest in the tx.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const calls: string[] = [];
  const writer: RestoreWriter = {
    writeProfile: async () => {
      calls.push('writeProfile');
      return { verified: false };
    },
  };
  const rb = await rollbackSections(snap, writer, ['profile'], { approve: approveAll });
  check('unverified write-back counts as rolled back', rb.rolledBack === 1 && rb.sections[0]?.result === 'unverified');
  check('unverified still ok at tx level', rb.tx.ok);
}

{
  // End-to-end R2 scenario: a multi-section apply fails mid-way; only the
  // sections that actually landed are rolled back (caller-side rule:
  // journal success/unverified → snapSection). Mirrors applyParsedImport.
  const store = new SnapshotStore(freshStorage());
  const snap = await store.capture(mockSource, 'pre-import');
  const { writer: applyWriter } = mockWriter(['writeCustomLightColors']);
  const touched = new Set<'keymap' | 'lighting' | 'macro'>();
  // Simulated import: keymap ok, colours fail, macro never reached.
  const tx = await runTransaction(
    [
      { type: 'keymap_write', section: 'Key assignments', apply: async () => applyWriter.writeKeymap!(0, 0, [1]) },
      { type: 'light_write', section: 'Per-key colours', apply: async () => applyWriter.writeCustomLightColors!([]) },
      { type: 'macro_write', section: 'Macros', apply: async () => applyWriter.writeMacroRegion!(new Uint8Array(4)) },
    ],
    { approve: approveAll },
  );
  check('simulated apply stopped on failure', !tx.ok && !tx.completed);
  tx.journal.forEach((e, i) => {
    if (e.result === 'success' || e.result === 'unverified') {
      touched.add((['keymap', 'lighting', 'macro'] as const)[i]!);
    }
  });
  check('only the landed section is marked touched', touched.size === 1 && touched.has('keymap'));
  const { writer: rbWriter, calls: rbCalls } = mockWriter();
  const rb = await rollbackSections(snap, rbWriter, [...touched], { approve: approveAll });
  check('rollback restores only keymap', rb.rolledBack === 1 && rbCalls.every((c) => c.name === 'writeKeymap'));
  check('no macro/profile writes during rollback', !rbCalls.some((c) => c.name === 'writeMacroRegion' || c.name === 'writeProfile'));
}

// ── Done ───────────────────────────────────────────────────────────────

console.log(failures === 0 ? '\nAll safety tests passed.' : `\n${failures} test(s) FAILED.`);
process.exit(failures === 0 ? 0 : 1);
