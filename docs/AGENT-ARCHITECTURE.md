# Agent Architecture

> **Normative contract:** `docs/agent-api.md` documents the shipped
> schema v1 (op names, arg shapes, lifecycle, policy). This file is the
> design rationale; where they disagree, `agent-api.md` wins.

Spec for the agent-first layer (`docs/vision-full-custom.md` §"Agent-first
architecture"). Target layout per vision §9:

```
web/src/agent/       device-context.ts  action-schema.ts  planner.ts
                     validator.ts       executor.ts
web/src/core/        capabilities.ts    feature-registry.ts  device-model.ts  compatibility.ts
web/src/safety/      validation.ts      policy.ts  snapshots.ts  transaction.ts  rollback.ts  audit-log.ts
```

Beads: `zlb` (capabilities), `sid` (safety), `ak2` (schema+dry-run),
`7js` (NL assistant, P4). CLI stays agent-drivable today via `--json`
(`README.md`); agent ops must reuse the same op vocabulary as the CLI so
one schema serves both.

## 1. Pipeline

device understanding → plan generation → validation & simulation →
preview & approval → execute → verify & recover

```
user/agent request
  → device-context.ts     assemble DeviceContext (below)
  → planner.ts            produce Plan { ops[], diff, impact, rollback }
  → validator.ts          schema + capability + policy + safety checks
  → executor.ts           run through safety/transaction.ts ONLY
  → verify                read-back per op; journal entry per op
  → recover               on failure: stop, report partial, offer rollback
```

Hard rules (vision "Strategic rules"):

1. Agents **never** call HID writes directly — executor goes through the
   same transaction engine as the UI (SAFETY-SPEC §4).
2. Planner may only emit ops whose capability is `supported:true`;
   `verified:false` ops carry a warning into the preview.
3. L4 ops are unplannable — validator rejects, executor never sees them.
4. Every plan is previewable (dry-run) before approval; approval is per
   plan, L3 ops get a second gate.

## 2. Device context assembly (`device-context.ts`)

Assembled per session, refreshed on (re)connect:

```ts
interface DeviceContext {
  device: { model: string; protocol: 'khub916'|'he-v2'|'compx'|'sparkplayjoy'|'khub916-wireless';
            vid: number; pid: number; verification: VerificationLevel };
  firmware?: { version: string; build?: string };
  state: { battery?: number; link?: string; lightingMode?: number;
           macroUsageBytes?: number; /* …read snapshot… */ };
  capabilities: CapabilityMap;          // from core/capabilities.ts
  layout?: KeyLayout;                    // physical truth (k1pro-layout.ts)
  activeProfile?: string;
}
```

Sources: `omg devices --json` / `kb916 info` equivalent in web
(`web/src/hid.ts` `Khub916Session`), `omg probe` for diagnostics,
capability registry for the rest. **No serials/tokens** in context
(SAFETY-SPEC §7).

## 3. Action op schema (`action-schema.ts`)

Closed JSON schema; one op = one atomic device mutation. Versioned
(`schema_version: 1`).

```json
{
  "schema_version": 1,
  "plan_id": "uuid",
  "device_selector": "auto | vid:pid | path",
  "ops": [
    {
      "op": "set_keymap",
      "layer": 0, "system": 0,
      "entries": [ {"index": 40, "value": 224},
                   {"index": 4, "value": 5} ],
      "id": "op-1"
    },
    {
      "op": "set_light",
      "config": { "mode": 3, "speed": 4, "brightness": 80 }
    },
    {
      "op": "set_macro",
      "name": "hello", "bind_key": 58,
      "steps": [ {"delay_ms": 0, "key": "h", "up": false},
                 {"delay_ms": 50, "key": "h", "up": true} ]
    },
    {
      "op": "set_adv",
      "mode": "mt", "key": "caps_lock",
      "tap": "0x70000000", "hold": "0xe0",
      "hold_ms": 200
    },
    {
      "op": "restore_snapshot",
      "id": "snap-2026-11-24T…",
      "sections": ["keymap", "lighting"]
    },
    {
      "op": "set_profile_section",
      "section": "sleep_seconds", "value": 300
    }
  ],
  "meta": { "requested_by": "user|agent:<name>", "rationale": "…" }
}
```

Shipped op vocabulary (schema v1 — see `agent-api.md` §1 for exact arg
shapes; `action-schema.ts` is the source of truth):

| op | khub916 | HE v2 | Compx | notes |
|---|---|---|---|---|
| `set_keymap` | cmd 3/4 write | `03 02/04` | — | per-layer, sparse slot patches |
| `set_light` | cmd 10/134 + profile fields | cat 5 zones/palette | `set light` | config = `validateLightConfig` shape |
| `set_macro` | cmd 5 region + tag-3 bind | cat 7 paged | mouse macros | ≤512 B region for 916 |
| `set_adv` | cmd 18 (**verified:false**) | cat 6 `hk` | — | mt/socd/tgl; carries caveat |
| `set_profile_section` | cmd 132/4 profile blob | profile cmds | config flash | `sleep_seconds` only in v1 |
| `restore_snapshot` | composed of above | composed | composed | expands to section ops |

**Not expressible in schema v1:** `set_polling`, `set_os`,
`set_performance`, `calibrate` (L3 class, HE-only) — adding them needs a
new op kind plus the stronger-confirmation flow. See `agent-api.md` §8.

Planner output = `{ ops[], diff, warnings[], rollback_hint }`. The `diff`
is human/agent-readable ("Layer0: Caps→Ctrl") — same structure the UI
preview modal renders (UIUX-SPEC §6).

## 4. Component responsibilities

- **`planner.ts`**: NL or structured request → op list. Rule-based first
  (bead `7js`): intent table ("caps as ctrl" → set_keymap, "dim lights" →
  set_light), slot-filling from DeviceContext (layer count, key names,
  mode ids). Never invents ops; unknown intent → clarification question,
  not a guess.
- **`validator.ts`**: ordered checks — (1) JSON schema, (2) per-op arg
  validation (`safety/validation.ts`), (3) capability gate per op,
  (4) policy class per op (`safety/policy.ts` L0–L4), (5) cross-op
  conflicts (two ops touching same key/region → reject or order),
  (6) L4 blacklist → hard reject. Output: `valid | errors[] | warnings[]`.
- **`executor.ts`**: executes validated plans through
  `safety/transaction.ts`; per-op verify; stops on first failure;
  emits journal entries; returns `{applied[], failed?, verified}`.
  Idempotent where protocol allows (read-before-write compare).
- **`device-context.ts`**: assembly + refresh; owns "which device" —
  ambiguity → ask, never guess (multi-device safety).

## 5. "Explain my keyboard" (feature spec)

User-facing capability dump — answers "what can THIS device do?":

1. Render `DeviceContext.capabilities` grouped: Works / Works-but-unconfirmed
   / Not on this hardware / Unknown.
2. Every entry cites evidence level (live-verified / code-tested / …) —
   never theory. E.g. K1 Pro shows "Per-key RGB ✅", hides "Rapid Trigger"
   entirely or shows it under "Not on this hardware (mechanical switch)".
3. Plain-language mode: one line per feature + why; Expert mode adds the
   protocol command IDs and verification notes (from `docs/protocol-*.md`).
4. Also lists *what changed recently* (audit log tail) and *what's
   unconfirmed* (advkey persistence, wireless, screen).
5. Bead: part of `7js`; capability data from `zlb`.

## 6. Tools & interoperability

| Surface | Status | Notes |
|---|---|---|
| CLI `omg … --json` | [shipped] | every data command; agents drive this today |
| `omg probe [--deep]` | [shipped] | read-only diagnostic summary |
| Profile JSON schema v1 | [shipped] | `omg profile export/import` (`crates/omg/src/profile.rs`) |
| Desired-state config | [partial] | `omg config schema/validate/apply/rollback` + web Custom config page; shared Draft 2020-12 artifact (`schemas/device-config.schema.json`); separate from agent ops — see `docs/agent-api.md` §8 |
| Web agent module (`web/src/agent/*`) | [missing] | beads `ak2`, `7js` |
| Dry-run API | [partial] | `profile import --dry-run` exists; general plan dry-run = `ak2` |
| MCP/tool manifest | [missing] | P2+: expose op schema as MCP tools; each tool = one op type, routed through executor |
| Batch ops | [missing] | plan = ordered op list covers this |
| Event stream (state changes) | [missing] | web `state.ts` subscribe() is in-process only |
| Docs for agents | [partial] | README + `--json` contract; needs `docs/agent-api.md` once `ak2` lands |

## 7. Key scenarios → op mapping (acceptance targets)

| Scenario | Ops | Class |
|---|---|---|
| "Caps Lock → Ctrl" | set_keymap {caps_lock: left_ctrl} | L2 |
| "WASD-only gaming layer" | set_keymap layer1 + bind | L2 |
| "Navigation layer on Fn" | set_keymap (Fn→layer2 if firmware supports) | L2 + capability check |
| "FPS red lighting" | set_lighting scene | L2 |
| "Coding layout preset" | set_keymap multi + optional layout note | L2 |
| "Factory restore" | restore_snapshot of pre-change baseline; **never** L4 reset | L2 via restore |
| "RT tuning" (HE only) | set_performance | L3, HE-gated |
| "Unused-key analysis" | read-only scan → suggestions | L0/L1 |
| "Device diagnostics" | probe + explain | L0 |
