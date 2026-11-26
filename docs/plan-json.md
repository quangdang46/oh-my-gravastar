# Plan JSON — `omg plan` (legacy command-plan format)

> **Status: compatibility format.** This document describes the
> *command-oriented* plan (`{version:1, steps:[{op,args}]}`) where `op`
> is a leaf CLI command path. It remains supported for multi-step CLI
> automation, but it is **not** the Agent configuration contract.
>
> The Agent configuration contract is the schema-driven desired-state
> document (`omg config schema|validate|apply|rollback`) specified in
> [device-config-schema.md](device-config-schema.md) with the shared
> artifact at `schemas/device-config.schema.json`.

Machine-readable multi-step device plans. This is the interchange format
for agents (and later the web agent engine): describe desired changes as
JSON, get a plan with per-step risk + predicted writes, validate it
against the connected hardware, apply sequentially with per-step
read-back verify + journal, and roll back snapshots on demand.

Commands:

```
omg plan create   --input plan.json [--json]
omg plan inspect  <id> [--json]
omg plan validate <id> [--json]
omg plan apply    <id> --yes [--json] [--device SEL] [--dry-run]
omg plan rollback <id> --yes [--json] [--dry-run]
```

Safety contract (docs/plan.md §2, docs/SAFETY-SPEC.md):

- `create`/`inspect`/`validate` never touch a device (validate reads
  `find_devices()` + the capability registry only).
- `apply` and `rollback` require `--yes`; without it they bail before any
  bytes are sent. `--dry-run` prints the execution order and predicted
  writes and sends nothing.
- Steps execute **sequentially through the same code path as the CLI** —
  each step is re-parsed by `Cli::try_parse_from` and dispatched through
  `run()`, so every existing `--yes` gate, range check, blacklist and
  read-back verify applies unchanged. The plan-level `--yes` is injected
  as each write command's `--yes`.
- On the first failed step the plan **stops**; the journal records
  exactly which steps landed (`success` | `unverified` | `failed`).
- L4 (forbidden) ops are refused at validation — the planner never emits
  them.

## Plan document (schema v1)

```json
{
  "version": 1,
  "name": "optional human label",
  "device": "optional default --device selector for all steps",
  "steps": [
    { "op": "kb916 set key", "args": { "index": 5, "value": "0x1234" } },
    { "op": "kb916 light apply", "args": { "config": { "mode": 2, "speed": 8 } } },
    { "op": "kb keys set", "args": { "layer": 0, "row": 3, "col": 4, "code": "0x1E" } }
  ]
}
```

- `version` must be `1` (`schema` is accepted as an alias).
- `op` is a **leaf command path** from `omg cli-schema` (the
  Clap-derived command listing), space-separated,
  restricted to the device-driving families `kb`, `kb916`, `mouse`,
  `profile`. Introspection commands (`cli-schema`, `device`, `devices`,
  `probe`) and `plan` itself cannot be planned.
- `args` keys are clap **argument ids** (the `name` field of
  `omg cli-schema <cmd>`); the `--long` spelling (`interval-ms`) is also
  accepted. Values are strings, numbers, booleans, or arrays (arrays
  repeat the token for `Vec` args like `codes`/`actions`/`colors`).
  Unknown args and clap-level errors (enums, ranges, required args,
  conflicts) are rejected at `create` time — the same parser the CLI
  uses does the checking.
- Boolean flags accept only `true` (emit `--flag`); `false` is an error —
  omit the arg instead.
- A step may carry `"device": "<selector>"` overriding the plan/global
  `--device`.
- Max 64 steps.

### Multi-device plans

One plan may span device families — e.g. an HE keyboard (`kb …`) and a
Compx mouse (`mouse …`) side by side:

```json
{
  "version": 1,
  "steps": [
    { "op": "kb light set", "device": "0x1234:0x5678",
      "args": { "area": 0, "brightness": 50, "on": true } },
    { "op": "mouse set dpi", "device": "/dev/hidraw3",
      "args": { "stage": 0, "dpi": 800 } }
  ]
}
```

Selector precedence is **step `device` > plan `device` > `--device`**.
Each step is validated and executed against the device *its own*
selector resolves to: `validate`/`apply` check the required device kind
per step (a step whose selector matches nothing of its kind is
`blocked`, it does not fall back to another device), and `apply`
re-parses each step into an `omg` argv carrying that step's `--device`.
Snapshots and the journal are keyed per resolved device path, so
rollback restores each device independently and restore argvs are
pinned to the snapshot's own `--device` path — a mixed plan cannot
write to the wrong device.

Restrictions still apply per step: mouse writes honour the `WRITABLE`
flash-range allowlist and every write keeps its `--yes` gate (injected
from the plan-level `--yes`), range checks and read-back verify. L4 ops
remain refused regardless of which device they name.

### Inline file arguments

Three ops accept inline payloads instead of file paths:

- `kb916 set keymap`: `"values": ["0x…", …]` (u32 tokens) — written to a
  temp file at apply time.
- `kb916 light apply`: `"config": {…}` — the lighting-schema JSON object,
  written to a temp file at apply time.
- `kb916 macro set`: `"steps"` may be the JSON array/object directly (the
  CLI already accepts inline JSON).

## Risk levels (SAFETY-SPEC §3)

| Level | Meaning | Plan policy |
|---|---|---|
| L0 | read-only op | allowed; executed like any step |
| L2 | standard write | allowed; `apply` needs `--yes` |
| L3 | disruptive write | allowed with warning: `kb polling set` (keyboard restarts), `kb calibration start|stop`, `kb916 light stream` |
| L4 | forbidden | **refused** — `kb916 reset`, `kb916 screen push|clock`, `kb916 wireless`, and any op whose path contains a blacklist segment (`factory*`, `firmware`, `pairing`, `bootloader`, `rf-channel`, `wipe`, `reset`) |

## Validation

`create` runs schema-level checks (op resolves, args parse, risk class,
conflict detection) and stores the result in the plan file. `validate`
additionally checks, per step:

- the required device kind is connected (honouring `--device` /
  plan / step selectors) — otherwise `blocked`;
- the capability registry entry for the op is `supported` on that
  device — `supported:false` → error with the registry `reason`;
- `blacklisted:true` → refused.

Conflicts: two writes addressing the same target produce a `warning`
("the later write wins"); a `kb polling set` step warns that later
same-family steps may fail after the restart.

## Storage

`$OMG_PLAN_DIR` when set, else `~/.local/share/omg/plans/`:

- `<id>.json` — the stored plan + latest validation result + status.
- `<id>.journal.json` — apply journal: every step `{step, op, argv,
  status, verified, error?, at}` plus rollback results.
- `<id>.snapshots.json` — pre-write rollback data per device.
- `<id>.tmp/` — materialised inline files and restore payloads.

Plan ids look like `plan-20261124T183450Z-64d2`.

## Apply semantics

Steps run **in order**. Before the first write touching a section, a
snapshot is captured; the journal is flushed after every step, so a
crash or failure leaves an exact record of what landed. Statuses:

- `success` — exit 0 and read-back verified (or no verify field).
- `unverified` — the command reported `verified:false` (e.g. polling
  restart dropping off the bus) — reported, not an error.
- `failed` — error or non-zero exit; **the plan stops here**.

## Rollback — coverage and honest limits

`omg plan rollback <id> --yes` restores, per device, the sections the
plan touched, in reverse order. Coverage:

- **khub916 (wired)** — fine-grained, via the protocol's own read paths:
  - keymap (`set key`/`keymap`, macro `bind`/`unbind`/`set --key`) →
    whole keymap for that layer/system rewritten from the pre-plan read;
  - `set sleep` → profile byte restored via `set sleep`;
  - `set light-mode` → mode/speed/brightness/type restored via
    `set light-mode`;
  - `set light-color` → 126-slot colour table restored via `light apply`;
  - `light apply` → mode/speed/brightness/colour-model + custom colours
    + the *current* mode's monochrome group colour restored via
    `light apply`. **Limit:** per-mode group colours of *mixing* modes
    and non-current modes are not restorable through `light apply` —
    reported as a snapshot note;
  - `macro set <i>` → the previous entry rewritten via `macro set`
    (the `macro get` JSON is the same schema `set` accepts);
  - `macro set next` → the appended entry deleted via `macro delete`;
  - `macro delete`, `adv set *`, `light stream` → **no restore path**,
    listed as `skipped` with the reason. There is no factory-reset
    shortcut — rollback only ever writes back captured values.
- **HE keyboard / Compx mouse** — whole-profile restore: a
  `profile export`-shaped snapshot is taken before the first write and
  restored via `profile import` (which does its own backup + verify).
  This reverts *every readable setting* of that device to its pre-plan
  value — including changes made outside the plan after the snapshot.
  `kb keys set`/`set-row` are additionally restored per-key/per-row
  (profile import has no keymap write). `kb hk set` (advanced keys) has
  no restore path → `skipped`.
- Restore argvs are pinned to the snapshot's device path
  (`--device <path>`), so multi-device plans cannot write to the wrong
  device. `profile import` steps inside a plan are not snapshot-
  restored (the import path keeps its own backups under
  `~/.local/share/oh-my-gravastar/backups/`).

Every restore is itself a verified write through the normal command
path; results land in the journal under `rollback.results` with
`success|unverified|failed|skipped`.

## Relationship to `omg config` (desired-state)

`plan` and `config` are complementary, not interchangeable:

- **`config`** (`omg config apply`, `docs/device-config-schema.md`) is a
  *desired-state* document: it describes the target configuration of one
  device and lets the engine diff it against live state. Use it for
  "make this device look like X".
- **`plan`** is an *ordered action* document: an explicit sequence of
  operations with per-step verify/journal/rollback, spanning multiple
  devices and mixing reads with writes. Use it for workflows
  ("probe, then set these three things, then verify").

`config` is deliberately **not** a plan family today: a plan step resolves
its device up front (`required_kind` → one `DeviceKind`), while a config
document selects its device from its own `device` block across three
protocol families. `config apply` therefore keeps its own journal +
snapshot store (`<id>.journal.json` / `<id>.snapshots.json` in the same
directory) and its own restore path (`omg config rollback <id> --yes`).
Wiring `config apply` in as a plan step is a future change that must also
teach `step_sections`/`khub916_needs` that plan-level rollback cannot see
inside the envelope — until then a plan must not claim coverage for it.
Conversely, action-verbs (factory reset, firmware update, pairing,
screen push, live streaming, calibration) are never config fields —
they stay plan/CLI operations only.
