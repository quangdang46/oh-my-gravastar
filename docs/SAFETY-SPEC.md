# Safety Spec

Normative spec for the 5-layer safety architecture (`docs/vision-full-custom.md`
§4). Applies to CLI (`crates/omg`) and web (`web/`). Current enforcement lives
in `docs/plan.md` §2 + `AGENTS.md` "Safety contract"; this doc extends it into
the transaction/policy model the UI and agent will share. **One engine**: UI,
CLI and agent all funnel writes through the same transaction executor —
agents never call HID writes directly.

## 1. Layer 1 — Input validation

Before any plan is built, every op argument passes schema validation:

- **Types**: strict JSON-schema types; reject unknown fields (closed schema).
- **Ranges** (existing, must be preserved):
  - actuation ≤ switch max travel (3.5 mm UFO / 3.4 mm otherwise);
    deadzone ≤ 1 mm — `crates/proto-he`, `crates/omg`.
  - palette indices 0–7.
  - khub916 key values: u32 BE, must decode via `decodeKeyValue` or be an
    explicit raw value flagged `raw: true` in Expert mode.
  - macro steps: `delay:keycode[:up]` grammar; keycodes checked against
    `keycode_blacklisted` (`crates/proto-he/src/lib.rs:350`); region ≤ 512 B.
  - color: `#RRGGBB` or `{r,g,b}` 0–255 ints; no dup checks needed for
    per-key slots but palette indices must be unique per palette.
  - profile JSON import: model/protocol compat, u32 bounds on numeric
    keymap values, array size limits, structural validation **before**
    write plan — this is risk R1 (§5) and bead `y6s`.
- **Sizes**: profile file ≤ 1 MiB; lighting JSON bounded by schema;
  macro ≤ region capacity.
- **Enums**: lighting mode ∈ `LIGHT_MODES` ids; layer ∈ device layer count
  from capability registry (khub916 = 4).
- **Desired-state config** (`omg config`, `docs/device-config-schema.md`):
  layer-1 structural validation is Draft 2020-12 against the single
  shared artifact `schemas/device-config.schema.json` — consumed by the
  Rust CLI (`crates/omg/src/config.rs`, `jsonschema` crate, embedded via
  `include_str!`) and the Web app (`web/src/device-config.ts`, AJV
  2020-12, same file imported — never copied). All ranges above apply
  inside that artifact; anything the schema cannot express statically
  (device-reported slot counts, unknown-tag base reads, firmware gates)
  remains a layer-2/3 check in the apply engine.

## 2. Layer 2 — Capability & compatibility

Every op is checked against the **capability registry**
(`docs/CAPABILITY-MATRIX.md` §3, bead `zlb`) for the *connected* device:

- `supported:false` → op rejected with the registry `reason` surfaced to UI
  ("unsupported-hardware: hall-effect only") and to the agent (planning
  error, not a write attempt).
- `verified:false` → allowed, but preview must carry the caveat
  (e.g. advkey cmd-18 "sent but persistence unconfirmed").
- Profile import additionally requires **model compatibility**: protocol +
  VID/PID + usage page match (existing behavior in `omg profile import`,
  `crates/omg/src/profile.rs`); whole-profile import for khub916/SparkPlayJoy
  is export-only until a mapping exists.
- Firmware/state checks: device must be in a writable state (not mid-stream,
  not already connected read-only); khub916 music/cmd-8 stream must be
  stopped and lighting restored before other light writes.

## 3. Layer 3 — Policy engine (L0–L4)

| Level | Class | Examples | Policy |
|---|---|---|---|
| L0 | read-only | info, keys get, light get, power, probe, travel read | automatic, no approval |
| L1 | draft/preview | staged keymap edits, lighting JSON edit, dry-run plan, diff view | automatic; nothing sent |
| L2 | standard write | keymap set, light mode/colors, macro set/bind, sleep, profile fields, mouse settings | approval required; preview shows exact change + affected keys + recoverability |
| L3 | disruptive write | polling set (device restarts, drops off bus), calibration (changes hardware behavior) | separate, stronger confirmation + explicit impact explanation; result may report `verified:false` without being an error |
| L4 | forbidden | factory reset, firmware update, settings wipe, bootloader, pairing, RF channel | **never** — blacklist (§6); agent must not even plan them |

Policy is a pure function `classify(op) -> L0..L4` in
`web/src/safety/policy.ts` (bead `sid`), keyed on op type — not on caller.
The CLI enforces the same classes via `--yes` (L2/L3) and omission (L4);
`--dry-run` covers L1.

`omg config apply` inherits this unchanged: `--dry-run`/no flag is L1
(validate + plan only, no bytes sent). `--yes` is L2 per effective write
AND is bound to the previewed plan: the dry run prints a `plan_hash`
(sha256 over the canonical document + selected device identity +
section plan) and apply refuses without `--approve <hash>` matching it —
the approval names exactly what was previewed, and the hash is recorded
in the apply journal.
A desired-state document can never express an L3 op — disruptive fields
(e.g. HE `polling_hz`, which restarts the keyboard) are either absent from
the schema or reported `not_applied` until their lowering implements the
L3 confirmation path explicitly. L4 has no representation in the config
envelope at all. Each `--yes` apply captures read-only snapshots of every
section it will write and persists `<id>.snapshots.json` BEFORE the first
write byte — if the snapshot cannot be written, the apply aborts. The
journal `<id>.journal.json` is rewritten after every section, apply stops
at the first hard failure AND the first `applied_unverified` write, and
`omg config rollback <id> --yes` re-verifies the connected device's
protocol + VID + PID + usage page against the recorded identity before
restoring — the same backup→write→verify→journal shape as §4, scoped per
section rather than per plan step. The legacy `kb916 macro
set|delete|bind|unbind` writes persist the same snapshot format before
their first write, so they are rollback-able through `omg config
rollback` as well. On the Web, the Custom config apply refuses to run
unless every snapshot section the planned ops need was captured without
errors (`web/src/custom-config.ts`).

## 4. Layer 4 — Transaction executor

Every L2/L3 write is one transaction:

```
backup → preview → approve → write → read-back verify → journal
```

1. **Backup**: snapshot affected sections (keymap layers, lighting state,
   macro region, profile fields) *before* first write byte. Web: localStorage
   history (`web/src/safety/snapshots.ts`); CLI already snapshots profile
   imports to `~/.local/share/oh-my-gravastar/backups/<utc-ts>/`.
2. **Preview**: structured diff (not raw bytes) — e.g. "Layer0: Caps→Ctrl;
   Layer1: WASD→arrows; Fn→layer2". Produced by planner for agent ops, by
   the page itself for UI ops.
3. **Approve**: L2 = one confirm (styled modal, bead `sfa`); L3 = second,
   separate confirm naming the impact ("keyboard will restart").
4. **Write**: ordered ops; on first failure **stop** and report partial
   (see R4).
5. **Read-back verify**: mandatory; `verified:true|false` in result.
   `kb polling set` is the documented exception — restart drops the device,
   report unverified, not error.
6. **Journal**: append entry {timestamp, device, ops, preview, result,
   verified} to `web/src/safety/audit-log.ts`; also powers "recent changes"
   on the Dashboard.

Recovery types (distinct — undo ≠ reset):

- **undo**: reverse the last transaction from snapshot.
- **redo**: re-apply a discarded preview.
- **discard draft**: drop staged edits, nothing was written.
- **restore point**: roll back to a named snapshot (bead `th0`).
- **factory defaults**: only via proven-safe path — for khub916 that means
  **write back the old values captured in backup**, never zero-fill
  (`docs/vision-full-custom.md` §4 recovery note).

## 5. Layer 5 — Recovery & audit + known risks (§4.2)

Each known risk gets a concrete fix requirement:

| # | Risk | Fix requirement |
|---|---|---|
| R1 | Profile import validation gaps | `validation.ts` checks: model/protocol compat before plan; u32 bounds on keymap values; color-index integer + uniqueness; array size caps; reject file >1 MiB. Bead `y6s`. |
| R2 | Multi-section restore partial failure | journal every section result; on failure stop, report which sections applied, offer restore-point rollback for the rest. |
| R3 | Macro region full-512 B rewrite | snapshot current region + compute/apply diff before send; never blind-overwrite (region is one blob — `docs/protocol-khub916.md` §6). |
| R4 | Lighting multi-write continues after failure | abort remaining writes on first error; report exactly which slots/modes landed (partial state is real state). |
| R5 | Adv-key cmd-18 history unverified on K1 Pro | capability flag `verified:false` + reason in registry; UI shows "sent, not confirmed" — already partially done in `web/src/advkeys.ts:252`; never label "stable". |
| R6 | Auto-reconnect UX | silent reconnect must show *which* device reconnected; easy disconnect; **no writes queued across reconnect** — any in-flight op fails safe, user re-approves. |

## 6. Blacklist (restatement — non-negotiable)

Never implemented, even later (`docs/plan.md` §2, enforced by omission in
code — `crates/proto-he/src/lib.rs:21`, `crates/proto-mouse/src/lib.rs:13,65`,
`crates/proto-khub916/src/lib.rs:50`, `crates/proto-khub916/src/wireless.rs:26`):

- **HE keyboard**: `02 01` factory reset, `08 xx` firmware update,
  `0A 01 01 FF` wipe, `10 02` gamepad mode, `02 0F 01` restart,
  `02 09` default switch, categories `0x0C/0x0E/0x0F`.
- **Compx mouse**: `05/06` pairing, `09` factory reset, `31` RF channel,
  `B0–B7` bootloader; writes only inside `WRITABLE` flash ranges.
- **khub916**: nothing until proven read-only (`docs/protocol-khub916.md`);
  wireless cmd 6 = factory reset — not implemented.
- Keycodes `0xF200`/`0xF203` rejected in HE macro/key paths
  (`keycode_blacklisted`).
- Agent policy: L4 ops are unplannable — the planner must reject them at
  validation, not rely on executor refusal.

## 7. No-secrets rule

No credentials/tokens/serial numbers in logs, JSON, journal, or agent
context unless `--include-serial` is explicitly passed (reserved flag;
do not emit serials meanwhile — `docs/plan.md` §2.6).
