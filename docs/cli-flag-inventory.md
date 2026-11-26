# `omg` CLI flag inventory

Exhaustive inventory of every CLI flag/arg across every `omg` subcommand, built
to support the planned migration from scattered per-command flags to a unified
desired-state JSON config (`omg config validate`/`omg config apply` —
implemented; see `docs/device-config-schema.md` for the shipped surface).

Scope: every `#[arg(...)]`, positional, and flattened-`Yes`/`HkSet`/`PerfWrite`
field across all `enum *Commands` blocks in `crates/omg/src/main.rs`, plus the
subcommand enums in `crates/omg/src/kb916_macro.rs`, `crates/omg/src/kb916_adv.rs`
and `crates/omg/src/plan.rs`. Compiled by a read-only source sweep against
commit `e0539d0` (2026-11-26); re-verify against current `main.rs` line numbers
before citing a specific line, since the file moves fast (5882 lines as of this
sweep).

## Disposition legend

- **KEEP** — invocation concern (device selection, output mode, confirmation,
  dry-run, action verb, stream pacing/stop-condition). Stays a flag regardless
  of config migration.
- **MIGRATE** — a value that should become a JSON field in a desired-state
  doc; commands not yet covered by `omg config apply` remain the primary
  path, and are deprecated only after parity + tests confirm the JSON path
  reproduces it exactly (per AGENTS.md: never delete a flag before its
  replacement is verified).
- **CONTEXT** — a pure scope selector (`--layer`, `--system`, `row`/`col`,
  `area`, `index`/`id`, `stage`). Keep as a scoping key *inside* the JSON doc
  (e.g. `keymap.layers[1].slots[4]`), not as a standalone CLI flag.
- **REVIEW** — ambiguous or doesn't fit the KEEP/MIGRATE/CONTEXT split cleanly.
  Mostly the live-streaming `kb916 light stream`/`light music` session
  parameters, which drive an ongoing process rather than persisted device
  state.

## 0. Global flags

| Flag | Type | What it does | Disposition |
|---|---|---|---|
| `--json` | bool | Machine-readable output | KEEP |
| `--device` | `Option<String>` (HID path / `VID:PID[:USAGE_PAGE]` / substring) | Target device selector | KEEP |
| `--dry-run` | bool | Preview a write without sending (generic interception via `schema::is_write_path`/`has_native_dry_run`) | KEEP |
| `--yes` (flattened `Yes`, every write command) | bool | Confirms a write; without it the command bails before device open | KEEP |

## 1. `omg devices` / `omg probe` / `omg device *`

| Command | Flag | Type | What it does | Disposition |
|---|---|---|---|---|
| `probe` | `--deep` | bool | Includes heavier SparkPlayJoy travel-matrix read | KEEP |
| `device inspect`/`capabilities`/`status`/`list` | (none beyond global) | — | — | — |

## 2. `omg profile *`

| Command | Flag | Type | What it does | Disposition |
|---|---|---|---|---|
| `profile export` | `file` (positional) | String | Output JSON path | KEEP |
| `profile export` | `--full` | bool | Include large raw blobs (khub916 per-mode light data, macro hex) | KEEP |
| `profile import` | `file` (positional) | String | Input profile JSON path | KEEP |
| `profile import` | `--yes` | bool | Confirms writes | KEEP |

**Note:** `profile export`/`import` is already a whole-device JSON
snapshot/restore and is one of three existing precedents for the target
`omg config` architecture (see §Precedents below).

## 3. `omg mouse *`

`mouse info|battery|fw|config` — no flags beyond global.

### `mouse set <field>` (`MouseSet`)

| Flag | Type/constraints | What it does | Disposition |
|---|---|---|---|
| `mouse set polling <hz>` | u32, one of 125/250/500/1000/2000/4000/8000, ≤ link max | Polling rate | MIGRATE |
| `mouse set dpi <stage>` | usize 0..7 | Target DPI stage (scope) | CONTEXT |
| `mouse set dpi <stage> <dpi>` | u32 ≥50, clamped to sensor max | DPI value | MIGRATE |
| `mouse set dpi-color <stage>` | usize 0..7 | Target stage (scope) | CONTEXT |
| `mouse set dpi-color <stage> <color>` | `#rrggbb`/`r,g,b` | Stage LED colour | MIGRATE |
| `mouse set stages <n>` | usize 1..8 | Active DPI-stage count | MIGRATE |
| `mouse set stage <i>` | usize 0..7 | Active DPI stage selector | MIGRATE |
| `mouse set lod <v>` | u8, sensor's valid LOD table | Lift-off distance | MIGRATE |
| `mouse set debounce <ms>` | u8 0..15 | Debounce time | MIGRATE |
| `mouse set motion-sync <on>` | `"on"\|"off"` | Motion Sync flag | MIGRATE |
| `mouse set angle-snap <on>` | `"on"\|"off"` | Angle snapping | MIGRATE |
| `mouse set ripple <on>` | `"on"\|"off"` | Ripple control | MIGRATE |
| `mouse set light-off-moving <on>` | `"on"\|"off"` | Lights-off-while-moving | MIGRATE |
| `mouse set sleep <seconds>` | u16, one of 10/30/60/120/180/300/600/900 | Sleep timeout | MIGRATE |
| `mouse set sensor-mode <mode>` | `"lp"\|"hp"` | Sensor power mode | MIGRATE |
| `mouse set button <slot>` | usize 0..`BUTTON_SLOTS` | Button slot (scope) | CONTEXT |
| `mouse set button <slot> <kind>` | `disabled\|mouse\|dpi\|shortcut\|macro` | Button assignment kind | MIGRATE |
| `mouse set button <slot> <kind> <param>` | `Option<String>` | Button kind parameter | MIGRATE |

### `mouse set light <what>` (`MouseLight`)

| Flag | Type | What it does | Disposition |
|---|---|---|---|
| `mouse set light on`/`off` | — | Light on/off | MIGRATE |
| `mouse set light mode <mode>` | u8 0..=6 | Light effect mode | MIGRATE |
| `mouse set light color <color>` | `#rrggbb`/`r,g,b` | Light colour | MIGRATE |
| `mouse set light speed <v>` | u8 1..=9 | Effect speed | MIGRATE |
| `mouse set light brightness <v>` | u8 1..=9 | Brightness | MIGRATE |

## 4. `omg kb *` (HE v2 keyboards)

`kb info|layout|lighting-areas|rt-precision` — no flags beyond global.
`kb live <row>` — row is CONTEXT.
`kb travel-matrix --watch` — KEEP (stream display mode, not state).

### `kb keys <sub>` (`KbKeys`)

| Flag | Type/constraints | Disposition |
|---|---|---|
| `--layer` | u8, default 0 | CONTEXT |
| `--row` / `row` | `Option<u8>` / u8 | CONTEXT |
| `--col` / `col` | `Option<u8>` 0..20, requires row | CONTEXT |
| `set` / `set-row` `code`/`codes` | decimal/hex string (blacklist `0xF200`/`0xF203`) / `Vec<String>` exactly 21 | MIGRATE |
| `default <system> <func> <row>` | u8 nibbles 0..15 / u8 | CONTEXT |

### `kb hk <sub>` (`KbHk`/`HkSet`)

| Flag | Type/constraints | What it does | Disposition |
|---|---|---|---|
| `row`, `col` | u8, col 0..20 | Target key (scope) | CONTEXT |
| `--mode` (get) | enum `none\|dks\|mpt\|mt\|tgl\|end\|socd\|rs` | Bank to query (scope) | CONTEXT |
| `--mode` (set) | same enum | Advanced-key mode to program | MIGRATE |
| `--kcs` | CSV keycodes, count depends on mode | Keycodes for the mode | MIGRATE |
| `--trps` | CSV u8×4 (dks only) | DKS trigger points | MIGRATE |
| `--dbs` | CSV f32 (dks=2, mpt=3) | Deadband distances | MIGRATE |
| `--time` | `Option<u16>` (mt/tgl) | Raw hold/toggle time | MIGRATE |
| `--delay` | `Option<u16>` (end/socd/rs) | Raw delay | MIGRATE |
| `--row2`/`--col2` | `Option<u8>` (socd/rs) | Partner key | MIGRATE |
| `--direction` | enum `cover\|priority-a\|priority-b\|neutral` (socd) | SOCD resolution policy | MIGRATE |

### `kb macro <sub>` (`KbMacro`)

| Flag | Type | Disposition |
|---|---|---|
| `get <id>` | u8 | CONTEXT |
| `get --offset` | `Option<u8>` | KEEP (read-granularity) |
| `set <id>` | u8 | CONTEXT |
| `set --rep-num` | u16, default 1 | MIGRATE |
| `set --mode` | u8, default 0 (semantics unverified) | MIGRATE |
| `set <actions>` | `Vec<String>` `delay:keycode[:up\|down]`, ≤3840 | MIGRATE |

### `kb profile <sub>` / `kb polling <sub>` / `kb os <sub>`

| Flag | Type | Disposition |
|---|---|---|
| `profile set <index>` | u8, < profile count | CONTEXT (selects active profile, not a profile's contents) |
| `polling set <rate>` | `8k\|4k\|2k\|1k\|500\|250\|125` | MIGRATE |
| `os set <os>` | `windows\|macos` | MIGRATE |

### `kb perf <sub>` (`KbPerf`/`PerfWrite`)

| Flag | Type/constraints | Disposition |
|---|---|---|
| `row`, `col` | u8 | CONTEXT |
| `--mode` | `fixed\|rt` | MIGRATE |
| `--actuation` | f32, >0 and ≤ switch max travel | MIGRATE |
| `--rt-first`/`--rt-press`/`--rt-release` | `Option<f32>` | MIGRATE |
| `--dz-press`/`--dz-release` | `Option<f32>` 0..=1.0 | MIGRATE |

(`set-all` applies the same `PerfWrite` to every key; scope becomes implicit "all keys".)

### `kb light <sub>` / `kb light palette <sub>` (`KbLight`/`KbPalette`)

| Flag | Type/constraints | Disposition |
|---|---|---|
| `area` | u8 | CONTEXT |
| `--on`/`--off` | bool, mutually exclusive | MIGRATE |
| `--mode` | `Option<u8>` (validated against zone's modes) | MIGRATE |
| `--brightness`/`--speed` | `Option<u8>` 0..=100 | MIGRATE |
| `--direction` | `fwd\|back` | MIGRATE |
| `--color` | `Option<u8>` 0..=7 (palette index) | MIGRATE |
| `palette set <colors>` | `Vec<String>`, 1-8 `#rrggbb` | MIGRATE |

### `kb light-sleep set <minutes>`

u16, must be in `omg_model::LIGHT_SLEEP_MINUTES` — MIGRATE.

### `kb calibration start|stop`

Only `--yes`. Device operation (hall-sensor recalibration), not settable
state — KEEP.

## 5. `omg kb916 *`

`info|power|light-colors get|light-data|profile get|doctor fn|wireless` —
no flags beyond global (wireless always returns unsupported).

### `kb916 keys`

`--layer` (u8, default 0) and `--system` (u8, default 0) — both CONTEXT.

### `kb916 reset`

| Flag | Type | What it does | Disposition |
|---|---|---|---|
| `--kind` | u8, default 0 (0=factory, 2=reset-all-layers) | Reset-type selector | KEEP (device operation, fire-and-forget, no read-back — not representable as desired-state) |
| `--yes` | bool | Confirm | KEEP |

### `kb916 set <sub>` (`Kb916Set`)

| Flag | Type/constraints | Disposition |
|---|---|---|
| `key --layer/--system <index> <value>` | u8/u8/usize/string (`0x` hex/decimal/`none`/`empty`/`-`) | layer/system = CONTEXT, index = CONTEXT, value = MIGRATE |
| `keymap --layer/--system <file>` | u8/u8/path or `-` | layer/system = CONTEXT; file content is already a JSON-shaped whole-keymap replacement (precedent, see below) |
| `sleep <seconds>` | u8 | MIGRATE |
| `light-mode <mode> <speed> <bright> --light-type` | u16 / u8 0-20 / u8 0-20 / u8 default 0 | MIGRATE |
| `light-color <index> <color>` | usize 0..125 / `#rrggbb`\|`r,g,b` | index = CONTEXT, color = MIGRATE |

### `kb916 screen <sub>` (`Kb916Screen`)

| Flag | Type | Disposition |
|---|---|---|
| `info` | — | — |
| `plan --width/--height/--frames/--fps/--interval-ms` | u16/u16/usize/`Option<f64>` (conflicts `--interval-ms`)/`Option<u16>` | KEEP (dry-run sizing calc only, sends nothing) |
| `push` | *(no args — see Changes below)* | always refuses | — |
| `clock` | — | always refuses | — |

### `kb916 light <sub>` (`Kb916Light`)

| Flag | Type | Disposition |
|---|---|---|
| `apply <file>` | path or `-` | Already the target pattern: JSON config per `docs/lighting-schema.md`, read-back verified. Template for other `set` commands, not a MIGRATE candidate itself. |
| `apply --yes` | bool | KEEP |
| `stream --pattern/--file/--color/--to/--fps/--duration/--frames/--loop-stream/--yes` | mixed | REVIEW — ephemeral live session, not persisted state; see note below |
| `music --source/--fps/--sensitivity/--brightness/--effect/--background/--beat-intensity/--attack/--decay/--pixel-order/--engine/--palette/--speed/--duration/--yes` | mixed | REVIEW (session params) / KEEP (`--fps`, `--duration`, `--pixel-order` as pacing/wiring concerns) |

**`stream`/`music` note:** these drive a live, ongoing process rather than
writing persistent device state, so "desired-state JSON" doesn't naturally
apply. Recommend a separate "scene/session" command family outside the
config-migration scope; at most, `--effect`/`--palette`/etc. could later
become a named preset referenced *from* config, not a config field itself.
See also the AGENTS.md cmd-8 incident-log entry — these paths are
hardware-unverified for long sessions and must keep their existing
SIGINT/SIGTERM restore handling regardless of how flags are reorganized.

### `kb916 macro <sub>` (`kb916_macro.rs`)

| Flag | Type/constraints | Disposition |
|---|---|---|
| `get <index>` / `set <index>` / `bind <index> <key>` / `delete <index>` / `unbind <key>` | usize / string (idx or `next`) | CONTEXT |
| `set <steps>` | file/`-`/inline JSON per `docs/macro-schema.md` | Already JSON-shaped — not a MIGRATE candidate, it's a precedent |
| `set --name` | `Option<String>`, ≤255 bytes UTF-8 | MIGRATE |
| `set --key --layer --system` | `Option<usize>`/u8/u8 | key = MIGRATE (keymap↔macro link), layer/system = CONTEXT |
| `set`/`bind` `--macro-type`/`--loop-count` | u8 default 1/1 | MIGRATE |
| `bind`/`unbind` `--layer --system` | u8 default 0 | CONTEXT |

### `kb916 adv <sub>` (`kb916_adv.rs`)

| Flag | Type | Disposition |
|---|---|---|
| `get <index>` | usize | CONTEXT |
| `set mt <key> --tap --hold --time` | usize / keyvalue string / keyvalue string / u16 default 50 | key = CONTEXT, rest = MIGRATE |
| `set socd <key_a> <key_b> --direction` | usize/usize/string default `"0"` | keys = CONTEXT, direction = MIGRATE |
| `set tgl <key> --toggle` | usize/usize | key = CONTEXT, toggle = MIGRATE |

## 6. `omg plan *` (`plan.rs`)

| Flag | Type | Disposition |
|---|---|---|
| `create --input` | file or `-` | KEEP (I/O); content is already a desired-state-shaped step list — precedent, not a MIGRATE target |
| `inspect/validate/apply/rollback <id>` | string | CONTEXT |
| `apply`/`rollback --yes` | bool | KEEP |

## 7. `omg cli-schema <path...>`

`Vec<String>` command-path segments — KEEP (agent introspection of the Clap
command tree; the device desired-state schema is `omg config schema`,
a different artifact — see AGENTS.md discussion of this distinction).

## Safety-flag audit

Searched all of `crates/omg/src/*.rs` for `--force`, `--unsafe`,
`--skip-validation`, `--ignore-capability`, `--no-verify` and snake_case
equivalents: **zero matches.** No bypass flag exists for the write-then-verify
or range-validation safety contract (AGENTS.md §Safety 1/3/4). The only flags
touching that contract are `--yes` itself (the required opt-in, not a
bypass) and `--dry-run` (prevents writes, doesn't skip verification of ones
that happen). `--include-serial`, reserved by AGENTS.md §Safety 5, is also
**not implemented** — there is currently no way to turn off serial redaction
either, intentionally.

## Changes already applied from this inventory

- `kb916 screen push` no longer takes a `file` argument. The field was dead:
  `Kb916Screen::Push { .. }` discarded it before unconditionally calling
  `kb916_unsupported(...)` (the GIF→RGB565 pipeline isn't implemented and
  screen writes have no read-back verify). The command still always refuses;
  only the pointless required argument was removed.

## Summary

- **158** distinct flags/args inventoried.
- **KEEP ~45** · **MIGRATE ~70** · **CONTEXT ~35** · **REVIEW ~15**.
- **Three existing precedents already have the shape a future `omg config`
  command needs** and should be generalized rather than redesigned from
  scratch:
  - `omg profile export/import` — whole-device JSON snapshot/restore.
  - `omg kb916 light apply <json>` — JSON config with read-back-verified
    writes (`docs/lighting-schema.md`).
  - `omg plan create/apply` — structured JSON step list with per-step
    verify/journal/rollback (`docs/plan-json.md`).
  Most `MIGRATE` rows above are really asking how a scattered flag folds into
  the shape these three already use, not how to invent a new one.
- Per AGENTS.md: no flag should be removed before its JSON-config replacement
  exists and parity is test-confirmed. The `MIGRATE`/`CONTEXT` dispositions
  here are a target direction, not an authorization to delete anything yet.
