/**
 * Smoke test for web/src/config-apply.ts — the desired-state → PlannedOp
 * lowering behind the Custom config page's guided apply. Uses a mock
 * ConfigSession (no HID). Verifies the capability gate mirrors the Rust
 * `plan_sections`, semantic conflict checks fire, and lowered ops produce
 * the right writeKeymap/writeMacroRegion/writeProfile calls.
 * Run: `npm run smoke` (chained) — plain Node TS runner, no test deps.
 */

declare const process: { exit(code?: number): never };

import {
  planConfigSections,
  configSemanticErrors,
  configToOps,
  configPlanHash,
  missingSnapshotSections,
  type ConfigSession,
} from '../src/config-apply.ts';
import { macroKeyValue } from '../src/proto/macro916.ts';
import semanticFixtures from '../../schemas/fixtures/semantic-cases.json' with { type: 'json' };
import planFixtures from '../../schemas/fixtures/plan-cases.json' with { type: 'json' };

const KHUB = { protocol: 'khub916', vid: 0x1e7d, pid: 0xf501 };

function doc(sections: Record<string, unknown>): Record<string, unknown> {
  return { schema_version: 1, device: KHUB, ...sections };
}

let failures = 0;
function check(name: string, cond: boolean, extra = ''): void {
  if (cond) {
    console.log(`  ok  ${name}`);
  } else {
    failures++;
    console.error(`  FAIL ${name} ${extra}`);
  }
}

// ── planConfigSections mirrors crates/omg/src/config.rs plan_sections ──

{
  const p = planConfigSections(doc({ keymap: { mode: 'patch' } }));
  check('khub916 keymap patch planned', p[0]!.status === 'planned');
}
{
  const p = planConfigSections(doc({ lighting: {}, advanced_keys: {} }));
  check(
    'khub916 lighting/advanced_keys not_applied',
    p.every((s) => s.status === 'not_applied') && /not yet wired/.test(p[0]!.detail),
  );
}
{
  const p = planConfigSections({
    schema_version: 1,
    device: { protocol: 'compx', vid: 1, pid: 2 },
    settings: { mode: 'patch' },
  });
  check(
    'compx patch rejected',
    p[0]!.status === 'not_applied' && /read-merge/.test(p[0]!.detail),
  );
}
{
  let gated = true;
  for (const protocol of ['he-v2', 'sparkplayjoy']) {
    const p = planConfigSections({
      schema_version: 1,
      device: { protocol, vid: 1, pid: 2 },
      settings: {},
    });
    if (p[0]!.status !== 'not_applied') gated = false;
  }
  check('he-v2 and sparkplayjoy gated', gated);
}

// ── configSemanticErrors ──

{
  const errs = configSemanticErrors(
    doc({
      keymap: {
        mode: 'patch', layer: 0, system: 0,
        entries: [
          { index: 3, value: '0x1' },
          { index: 3, value: '0x2' },
        ],
      },
    }),
  );
  check('duplicate patch index flagged', errs.some((e) => e.includes('duplicate index 3')));
}
{
  const errs = configSemanticErrors(
    doc({
      keymap: { mode: 'patch', layer: 0, system: 0, entries: [{ index: 5, value: '0x1' }] },
      macros: [{
        index: 'next', steps: [{ action: 'down', key: 'KeyA' }],
        bind: { layer: 0, system: 0, key: 5, macro_type: 1, loop_count: 1 },
      }],
    }),
  );
  check('keymap vs macro bind slot conflict flagged', errs.some((e) => e.includes('slot conflict')));
}
{
  const errs = configSemanticErrors(
    doc({
      keymap: { mode: 'patch', layer: 0, system: 0, entries: [{ index: 5, value: '0x1' }] },
      macros: [{
        index: 'next', steps: [{ action: 'down', key: 'KeyA' }],
        bind: { layer: 0, system: 0, key: 6, macro_type: 1, loop_count: 1 },
      }],
    }),
  );
  check('non-conflicting bind is clean', errs.length === 0);
}

// ── configToOps + mock session ──

interface MockSession extends ConfigSession {
  calls: {
    writeKeymap: unknown[][];
    writeMacroRegion: number;
    writeProfile: number;
  };
}

function mockSession(over: Partial<ConfigSession> = {}): MockSession {
  const calls = { writeKeymap: [] as unknown[][], writeMacroRegion: 0, writeProfile: 0 };
  return {
    calls,
    keyValues: async () => [0x01000000, 0x02000000, 0x03000000],
    writeKeymap: async (l: number, s: number, v: readonly number[]) => {
      calls.writeKeymap.push([l, s, ...v]);
      return { verified: true };
    },
    macroRegion: async () => new Uint8Array(512),
    writeMacroRegion: async () => {
      calls.writeMacroRegion++;
      return { verified: true };
    },
    profile: async () => ({ raw: new Uint8Array(128) }),
    writeProfile: async () => {
      calls.writeProfile++;
      return { verified: true };
    },
    ...over,
  };
}

{
  const s = mockSession();
  const ops = configToOps(
    doc({
      keymap: {
        mode: 'patch', layer: 1, system: 0,
        entries: [{ index: 1, value: '0xA5' }],
      },
    }),
    s,
  );
  check('keymap patch → one keymap_write op', ops.length === 1 && ops[0]!.type === 'keymap_write');
  await ops[0]!.apply();
  const call = s.calls.writeKeymap[0]!;
  check(
    'patch edits only the targeted slot',
    call[0] === 1 && call[1] === 0 && call[2] === 0x01000000 && call[3] === 0xa5,
    JSON.stringify(call),
  );
}

{
  const s = mockSession();
  const ops = configToOps(
    doc({ keymap: { mode: 'replace', layer: 0, system: 0, values: ['0x1', '0x2'] } }),
    s,
  );
  let threw = '';
  try {
    await ops[0]!.apply();
  } catch (e) {
    threw = e instanceof Error ? e.message : String(e);
  }
  check(
    'keymap replace length mismatch throws before write',
    /reports 3 slots/.test(threw) && s.calls.writeKeymap.length === 0,
    threw,
  );
}

{
  const s = mockSession();
  const ops = configToOps(
    doc({
      macros: [{
        index: 'next',
        name: 'hi',
        steps: [{ action: 'down', delay_ms: 10, key: 'KeyA' }],
        bind: { layer: 0, system: 0, key: 2, macro_type: 1, loop_count: 1 },
      }],
    }),
    s,
  );
  check('macros → one macro_write op', ops.length === 1 && ops[0]!.type === 'macro_write');
  await ops[0]!.apply();
  const call = s.calls.writeKeymap[0];
  check(
    'macro bind writes tag-3 value at key slot',
    s.calls.writeMacroRegion === 1 &&
      s.calls.writeKeymap.length === 1 &&
      call !== undefined &&
      call[4] === macroKeyValue(1, 1, 0),
    JSON.stringify(call),
  );
}

{
  const s = mockSession();
  const ops = configToOps(doc({ settings: { sleep_seconds: 42 } }), s);
  check(
    'settings.sleep_seconds → one settings_write op',
    ops.length === 1 && ops[0]!.type === 'settings_write',
  );
  await ops[0]!.apply();
  check('sleep writes profile blob', s.calls.writeProfile === 1);
}

{
  const ops = configToOps(
    {
      schema_version: 1,
      device: { protocol: 'compx', vid: 1, pid: 2 },
      settings: { mode: 'replace' },
    },
    mockSession(),
  );
  check('non-khub916 doc produces no ops', ops.length === 0);
}

// ── Safety regressions (review P0/P1) ──

{
  const errs = configSemanticErrors(
    doc({
      macros: [
        { index: 2, steps: [{ action: 'down', key: 'KeyA' }] },
        { index: 2, steps: [{ action: 'down', key: 'KeyB' }] },
      ],
    }),
  );
  check('duplicate macro index flagged', errs.some((e) => e.includes('duplicate index 2')));
}

{
  // A bind that fails preflight must abort BEFORE the region write.
  const s = mockSession();
  const ops = configToOps(
    doc({
      macros: [{
        index: 'next',
        steps: [{ action: 'down', key: 'KeyA' }],
        bind: { layer: 0, system: 0, key: 99, macro_type: 1, loop_count: 1 },
      }],
    }),
    s,
  );
  let threw = '';
  try {
    await ops[0]!.apply();
  } catch (e) {
    threw = e instanceof Error ? e.message : String(e);
  }
  check(
    'bad bind preflight aborts before region write',
    /out of range/.test(threw) &&
      s.calls.writeMacroRegion === 0 &&
      s.calls.writeKeymap.length === 0,
    `${threw} writes=${s.calls.writeMacroRegion}`,
  );
}

{
  // Duplicate resolved slots ("next" colliding with an explicit index of
  // the append position) must be rejected before any write.
  const s = mockSession();
  const ops = configToOps(
    doc({
      macros: [
        { index: 'next', steps: [{ action: 'down', key: 'KeyA' }] },
        { index: 0, steps: [{ action: 'down', key: 'KeyB' }] },
      ],
    }),
    s,
  );
  // Empty region → "next" resolves to 0, same as explicit index 0.
  let threw = '';
  try {
    await ops[0]!.apply();
  } catch (e) {
    threw = e instanceof Error ? e.message : String(e);
  }
  check(
    'duplicate resolved slot rejected before write',
    /duplicate resolved index/.test(threw) && s.calls.writeMacroRegion === 0,
    threw,
  );
}

{
  // Unverified region write must not be followed by bind writes.
  const s = mockSession({
    writeMacroRegion: async () => {
      s.calls.writeMacroRegion++;
      return { verified: false };
    },
  });
  const ops = configToOps(
    doc({
      macros: [{
        index: 'next',
        steps: [{ action: 'down', key: 'KeyA' }],
        bind: { layer: 0, system: 0, key: 1, macro_type: 1, loop_count: 1 },
      }],
    }),
    s,
  );
  const out = (await ops[0]!.apply()) as { verified?: boolean } | undefined;
  check(
    'unverified region write skips binds',
    out?.verified === false && s.calls.writeKeymap.length === 0,
  );
}

// ── missingSnapshotSections — the pre-write restore-point gate ──

{
  const ops = configToOps(
    doc({
      keymap: { mode: 'patch', layer: 0, system: 0, entries: [{ index: 1, value: 'KeyA' }] },
      macros: [{ index: 0, steps: [{ action: 'press', key: 'KeyA' }] }],
      settings: { sleep_seconds: 30 },
    }),
    mockSession(),
  );
  const full = {
    sections: { keymap: { '0': [1] }, macro: 'bb', profile: 'cc' },
  };
  check('complete snapshot passes gate', missingSnapshotSections(ops, full).length === 0);
  const missingMacro = {
    sections: { keymap: { '0': [1] }, profile: 'cc' },
    errors: { macro: 'timeout' },
  };
  const missing = missingSnapshotSections(ops, missingMacro);
  check(
    'missing+errored macro section blocks apply',
    missing.includes('macro') && !missing.includes('keymap'),
    missing.join(','),
  );
  const emptySnap = { sections: {} };
  check(
    'empty snapshot blocks all needed sections',
    missingSnapshotSections(ops, emptySnap).sort().join(',') === 'keymap,macro,profile',
  );
}

// ── shared semantic fixtures (schemas/fixtures/semantic-cases.json) ──
// Same cases the Rust `shared_semantic_fixtures` test runs — both
// implementations must agree on every doc.

{
  const fixture = semanticFixtures as unknown as {
    cases: {
      name: string;
      doc: Record<string, unknown>;
      expect_errors: string[];
      expect_ok: boolean;
    }[];
  };
  for (const c of fixture.cases) {
    const errs = configSemanticErrors(c.doc);
    const joined = errs.join('\n');
    if (c.expect_ok) {
      check(`semantic fixture ok: ${c.name}`, errs.length === 0, joined);
    } else {
      const missing = c.expect_errors.filter((w) => !joined.includes(w));
      check(`semantic fixture: ${c.name}`, missing.length === 0, `missing ${missing}`);
    }
  }
}

// ── configPlanHash — approval bound to the previewed plan ──

{
  const d = doc({ settings: { sleep_seconds: 30 } });
  const plans = planConfigSections(d);
  const dev = { vendorId: 0x1e7d, productId: 0xf501, usagePage: 0xff };
  const h1 = await configPlanHash(d, plans, dev);
  check('plan hash is sha256 hex', /^[0-9a-f]{64}$/.test(h1));
  check(
    'plan hash deterministic',
    (await configPlanHash(d, plans, dev)) === h1,
  );
  const d2 = doc({ settings: { sleep_seconds: 60 } });
  check(
    'doc change → different hash',
    (await configPlanHash(d2, planConfigSections(d2), dev)) !== h1,
  );
  check(
    'device change → different hash',
    (await configPlanHash(d, plans, { ...dev, productId: 0xf502 })) !== h1,
  );
  // Key order in the source JSON must not matter (canonical form).
  const reordered = JSON.parse(
    JSON.stringify({ device: KHUB, schema_version: 1, settings: { sleep_seconds: 30 } }),
  ) as Record<string, unknown>;
  check(
    'key order irrelevant',
    (await configPlanHash(reordered, plans, dev)) === h1,
  );
}

// ── shared plan fixtures (schemas/fixtures/plan-cases.json) ──
// khub916 capability gate pinned identically to Rust plan_sections.

{
  const fixture = planFixtures as unknown as {
    cases: {
      name: string;
      doc: Record<string, unknown>;
      expect: Record<string, 'planned' | 'not_applied'>;
    }[];
  };
  for (const c of fixture.cases) {
    const plans = planConfigSections(c.doc);
    const byName = new Map(plans.map((p) => [p.name, p.status]));
    for (const [section, want] of Object.entries(c.expect)) {
      check(
        `plan fixture ${c.name}: ${section}=${want}`,
        byName.get(section) === want,
        `got ${byName.get(section)}`,
      );
    }
  }
}

if (failures) {
  console.error(`smoke-config-apply: ${failures} failure(s)`);
  process.exit(1);
}
console.log('smoke-config-apply: all checks passed');
