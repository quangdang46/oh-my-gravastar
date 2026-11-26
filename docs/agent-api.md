# Agent API — Web agent contract (`web/src/agent/*`)

> **Scope note:** this document is the **Web application's** agent
> contract only — the TypeScript op vocabulary in
> `web/src/agent/action-schema.ts` (`schema_version: 1`). It is
> intentionally **not** shared with the Rust CLI.
>
> The Rust CLI's Agent-facing contract is a separate, schema-driven
> desired-state configuration surface (`omg config schema|validate|
> apply|rollback`, `omg cli-schema`) specified in
> [device-config-schema.md](device-config-schema.md) with the shared
> artifact at `schemas/device-config.schema.json`.
> The two contracts have different op vocabularies and must not be
> conflated; they agree only on externally observable device semantics
> and safety policy.

Normative reference for `web/src/agent/*` (beads `ak2`, `agent-validator`,
`oe4` agent-docs). This is the contract an external caller — NL assistant,
MCP tool manifest, or test harness — uses to build, validate and execute
plans against a connected GravaStar device.

**Status:** library-only. `web/src/agent/index.ts` exports the module; no
page or CLI command is wired to it yet. Everything below is implemented
in TypeScript; nothing here is aspirational except where marked.

**Source of truth:** the op vocabulary is *generated from*
`web/src/agent/action-schema.ts` (schema_version 1). This document is kept
in sync manually — when the schema changes, update this file in the same
commit. Other normative refs: `docs/AGENT-ARCHITECTURE.md` (pipeline),
`docs/SAFETY-SPEC.md` (policy classes, blacklist),
`docs/protocol-khub916.md` (wire), `docs/lighting-schema.md`,
`docs/macro-schema.md`, `docs/advkeys-schema.md`.

---

## 1. Op vocabulary (schema_version 1)

A **plan request** is either a bare ops array or an envelope:

```json
{
  "schema_version": 1,
  "device_selector": "auto",
  "ops": [ { "op": "…", "device_selector": "…" } ],
  "meta": { "requested_by": "user|agent:<name>", "rationale": "…" }
}
```

- `schema_version` must be `1` when present; anything else is an error.
- `ops` must be a non-empty array; **cap 64 ops per plan**.
- `meta` is carried for audit/journal only; it grants no privileges.

### Common fields (every op)

| field | type | required | constraint |
|---|---|---|---|
| `op` | string | yes | one of `AGENT_OP_KINDS` (closed set, below) |
| `device_selector` | string | yes | non-empty; `"auto"` \| `"vid:pid"` \| device path. Resolution is the caller's job — the schema only requires the string, never guesses. Per-op selectors are the multi-device intent: the planner echoes the unique set as `AgentPlan.device_selectors`, and each maps 1:1 onto the `device` field of an `omg plan` step (docs/plan-json.md). Mixed selectors across ops → plan warning; execution stays single-session. `DeviceContext.device.selector` (`"0xVID:0xPID"`) is the ready-made value when the HID ids are known. |
| `id` | string | no | caller-assigned label echoed into diffs/journal |

### `set_keymap` — sparse keymap patch (khub916)

One atomic op = one layer/system table read-modify-write. The executor
reads the current u32 table, applies the sparse `entries`, writes the
whole table back.

| field | type | constraint |
|---|---|---|
| `layer` | int | **0–3** (required) |
| `system` | int | 0–1, default 0 (arg = `layer \| system<<2`) |
| `entries` | array | non-empty, ≤128 entries; each `{index, value}` |
| `entries[].index` | int | 0–127 (`KEYMAP_ENTRIES` = 128-slot u32 table), unique within the op |
| `entries[].value` | u32 | integer `0..0xffffffff` **or** `"0x…"` hex string; decoded by `decodeKeyValue` (normal `0x00` tag = HID usage + modifier bits, `0x03` macro, `0x0d` Fn-layer, `0x20/21/22` adv keys) |

### `set_light` — lighting config (khub916)

`config` is validated by `safety/validation.ts::validateLightConfig`
(`docs/lighting-schema.md`). At least one of `mode`, `speed`,
`brightness`, `color_model`, `color`, `keys` must be present.

| field | type | constraint |
|---|---|---|
| `mode` | int | 0–65535 (effect id; see lighting-schema mode table) |
| `speed` | int | 0–20 (vendor limit) |
| `brightness` | int | 0–20 |
| `color_model` | int | 1 = monochrome, 2 = mixing |
| `color` | color | `#rrggbb` / `rrggbb` / `r,g,b` / `{r,g,b}` — group colour for the active mode |
| `keys.color` | color | same forms — group colour alias |
| `keys.colors` | array | **exactly 126** colours — full custom table (cmd-6 write) |
| `keys.entries[]` | array | sparse `{index\|pos: 0–125, color}` — read-modify-write of the 126-slot table |
| `keys.preset` | — | **rejected** — no device write mapping yet |
| `keys.to` | — | **rejected** — gradient target has no wire mapping |

The executor lowers one `set_light` op into up to three write ops
(profile fields → light-data group colour → custom colour table), each
read-back verified. Fields the wire cannot express are rejected at
validation, never silently dropped.

### `set_macro` — create/replace/delete a macro + optional bind

Steps follow `docs/macro-schema.md`. `steps` may be a bare array or
`{name?, steps[]}`; `up: true|false` is accepted as sugar for
`action:"up"|"down"`, and a bare single-letter `key` is normalised to
`KeyX` before validation.

| field | type | constraint |
|---|---|---|
| `name` | string | ≤255 bytes on the wire; required for a *new* macro unless `index` is given |
| `index` | int | 0–2048 (`4 × MAX_MACRO_STEPS`) — alternative to `name` for targeting |
| `steps` | array/object | macro steps (≤512 steps, `delay_ms` ≤ 1048575); mutually exclusive with `delete` |
| `delete` | bool | `true` removes the macro; mutually exclusive with `steps`; `bind_key` forbidden with it |
| `bind_key` | int | 0–127 keymap slot to bind on `bind_layer`; requires the macro write to succeed first |
| `bind_layer` | int | 0–3, default 0 |
| `macro_type` | int | 0–255, default 1 (vendor macro-type byte) |
| `loop` | int | 0–255, default 1 (vendor loop count) |

Region capacity (≤512-byte macro region) is enforced at context
validation via `macroSpaceNeeded` and `state.macroFreeBytes`, with
credit for the replaced macro's current body.

### `set_adv` — advanced-key bind (SOCD / MT / TGL)

Lowers to the vendor two-write path (`writeKeyConfiguration` +
`writeAdvKeyConfig`, `web/src/proto/advkey916.ts`).

| field | type | constraint |
|---|---|---|
| `mode` | `"mt"\|"socd"\|"tgl"` | required |
| `key`, `key_a`, `key_b`, `toggle` | int \| string | keymap index 0–127 **or** layout key name (e.g. `"Caps"`, `"L_Ctrl"`) resolved by the planner against `DeviceContext.layout`; required per mode (below) |
| `tap`, `hold` | u32 | MT only — integer or `0x…` hex |
| `hold_ms` | int | MT only, 0–655350 (vendor stores 10 ms units × 65535); default 200 ms |
| `direction` | int | SOCD only, 0–3 (`socdList` id); default 0 |

Mode requirements: `mt` needs `key`+`tap`+`hold`; `socd` needs
`key_a`+`key_b`; `tgl` needs `key`+`toggle`. Unresolvable layout names
land in plan `errors` — the executor refuses string refs that were never
resolved to an index.

### `restore_snapshot` — restore a stored snapshot

| field | type | constraint |
|---|---|---|
| `id` | string | non-empty snapshot id (`snap-…` from `SnapshotStore`). **Note:** this field *is* the snapshot id — it shadows the common caller-`id`, so `restore_snapshot` cannot carry a separate caller label. |
| `sections` | array | optional subset of `keymap\|lighting\|macro\|profile`; default = every section present in the snapshot |

Snapshot existence and section coverage are checked against
`DeviceContext.snapshots`; the executor expands the op through
`safety/rollback.ts::planRestore` in fixed order
keymap → lighting → macro → profile.

### `set_profile_section` — profile-blob field write

| field | type | constraint |
|---|---|---|
| `section` | `"sleep_seconds"` | the only section with a proven write mapping (`profileWithSleepSeconds`) |
| `value` | int | 0–255 |

### Op → policy → section → execution order

`planner.ts::OP_META` assigns each kind a section and a stable order;
`restore_snapshot` always sorts last so a restore can never be
overwritten by an earlier write it was meant to undo.

| op kind | policy `OpType` | class | section | order |
|---|---|---|---|---|
| `set_keymap` | `keymap_write` | L2 | keymap | 0 |
| `set_light` | `light_write` | L2 | lighting | 1 |
| `set_macro` | `macro_write` | L2 | macro | 2 |
| `set_adv` | `adv_write` | L2 | advkeys | 3 |
| `set_profile_section` | `settings_write` | L2 | profile | 4 |
| `restore_snapshot` | `profile_restore` | L2 | snapshot | 5 (forced last) |

---

## 2. Plan lifecycle

```
ops[] ──validatePlan/planFromOps──▶ AgentPlan ──validatePlanAgainstContext──▶
executePlan(dryRun?|approve?) ──▶ ExecuteResult { applied, verified, tx.journal }
```

1. **Schema validation** — `validatePlan(input)` (envelope or array) or
   `planFromOps(ops, context?)`. `validateOp`/`validatePlan` **never
   throw**; they return `{ok, errors}` plus the *normalised* op (u32
   numbers, macro `up:` sugar folded, layout names in `set_adv` resolved
   to indices when a `DeviceContext` was supplied). `planFromOps` produces
   the `AgentPlan`: per-op `policy` decision, one-line `diff`, stable
   section ordering, `valid`, `requiresApproval`, `warnings`, `errors`.
   A plan with `valid:false` must not run.
2. **Context validation** — `validatePlanAgainstContext(plan, context)`
   returns `PlanValidation {ok, errors, warnings}`. Ordered gates:
   (a) policy — class re-derived from the op *kind* (plan-supplied labels
   are display data, never trusted); (b) capability — per-op
   `CapabilityKey` lookups (`keymap`, `lighting_effects`,
   `per_key_rgb` for per-key light writes, `macro`, `adv_keys`);
   `unsupported-hardware`/`unsupported-firmware`/`protocol-unknown`/missing
   → error, `unverified`/`experimental` → warning; (c) cross-op conflicts;
   (d) storage limits (macro region bytes, snapshot existence).
3. **Execution** — `executePlan(plan, {session, approve?, dryRun?,
   context?, snapshots?, audit?})`. Gate order: schema → context →
   *lowering* (ops → `PlannedOp`s; pure, closures never invoked during
   lowering — lowering failures become plan errors) → `dryRun:true`
   returns `preview` (policy classification per write op) and never
   touches the device → real run calls `runTransaction`, the **only**
   path to device writes.
4. **Result/audit** — `ExecuteResult` carries `validation`, `preview`
   (dry-run policy map), `tx` (`TransactionResult`: per-op journal with
   `success|failed|unverified`, timestamps, policy class), `applied[]`
   (labels that did not fail), `failed?` (first failure), `verified`
   (true only when no journal entry is `failed`/`unverified`), and
   `errors[]`. `approve` is `true` (approve all), a per-op predicate, or
   omitted — omitted denies every L2/L3 op (safety-first default).
   Every journal entry is appended to the optional `AuditLog`.

The transaction executes ops **sequentially** and **stops on first
failure**; partial results are reported honestly (`completed:false`,
`ok:false`). `unverified` does not stop the run but is never hidden —
e.g. adv-key cmd-18 writes report "sent, read-back did not confirm".

---

## 3. Policy classes L0–L4

Classes come from `safety/policy.ts::classify(opType)` — a pure function
keyed on op *type*, not caller. `DECISIONS` in policy.ts is normative:

| class | meaning | agent ops |
|---|---|---|
| L0 | read-only query — automatic | none (reads are `DeviceContext`, not plan ops) |
| L1 | draft/preview — nothing sent | `executePlan({dryRun:true})` is the L1 path |
| L2 | standard write — approval required | **every v1 op kind** (all six are L2) |
| L3 | disruptive write — separate confirmation + impact explanation | none expressible in schema v1 (`polling`, `calibration` have no op kind) |
| L4 | forbidden — never plannable | impossible by construction; validator refuses defensively anyway |

Consequences:

- `plan.requiresApproval` is true for any non-empty valid plan.
- `approve` omitted → every write denied at the transaction gate; the
  plan fails with `approval denied` in the journal.
- L3 ops (`set_polling`, calibration) exist in the CLI/policy layer but
  **cannot be expressed** in agent schema v1 — adding them is a schema
  change, not a caller choice.
- L4 (factory reset, firmware update, wipe, bootloader, pairing, RF
  channel, khub916 cmd-17/reset paths) has no op kind and is rejected at
  both plan and transaction layers. See §8.

---

## 4. Device context (`buildDeviceContext`)

Assembled per session via
`buildDeviceContext(session, {device?, snapshots?})`
(`web/src/agent/device-context.ts`). Every read is individually guarded —
a failed read degrades that section to absent plus a `warnings[]` entry;
it never fails the whole context. No serials/tokens are ever placed in
the context (SAFETY-SPEC §7).

```ts
interface DeviceContext {
  device: {
    model: string | null;
    protocol: 'khub916' | 'khub916-wireless' | 'he-v2' | 'compx' | 'sparkplayjoy' | …;
    catalogModel: string | null;
    transport: 'wired' | 'wireless' | null;
    vid?: number; pid?: number;
    selector?: string;                        // "0xVID:0xPID" — usable as a plan device_selector
    verification: 'live' | 'code' | 'synthetic' | 'none'; // weakest cap level
  };
  firmware?: { version: string };              // e.g. "0x0100"
  state: {
    battery?: number;
    lightingMode?: number;
    sleepSeconds?: number | null;
    macroEntries?: number;
    macroFreeBytes?: number;                   // free space in 512-B region
  };
  capabilities: CapabilityMap;                 // core/capabilities.ts
  layout?: readonly KeyBlock[];                // K1 Pro physical layout only
  keymaps: {                                   // args 0–3 (layer|sys<<2, sys 0)
    arg: number;
    values: number[];                          // raw u32 table as read
    entries: { index: number; value: number; label: string }[]; // non-zero only
  }[];
  lighting?: { mode: number; modeName?: string; speed: number;
               brightness: number; colorModel: number; color?: Rgb };
  macros: { name: string; steps: number }[];
  snapshots?: SnapshotMeta[];                  // when a SnapshotStore was passed
  warnings: string[];
}
```

The planner uses `layout` to resolve `set_adv` key names and the validator
uses `capabilities`, `state.macroFreeBytes`, `macros`, and `snapshots`.
Always pass a context to `planFromOps`/`executePlan` — without it,
layout names cannot resolve and storage/snapshot checks are skipped.

---

## 5. Snapshot & restore semantics

- **Capture** — `SnapshotStore.capture(source, label)` (Web Storage,
  `omg.snap.*` prefix) reads every writable section: keymap args 0–7,
  lighting (light-data blob hex + 126 custom colours), macro region hex,
  profile blob hex. Ids look like `snap-<base36-ts>-<rand>`;
  `list()` returns `SnapshotMeta {id,label,ts,sections,partial}`.
  A failed section read is recorded in `snapshot.errors` and the snapshot
  is marked `partial` — it still restores what it captured.
- **Restore** — `restore_snapshot` validates the id and requested
  sections against `context.snapshots`, then `planRestore` emits one
  `PlannedOp` per section **in fixed order keymap → lighting → macro →
  profile**, each going through the session's verified write methods
  inside the transaction (stop-on-first-failure, per-section journal).
  `sections` omitted → every section present in the snapshot.
- **Auto-snapshot** — `snapshotBeforeWrite(store, source, label)` is the
  helper write paths call *before* mutating so a later restore can undo.
- Snapshots live in browser storage (or injected `KeyValueStorage`); they
  are device-state, not device identity — restoring a snapshot taken on
  a different model is the caller's responsibility.

---

## 6. Error & warning taxonomy

All validation is non-throwing; results are string lists. Three stages,
each feeding the next:

**Stage A — schema (`validatePlan` / `planFromOps`)**
`{ok:false, errors[]}` / `AgentPlan.errors`:

- `unsupported schema_version …` / `plan must be an object {ops:[…]} or an ops array`
- `plan ops must be a non-empty array`; `plan has N ops — cap is 64`
- `ops[i]: unknown op "…" — want one of …`; `ops[i]: device_selector must be a non-empty string …`
- per-op field errors, e.g. `set_keymap: entries must be a non-empty array`,
  `set_keymap entries[j]: value must be a u32 (integer or 0x… hex)`,
  `set_macro: delete and steps are mutually exclusive`,
  `set_adv socd: missing key_a`, `set_light config: …` (safety-layer
  messages), `set_light config: keys.preset has no device write mapping yet`
- planner resolution: `set_adv key 'X' does not match a key on the device layout`
- plan-level warnings: mixed `device_selector`s ("executor runs against ONE session")

**Stage B — context (`validatePlanAgainstContext`)**
`{ok, errors[], warnings[]}`. Error categories:

- **policy**: `op '…': refused by policy (L4) — …` (defensive; unreachable via schema v1)
- **capability**: `op '…': capability 'keymap' not reported for this device`;
  `op '…': 'adv_keys' is unsupported-hardware — <reason>`
- **conflicts**: `N set_keymap ops target layer/system L:S — merge them into one op`;
  `set_macro name 'x' is targeted by both …`; `set_macro index N is targeted by both …`
- **storage**: `set_macro 'x': needs ~N bytes but only ~M are free in the macro region`;
  `… over the 512-byte cap`
- **snapshot**: `restore_snapshot: no snapshot 'snap-…' in the store`;
  `restore_snapshot: snapshot '…' has no 'lighting' section`
- **unknown op kind** (forged plans): `op '…': unknown op kind — refused`

Warnings (non-fatal, ride into the preview):

- capability `unverified`/`experimental` — "the result may be unconfirmed"
- arg-0 slot collisions — "key N is written by both … — the later write wins"
- `restore_snapshot` mixed with writes — "restore runs last — earlier write ops will be overwritten"
- L3 ops — "needs a separate confirmation with impact explanation" (unreachable in v1)

**Stage C — execution (`executePlan` → `ExecuteResult`)**

- `errors[]` aggregates validation + lowering failures (`<diff>: <message>`,
  e.g. `no macro-region path on this session`, `no snapshot '…'`).
- `tx.journal[]` entries: `{op, section, label, class, started, finished,
  result: success|failed|unverified, error?}`.
- `applied[]` = labels of ops that did not fail; `failed?` = first failure;
  `verified` = no `failed`/`unverified` entries; `ok` = `tx.ok && tx.completed`.
- Denied approval journals `failed` with `approval denied — operation not permitted`.

---

## 7. Worked examples

All examples assume `device_selector` resolves to one connected khub916
(K1 Pro). Slot indices below use the K1 Pro layout
(`web/src/k1pro-layout.ts`: Caps → keymap index 3, L_Ctrl → index 5);
**always confirm indices against `context.keymaps` / `context.layout`** —
they are device/model-specific.

### 7.1 Remap Caps Lock → Left Ctrl (L2)

u32 keymap value for a bare Ctrl modifier = modifier bit `0x10000`
(usage 0 + ctrl flag, per `decodeKeyValue`).

```json
{
  "schema_version": 1,
  "device_selector": "auto",
  "ops": [
    {
      "op": "set_keymap",
      "device_selector": "auto",
      "layer": 0,
      "system": 0,
      "entries": [ { "index": 3, "value": "0x10000" } ],
      "id": "caps-to-ctrl"
    }
  ],
  "meta": { "requested_by": "agent:assistant", "rationale": "user asked caps as ctrl" }
}
```

Expected: `plan.valid=true`, `requiresApproval=true`, one diff line
`keymap layer 0 system 0: set slot 3`. Dry-run preview shows one
`keymap_write` L2 op; real run reads the layer-0 table, patches slot 3,
writes all 128 values, verifies by read-back.

### 7.2 Lighting scene apply (L2)

Breathing blue, dim — profile fields + group colour. (`keys.preset` /
`keys.to` are **not** usable here; use `keys.colors`/`keys.entries` for
per-key work, or ask the CLI-side preset generator to emit the resolved
126-colour table.)

```json
{
  "schema_version": 1,
  "ops": [
    {
      "op": "set_light",
      "device_selector": "auto",
      "config": {
        "mode": 2,
        "speed": 8,
        "brightness": 5,
        "color_model": 1,
        "color": "#2244ff"
      },
      "id": "fps-blue"
    }
  ]
}
```

This lowers to **two** write ops inside one plan position: profile write
(mode/speed/brightness/color_model) then light-data write (group RGB for
mode 2). Per-key variant adds `"keys": {"entries": [{"index": 0,
"color": "#00ff00"}]}` which triggers a third write (custom colour
table read-modify-write).

### 7.3 Macro create + bind (L2)

Type "hi" on `bind_key` slot 20 (layer 0), then bind it. `up:` sugar is
accepted; the planner normalises `"h"` → `"KeyH"`.

```json
{
  "schema_version": 1,
  "ops": [
    {
      "op": "set_macro",
      "device_selector": "auto",
      "name": "hi",
      "bind_key": 20,
      "bind_layer": 0,
      "steps": [
        { "key": "h", "up": false, "delay_ms": 0 },
        { "key": "h", "up": true,  "delay_ms": 30 },
        { "key": "i", "up": false, "delay_ms": 0 },
        { "key": "i", "up": true,  "delay_ms": 0 }
      ],
      "id": "macro-hi"
    }
  ]
}
```

Execution order inside the op: macro-region read → append/replace entry
→ region write → keymap read → patch `bind_key` with the
`macroKeyValue(macro_type, loop, index)` value → keymap write. If the
region write fails, the bind never runs (sequential transaction).

### 7.4 Snapshot restore (L2)

Restore only the keymap and lighting sections of a previously captured
snapshot:

```json
{
  "schema_version": 1,
  "ops": [
    {
      "op": "restore_snapshot",
      "device_selector": "auto",
      "id": "snap-mj3k9x2-abc123",
      "sections": ["keymap", "lighting"]
    }
  ]
}
```

Validator checks `snap-mj3k9x2-abc123` exists in `context.snapshots` and
contains both sections; `planRestore` emits keymap args 0–7 writes then
lighting writes, in that fixed order. Omit `sections` to restore every
captured section. Because `restore_snapshot` sorts last, it can be
combined with other ops — but the validator warns that earlier writes
will be overwritten.

---

## 8. Non-goals (explicit)

- **No raw HID.** Agents never see or send feature reports; every op
  lowers to session methods (`writeKeymap`, `writeMacroRegion`, …) that
  perform read-back verification. There is no "send bytes" op.
- **No factory reset / firmware / wipe / bootloader / pairing / RF
  channel.** L4 ops are unplannable — no op kind exists for them, and
  the transaction engine refuses `forbidden`-class ops even if one were
  forged into a plan.
- **khub916 cmd-17 blacklist stays.** Per `docs/protocol-khub916.md` §6,
  the vendor reset command and all `→` writes remain off-limits; nothing
  in the agent surface exposes them.
- **No L3 ops in schema v1.** `kb polling set` and calibration stay CLI/
  UI-only until the schema grows an op kind with the stronger-confirmation
  flow.
- **Desired-state config is not an agent op.** The `device-config`
  envelope (`schemas/device-config.schema.json`, `omg config`, and the
  web Custom config page) is a separate validated format with its own
  structural schema — it is intentionally not part of this op
  vocabulary, so no agent plan can smuggle a whole-document write past
  per-op policy classification. `web/src/config-apply.ts` lowers it to
  the same `PlannedOp`/`runTransaction` path the agent executor uses.
- **No cross-device execution.** Ops carry `device_selector` per op and
  the plan echoes the unique set as `device_selectors`, so multi-device
  *intent* is representable and maps 1:1 onto the `device` field of
  `omg plan` steps (docs/plan-json.md — the CLI executor DOES route
  per-step devices, including keyboard+mouse mixes). But
  `executePlan` runs against ONE `AgentSession` (a single khub916
  wired session — every v1 op kind lowers to khub916 writes; HE keyboard
  and Compx mouse ops do not exist in this vocabulary), so mixed
  selectors produce a warning, not fan-out. Until the vocabulary grows
  per-family ops and the engine grows multi-session execution, callers
  needing keyboard+mouse plans should emit `omg plan` JSON directly.
- **No invented ops.** The planner (when `7js` lands) must emit only
  vocabulary ops; unknown intent → clarification, never a guess.
- **No secrets.** Device context, plans, diffs, journals and audit
  entries never contain serials/tokens (`--include-serial` remains
  reserved and unimplemented).
