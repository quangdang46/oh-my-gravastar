# Device desired-state JSON config — DRAFT (partially implemented)

**Status: contract review pass 5 (2026-11-26).** The machine-readable
form lives at `schemas/device-config.schema.json` (JSON Schema Draft
2020-12, exercised by `schemas/test_schema.py`, mirrored into CI) and the
Rust CLI now consumes it: `omg config schema` prints the artifact,
`omg config validate <file>` runs offline structural validation with all
errors + JSON paths, and `omg config apply <file> [--dry-run|--yes]`
matches the device, runs layer-2 semantic checks (slot conflicts,
opaque-step preservation, capability gating) and prints a per-section
plan. **Apply coverage today:** compx `settings`/`lighting` `replace`,
khub916 `keymap` patch/replace, `macros`, and `settings.sleep_seconds`.
Everything else is reported `not_applied` with an explicit reason — a
schema-valid document is never reported as apply-able. The Web validator
(AJV) and the remaining section lowerings are still pending. JSON Schema
validity is **structural only**: protocol /
capability gating, base-read checks, keymap-slot conflict detection and
read-back verify are the apply engine's job (layer 2, §8) and are *not*
expressible here — e.g. an HE `keymap` document or a Compx `patch` can be
schema-valid while being unappliable today. This is P0 groundwork
referenced from `docs/cli-flag-inventory.md`; it generalizes three formats
that **do** exist and are shipped, rather than
inventing a config shape from nothing:

1. `omg profile export/import` (schema v1, `crates/omg/src/profile.rs`) —
   whole-device JSON snapshot/restore, already multi-protocol.
2. `omg kb916 light apply <json>` (`docs/lighting-schema.md`) — sparse,
   every-field-optional merge onto live state, read-back verified.
3. `omg plan create/apply` (`docs/plan-json.md`) — structured step list,
   per-step verify/journal/rollback, already spans multiple device families
   in one document.

Plus two data-shape precedents that are already JSON-native and should slot
into this envelope unchanged: `docs/macro-schema.md` (macro steps) and
`docs/advkeys-schema.md` (khub916 SOCD/MT/TGL).

Do not implement any part of this without updating it first if reality
disagrees — this document is the one that must track AGENTS.md §Safety, not
the other way around.

## 0. Decision log

Decisions taken in the 2026-11-26 contract review, with rationale. Items
still open are in §10.

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **No global `mode`.** Each section that supports both update styles declares its own `"mode": "patch" \| "replace"`; sections that only support one style omit `mode` entirely (they are always patch). | A top-level mode plus per-section modes can contradict each other (the draft's own example had both). Per-section mode makes each section self-describing and lets JSON Schema express "which fields are legal" via `if/then` on that section's `mode` alone. |
| D2 | `patch` = partial update request, not desired state; `replace` = complete desired state for that section's managed writable fields. `replace` requires every managed field present — missing is a validation error, never a default (§1a). | AGENTS.md §Safety 6: whole-table writes need a trustworthy base. `patch` never claims untouched fields; `replace` makes the completeness claim explicit so the engine can enforce the base-read check. |
| D3 | Replaceability is per section **and** per protocol, enumerated in §1c. A section that cannot prove full preservation refuses `replace` — it does not pretend. | `advanced_keys` has no delete op and its history blob is written whole; khub916 `macros` region packing is not proven byte-faithful on rebuild; HE has no keymap write API at all. Claiming `replace` there would be a silent lie. |
| D4 | Macro→key binding is `macros[].bind`, an object the engine lowers to a keymap-slot write via `macro916::macro_key_value`. Users never hand-encode tag-3 `0x03 TT NN II` values. | The tag-3 encoding is an implementation detail (type/loop/index bit packing, `kb916_macro.rs`); requiring agents to emit it in `keymap.entries[].value` invites corruption, and a `macro delete` silently invalidates hand-encoded binds. `bind` keeps the intent declarative. |
| D5 | HE `settings.perf` uses `default` + `overrides` with `PerfWrite`-compatible field names; the apply engine expands it through the existing `apply_perf_spec` read-modify-write path (`main.rs`), so switch echo fields are carried, never user-supplied. `perf` is patch-only. | `profile export`'s flat per-slot `perf` array is a *snapshot* format, not desired-state; it stays unchanged. `config export` (future) can emit `default`+`overrides` losslessly because apply is per-key read-modify-write — grouping is cosmetic. The compact shape is what a non-technical Custom flow should produce; listing ~100 keys to change one actuation depth is the wrong UX. |
| D6 | khub916 `advanced_keys` is patch-only and khub916-only. HE `kb hk` modes get a **separate** future section (`hk`), not a `kind` union inside `advanced_keys`. | The two protocols' adv-key models differ (3 kinds vs 7 modes, different parameters, different history mechanism). Sharing one section name invites semantic drift; §Safety 7 demands unknown shapes stay opaque. |
| D7 | Sections that implicitly write keymap slots (`macros[].bind`, `advanced_keys` entries) conflict-check against the `keymap` section: two writers for the same `(layer, system, slot)` is a validation error. | adv binds always target keymap arg 0 (`kb916_adv.rs::read_state` reads `key_values(0)`); macro binds target any arg. Without a conflict rule, apply order would silently decide which write wins. |
| D8 | `mode` is **required** on every section that defines it. No implicit `"patch"` default. | Same rule as every other field: an absent mode must not be guessed. A document that means patch says `"mode": "patch"`. |

## 1. Why a new envelope, and what it does not replace

`profile` already proves a multi-protocol JSON envelope works. What it
*doesn't* do:

- It has no **validate-without-a-device** step — `load_profile`/
  `validate_profile` only check `schema == 1` and array shapes exist; it does
  not check per-field ranges, enums, or capability before attempting writes.
- khub916 import is **export-only** (`KHUB916_WRITE_SKIP`) — there is no
  read-modify-write mapping from a profile `state` object back onto the
  device for khub916, only for HE (`he_plan`/`HeWrite`) and the mouse
  (`mouse_config_from_json`).
- It is one win-or-lose blob per device, not a sparse "change only these
  fields" patch — `kb916 light apply` and the web `SetKeymapOp.entries`
  pattern (`web/src/agent/action-schema.ts`) already prove sparse patches are
  the right shape for day-to-day edits; `profile import` is the right shape
  for full backup/restore.

This proposal keeps **both** shapes, explicitly named per section (D1/D2),
rather than forcing one:

- **`"mode": "patch"`** — every field in the section optional; absent = keep
  current device value. This is `kb916 light apply` generalized.
- **`"mode": "replace"`** — every managed-writable field in the section
  **required**; a missing field is a validation error, never a silent
  default or `0`. This is `profile import` / `kb916 set keymap <file>`
  generalized, and is the only mode allowed to touch a whole table
  (AGENTS.md §Safety 6: the base read must be known-good before a replace
  proceeds).

Whichever mode a section uses, **unknown properties are rejected** — a
closed schema, per `docs/SAFETY-SPEC.md` §1. This is new: `profile`'s
current `validate_profile` does not reject unknown keys.

**No implicit defaults, ever.** Neither mode may substitute a default value
for a missing field — `patch` leaves it untouched (reads current), `replace`
rejects the document (field is mandatory), and a section that supports only
one mode has no `mode` field at all rather than an optional one (D8). Nothing
in an apply engine built from this draft may use `unwrap_or_default`,
zero-fill, or a hardcoded fallback for a missing or unparseable value
(AGENTS.md §Safety 6) — this applies to the schema validator itself, not
just the Rust/TS apply code: a validator that coerces a missing field to
`0`/`false`/`""` to make a document "valid" is a bug in the validator.

**Opaque data is never a user-managed field.** Where a protocol has slots
this draft's field list doesn't name (unknown keymap tags, the TGL-footer
quirk bytes in §6, the 18 constant template bytes per lighting-data group
in §4) — those bytes are not part of any section's schema at all, patch or
replace. The apply engine carries them through byte-identical from the
pre-apply read; they must never appear as a field a config document can set,
and a document that tries to set one is rejected as an unknown property,
not silently ignored (AGENTS.md §Safety 7).

## 1a. `mode` semantics, precisely

"patch" is a **partial update request**, not a desired state: fields absent
from a `patch` document carry no claim about the device's current value —
the apply engine reads current state and changes only the fields given.
"replace" is a **complete desired state for that section**: every
managed-writable field for that section/protocol must be present, or the
request is rejected before any write (AGENTS.md §Safety 6). A `replace`
section is never partially satisfied by falling back to a read — that
would silently turn a replace into a patch. Per protocol/section,
"managed writable fields" is exactly the field list given in §3–§7 for that
protocol; a `replace` validator must reject the section if any of them is
missing, not treat missing-as-keep.

A section may support only one mode. In that case the section schema has
**no `mode` property** — its semantics are fixed, and writing `"mode"` into
it is an unknown-property error. The full support matrix is §1c.

## 1b. Device identity and schema compatibility

`device` in §2 matches a **device class** (protocol + vid + pid +
usage_page), not a single physical unit or firmware revision — same
matching semantics as `profile.rs::device_matches_entry` today. This
envelope does **not** carry a firmware-version or serial field (serials are
excluded by AGENTS.md §Safety 5 regardless). Consequence: a config that
validates against the schema can still be rejected at apply time if the
*specific connected unit's* capability registry or firmware doesn't support
a field the schema allows for its protocol class — that check is layer 2
(§8), never layer 1. A config file is portable across units of the same
VID/PID/usage_page; it is not guaranteed portable across firmware
revisions or between models that happen to share a protocol family (e.g.
two different khub916 models with different HK-equivalent support) — the
capability check at apply time is what catches that, the schema shape alone
cannot.

## 1c. Section ownership / mode / replaceability matrix

This is the authoritative answer to "which sections exist, which modes they
take, and whether `replace` is honest". A field appearing in §3–§7 for a
protocol means *this schema has a slot for it*; it does not mean *every
device claiming that protocol can read, write, and read-back-verify it*.
The per-device-per-feature verification matrix is
`docs/CAPABILITY-MATRIX.md` §2 — the apply engine must look a field up there
before sending it (layer 2, §8).

| Section | khub916 | he-v2 | compx | sparkplayjoy |
|---|---|---|---|---|
| `keymap` | patch + replace (§3) | replace shape defined, **no write path** — apply always rejects until `HeWrite::Keymap` exists | rejected (unknown property) | rejected |
| `lighting` | patch only, no `mode` (§4 — it *is* `lighting-schema.md`) | patch only, no `mode` | patch + replace (`mode` required); **patch is schema-valid but apply-rejected** until a read-merge path exists (§10.4) | rejected |
| `macros` | patch only, no `mode` (§5; no `delete` in v1) | rejected | rejected | rejected |
| `advanced_keys` | patch only, no `mode` (§6) | rejected — HE `hk` modes are a separate future `hk` section (D6) | rejected | rejected |
| `settings` | patch only, no `mode` (§7 — one field today) | patch only, no `mode` | patch + replace (`mode` required); **patch is schema-valid but apply-rejected** until a read-merge path exists (§10.4) | rejected |

"patch only, no `mode`" means the section object has no `mode` key at all —
writing one is an unknown-property error (D1/D8). "patch + replace" means
`mode` is a **required** property of the section object.

Replaceability rationale (D3):

- **khub916 `keymap` replace** — honest: `write_keymap`/`write_keymap_rmw`
  write the whole 128-slot table (zero-padded, `5A A5` trailer) and verify
  read-back; the base-read completeness check of §Safety 6 applies. The
  catch is per-device slot count: the wire table is 128 u32 but devices
  report only populated slots (K1 Pro: 126 — `KEYMAP_ENTRIES`/`write_keymap`
  docs in `crates/proto-khub916/src/lib.rs`). `values[]` must match the
  count the base read returned, which is a layer-2/engine check, not a
  schema constant.
- **khub916 `macros`** — patch only. `write_macro_region` +
  `build_macro_region` exist (used by `macro delete`'s rebuild), so a
  whole-region replace is *possible*, but byte-faithful repacking of
  name/step records is not yet proven against §Safety 8 (semantic
  preservation of untouched entries). Deferred until a round-trip test
  exists.
- **khub916 `advanced_keys`** — patch only, permanently for now: the
  protocol has **no delete** (vendor has none either — `kb916_adv.rs`
  header), and the history blob is written whole via cmd 18. "Replace"
  would silently leave stale history slots behind.
- **he-v2 `keymap`** — no `HeWrite::Keymap` variant exists; `he_plan`
  explicitly notes "the proto has no keymap write API (03 02/03 04 are
  documented but unimplemented)". The section shape mirrors `profile
  export`'s `keymap` array so `config export`/`validate` can round-trip
  it, but every apply attempt is rejected at layer 2.
- **compx** — `mouse_config_from_json` is all-or-nothing today, so
  `settings`/`lighting` `replace` maps 1:1 onto shipped code. `patch` is
  defined in the schema but **not implementable yet**: there is no
  partial-config path and no read-merge in `mouse_config_from_json` — the
  engine must read `mouse_state().config` and merge before this is usable
  (open question §10.4).
- **sparkplayjoy** — read-only protocol (`SPARK_WRITE_SKIP`); any section
  is rejected.
- **khub916 wireless** (`DeviceKind::Khub916Wireless`) — `kb916_session`
  refuses non-wired transports; the `khub916` protocol string matches both
  kinds for `validate`/`diff`, but apply on a wireless unit is rejected at
  layer 2.

## 2. Envelope

```json
{
  "schema_version": 1,
  "device": {
    "protocol": "khub916" | "he-v2" | "compx" | "sparkplayjoy",
    "vid": 9610, "pid": 268, "usage_page": 65280,
    "model": "optional, informational only — matching is protocol+vid+pid+usage_page"
  },
  "keymap":     { "…": "protocol-specific, §3 — mode required where defined" },
  "lighting":   { "…": "protocol-specific, §4" },
  "macros":     [ "…": "docs/macro-schema.md shape + bind, §5 — khub916 only" ],
  "advanced_keys": [ "…": "docs/advkeys-schema.md shape, khub916 only, §6" ],
  "settings":   { "…": "protocol-specific, §7" }
}
```

- `schema_version` — required, must be `1`.
- `device` — required. `protocol`/`vid`/`pid` required; `usage_page`
  optional (matches `profile.rs::device_matches_entry`, which treats a
  missing `usage_page` as "any"). `model` is informational, never used for
  matching (so a renamed/relabeled unit doesn't silently fail to match).
- There is **no top-level `mode`** (D1). Update style is declared inside
  each section that supports both (§1c).
- Every section is optional at the envelope level — a doc may touch only
  `lighting`, only `settings`, etc. What's **not** optional is *within* a
  section once that section is present and declares `mode: "replace"`,
  nor the `mode` key itself on sections that define it (D8).
- A document must contain at least one section; an empty envelope is a
  validation error (nothing to apply).

## 3. `keymap` (protocol-specific)

### khub916

`mode` is required. `"patch"` uses `entries`; `"replace"` uses `values`.
The two are mutually exclusive — a section carrying both, or neither, is a
validation error.

```json
{
  "keymap": {
    "layer": 0, "system": 0,
    "mode": "patch",
    "entries": [ { "index": 5, "value": "0x1234" }, { "index": 79, "value": "none" } ]
  }
}
```

```json
{
  "keymap": { "layer": 1, "system": 0, "mode": "replace",
              "values": [ "0x…", "… one entry per reported slot …" ] }
}
```

- `layer`/`system` select the keymap arg (`arg = layer & 3 | (system & 1)
  << 2`, `crates/proto-khub916/src/lib.rs:853`) — required, no default,
  since silently defaulting to layer 0 could overwrite the wrong layer.
  Valid ranges: `layer` 0–3, `system` 0–1 (same check as
  `kb916_macro.rs::check_keymap_args`).
- `entries[].index` — 0-based slot index, `< KEYMAP_ENTRIES` (128).
  `entries[].value` / `values[]` use the same string grammar as
  `omg kb916 set key` (`0x` hex ≤ `0xFFFFFFFF` / decimal 0–4294967295 /
  `none` / `empty` / `-`) — the schema encodes the u32 bound exactly.
- `entries` must be non-empty in patch mode; duplicate `index` values in
  one `entries` array are a validation error (which one wins would
  otherwise depend on write order).
- `replace` requires `values` to contain **exactly as many entries as the
  base read reports for that arg** — device-dependent (K1 Pro: 126 of the
  128-slot wire table; the proto pads short tables to 128 on write and
  compares zero-padded on verify). This count cannot be a schema constant;
  it is checked by the engine against the live read. Per AGENTS.md
  §Safety 6/7, **the apply engine must read the current table first and
  refuse if that read is short, failed, or any slot decodes to an unknown
  tag** — it is not enough that the *incoming* JSON has N entries.
- Per AGENTS.md §Safety 7, the Fn layer's tag `0x08` (undocumented as of
  2026-11-25) makes a full Fn-layer (`layer=1`) `replace` **impossible
  until that tag is reversed**: the base-read check above will always
  abort it. Only `patch` with `entries` targeting *other* slots is viable
  on layer 1 today, and even then any entry whose current value already
  decodes as `0x08…` must be carried through byte-identical, never
  offered for edit.
- Tag `0x03` (macro bind) and `0x20/0x21/0x22` (adv-key bind) are **known
  tags** — they may appear in `values[]`/`entries[].value` verbatim. But
  prefer `macros[].bind`/`advanced_keys` for those slots (D4/D7): the
  conflict rule in §8 rejects a document that sets the same slot through
  both paths.

### he-v2

`mode` is **required** (D8) and the only defined value is `"replace"` —
the patch shape is not defined, so `"patch"` is a validation error, not a
forward-compat allowance. The shape mirrors `profile export`'s `keymap`
array, but **no apply path exists** — rejected at layer 2 until
`HeWrite::Keymap` lands (see §1c).

```json
{
  "keymap": {
    "mode": "replace",
    "layers": [ { "layer": 0, "rows": [ { "row": 0, "codes": [1,2,3,"…21 total…"] } ] } ]
  }
}
```

- Generalizes `he_state`'s `keymap` array (`{layer, row, codes}` flattened)
  and `HeWrite`'s absence of a keymap variant today — HE keymap writes
  currently go through `kb keys set`/`set-row` only; there is no
  `HeWrite::Keymap` yet. This section is written in the shape the data
  already has in `profile export`, but **the apply side does not exist**
  until `he_plan`/`HeWrite` grows a keymap variant — do not assume this
  works end-to-end before that lands.
- `codes` per row must have exactly `omg_model`'s row width (21 on the
  devices currently modeled); a short row is a `replace`-mode validation
  error, not a partial write. The width is device-dependent, so the exact
  count is an engine check, not a schema constant.

### compx (mouse)

No `keymap` section — Compx mice have no keymap concept in the current
protocol (`mouse_config_from_json` has no keymap field). A `keymap` key
under `protocol: "compx"` is rejected as an unknown property.

## 4. `lighting` (protocol-specific)

### khub916

Identical to the existing `kb916 light apply` config
(`docs/lighting-schema.md`) — this section **is** that document's schema,
unchanged, and has **no `mode` property** (always patch):

```json
{
  "lighting": {
    "mode": 15, "speed": 4, "brightness": 10,
    "color_model": 2, "color": "#3366ff",
    "keys": { "preset": "rainbow" }
  }
}
```

`docs/lighting-schema.md` already requires at least one of
`mode`/`speed`/`brightness`/`color_model`/`color`/`keys` to be present;
this draft does not change that. (Note the name collision is intentional
and local: `lighting.mode` inside this section is the *effect mode id*,
unrelated to the removed section-level `mode`.)

### he-v2

Generalizes `HeWrite::Lighting` / `light_json` per zone. Patch only — no
`mode` property; every entry is already a per-area partial update:

```json
{
  "lighting": {
    "areas": [
      {
        "area": 0, "on": true, "mode": 2, "brightness": 50, "speed": 4,
        "direction": "fwd", "color_index": 0,
        "palette": ["#ff0000", "…"]
      }
    ]
  }
}
```

- `area` required per entry (scope, matches `kb light set <area>`).
- All other fields optional per entry — per-area patch, matching
  `he_plan`'s existing diff-and-write-only-if-changed behavior.
- `palette` replaces the whole palette for that area when present (matches
  `kb light palette set`'s `Vec<String>` semantics — not sparse).
- No `replace` form: the set of lighting areas is device/firmware
  dependent, so "all areas" cannot be enumerated in the schema (§1c).

### compx (mouse)

Generalizes the `light` object inside `mouse_config_from_json`. `mode` is
**required** (`patch` | `replace`):

```json
{
  "lighting": { "mode": "replace",
                "on": true, "mode_id": 2, "color": "#3366ff", "speed": 5, "brightness": 7 }
}
```

- `mode: "replace"` — every sub-field required together, mirroring
  `mouse_config_from_json`'s current all-or-nothing parse of the `light`
  object. `speed`/`brightness` are 1–9 here (the `mouse set light`
  writable range); note `mouse_validate` in `profile.rs` accepts 0–9 —
  the stricter CLI range is what the schema encodes.
- `mode: "patch"` — every sub-field optional. **Schema-valid but
  apply-rejected today**: the shipped parser has no partial form; the
  engine must read `mouse_state().config` and merge first (open question
  §10.4). JSON Schema validity must never be read as apply-ability — the
  capability gate (§8, layer 2) is what rejects it.
- The effect-mode field is named `mode_id` (u8, `0..=6` per
  `MouseLight::Mode`) to avoid colliding with the section's own required
  `mode` (D1) — same `*_id` convention `profile.rs::perf_json` already
  uses. `color` is a `"#rrggbb"` string (envelope convention, matching
  khub916 `lighting.color` and HE `palette` entries); the engine converts
  to `pm::Rgb`. This deliberately differs from `profile export`'s
  `{r,g,b}` object — `config export` emits the string form.

## 5. `macros` (khub916 only)

Patch only — **no `mode` property**. Array of entries in the
`docs/macro-schema.md` object shape, plus `index` and an optional `bind`:

```json
{
  "macros": [
    { "index": 0, "name": "jump",
      "steps": [ { "action": "down", "key": "KeyA", "delay_ms": 8 } ],
      "bind": { "key": 5, "layer": 0, "system": 0,
                "macro_type": 1, "loop_count": 1 } },
    { "index": "next", "name": "combo", "steps": [ "…" ] }
  ]
}
```

- `index` — entry slot: non-negative integer, or the string `"next"` to
  append (matches `kb916 macro set <index>`).
- `steps` validation is exactly `docs/macro-schema.md` (key names, 20-bit
  `delay_ms`, `kind` disambiguation) — this section does not redefine it.
  `steps` is required on every entry. The schema additionally enforces the
  parser's real domains, which `docs/macro-schema.md` describes
  informally: `key`/`code` must be a `MACRO_KEY_TABLE` name
  (case-insensitive; `Fn` and `counter` are excluded because their usage
  exceeds the u8 step encoding), `0x` hex ≤ `0xFF`, or decimal 0–255; and
  `delay_ms`/`duration` are aliases — declaring **both** is a validation
  error so the parser can never silently drop one.
- **No `delete` in v1.** `omg kb916 macro delete` exists and rebuilds the
  region via `build_macro_region`/`write_macro_region`, but that rebuild
  is parse→re-encode and is **not** round-trip safe for unnamed steps —
  two independent failures, either sufficient:
  1. `step_json` emits `StepKind::Other(n)` as `{"kind":"other",
     "nibble":n, "usage":u}` with **no `key`/`code` field**, so
     `parse_one_step` fails earlier still — it requires `key` or `code`
     and never reads `usage`.
  2. Even if the missing-key issue were fixed, `parse_kind` only accepts
     `key|modifier|mouse` — adding `"other"` there alone would not restore
     round-trip, because the `nibble` payload also has no input field.
  `encode_step` *can* re-emit `Other(n)` if the `MacroStep` survives
  intact — the loss happens entirely in the JSON parse layer. So a
  rebuilt region can silently drop or corrupt steps it could not name.
  That is exactly the §Safety 7/8 failure mode, so desired-state delete
  stays out until a preservation proof exists (same gate as `replace`,
  §10.2). The same caveat applies to `steps` *replacement* of an entry
  whose stored steps contain `Other` kinds — the engine must compare the
  pre-write parse against the post-write read and refuse when they
  diverge. **CLI status (pass 5):** `omg kb916 macro set`/`delete` now
  fail closed — before any region rebuild they scan every entry the user
  did not explicitly target and refuse if it contains a `StepKind::Other`
  step (error names the macro and step index); the targeted entry itself
  is exempt since replacing/removing it is the declared intent.
  `macro delete` additionally scans all keymap args for tag-3 bindings and
  warns about slots bound to the deleted index and bindings to later
  indices that silently retarget after region compaction — it never
  unbinds implicitly.
- `bind` (D4) — optional object, all five fields **required when present**
  (no defaults, D8): `key` (0-based keymap slot), `layer` (0–3), `system`
  (0–1), `macro_type` (u8), `loop_count` (u8 ≥ 1). The engine lowers it to
  `macro916::macro_key_value(macro_type, loop_count, index)` and performs
  the same verified keymap-slot write as `kb916 macro bind`. `bind.index`
  is the entry's own `index` — it must be an integer (not `"next"`), since
  the tag-3 value encodes the slot number; `"next"` + `bind` is a
  validation error (the appended index isn't known until apply time).
- **Conflict rule (D7):** a `bind` targeting `(layer, system, key)`
  conflicts with any `keymap` section write to the same slot — patch
  `entries[].index` or `replace values[]` position — and with another
  `bind`/`advanced_keys` entry targeting it. Conflict = validation error,
  never last-write-wins.
- HE v2 macros (`kb macro set <id> --rep-num --mode <actions>`) are **not**
  mapped here — `docs/macro-schema.md` is khub916-specific (vendor key
  name table, 20-bit delay encoding). HE's `delay:keycode[:up|down]` token
  grammar is a different wire format; a shared `macros` section needs a
  `format` discriminator before HE can use it. Left as an open item (§10).

## 6. `advanced_keys` (khub916 only)

Patch only — **no `mode` property**, and `replace` is rejected by design
(D3): the protocol has no delete and the 512-byte history blob
(SOCD 200 B + MT 200 B + TGL 112 B) is written whole via cmd 18.

```json
{
  "advanced_keys": [
    { "kind": "mt", "key": 5, "tap": "0x04", "hold": "0x05", "time_ms": 500 },
    { "kind": "socd", "key_a": 10, "key_b": 11, "direction": 0 },
    { "kind": "tgl", "key": 20, "toggle": 21 }
  ]
}
```

Directly mirrors `docs/advkeys-schema.md`'s `adv set mt|socd|tgl` field
names, with `kind` as the discriminator (the CLI uses subcommands; JSON
needs the tag). Per-key field names:

- `mt`: `key`, `tap`, `hold` (keymap-slot u32 value grammar, same as
  `keymap.entries[].value`), `time_ms` — the CLI's `--time` is in 10 ms
  units; this schema exposes **milliseconds** and the engine converts
  (`time_ms` must be a multiple of 10, range per vendor slider 0–1000 ms).
- `socd`: `key_a`, `key_b`, `direction` (0–3 or the string tokens
  `last|key-a|key-b|cancel` — same set as `parse_direction`).
- `tgl`: `key`, `toggle`.

Each entry lowers to one `advkey::plan_*` + the vendor's two writes
(keymap arg 0 via `write_key_configuration`, then history blob via
`write_adv_key_config`), both read-back verified — exactly what
`kb916 adv set` does today.

Caveats the apply preview must surface (not just doc comments, per
`docs/SAFETY-SPEC.md` §2):

- The keymap half of a bind is **live-verified**; the cmd-18 history-blob
  half is **code-verified, persistence unconfirmed** on the only hardware
  tested (K1 Pro) — and cmd-146 read-back is unstable for ~60 s after a
  cmd-18 write, so verification may require a fresh session (existing
  behavior in `kb916_adv.rs`).
- There is **no delete**: an entry can only be added or have its keymap
  binding re-pointed, never removed from the history blob.
- **Conflict rule (D7):** every `advanced_keys` entry implicitly writes
  keymap arg 0 (`layer=0, system=0`) at its `key`/`key_a`/`key_b`/`toggle`
  slot(s). Those slots conflict-check against the `keymap` section exactly
  like `macros[].bind`.

he-v2 `kb hk` modes (DKS/MPT/MT/TGL/END/SOCD/RS — more modes, different
parameters: `--kcs`/`--trps`/`--dbs`/`--time`/`--delay`/`--row2`/`--col2`)
are **not** in this section (D6). They get a separate `hk` section in a
later revision; until then `advanced_keys` under `protocol: "he-v2"` is
rejected as an unknown/unsupported section.

## 7. `settings` (protocol-specific)

### khub916

Patch only — no `mode` property. Only field with a known write path today
(`kb916 set sleep`):

```json
{ "settings": { "sleep_seconds": 60 } }
```

`sleep_seconds` is a u8 matching the profile blob's sleep field. `light_mode`
inside the existing `profile` read (`p.light_mode()`) is **derived display
data about the lighting profile blob**, not an independent settings field —
it is covered by `lighting.mode` above, not duplicated here.

### he-v2

Patch only — no `mode` property. Generalizes `HeWrite::{Polling,Os,
LightSleep}` and the per-key `Perf` variant:

```json
{
  "settings": {
    "polling_hz": 1000, "os": "windows", "lighting_sleep_minutes": 5,
    "perf": {
      "default": { "mode": "fixed", "actuation_mm": 1.2 },
      "overrides": [
        { "row": 0, "col": 0, "mode": "rt", "actuation_mm": 0.4,
          "rt_press_mm": 0.1, "rt_release_mm": 0.1,
          "dz_press_mm": 0.3, "dz_release_mm": 0.3 }
      ]
    }
  }
}
```

- `polling_hz` must be one of 8000/4000/2000/1000/500/250/125 — matches
  `he_plan`'s existing "id in supported list" check (`rate_hz` table,
  `profile.rs`). This is one of the two L3 (disruptive) ops in
  `docs/SAFETY-SPEC.md` §3: the keyboard restarts and a missing read-back
  after apply is `verified:false`, not an error (same exception already
  carved out for `kb polling set` in AGENTS.md).
- `os`: `"windows"` | `"macos"`. `lighting_sleep_minutes`: u16, must be in
  `omg_model::LIGHT_SLEEP_MINUTES`.
- `profile` (active profile index) is deliberately **not** a settings
  field — it's captured in `profile export` but has no desired-state use
  case yet; revisit if one appears.

#### `settings.perf` (D5)

`perf` is an object with `default` (a PerfSpec applied to **every** key
slot — the `kb perf set-all` case) and `overrides` (array of
`{row, col}` + PerfSpec applied after `default` — the `kb perf set` case).
At least one of `default`/`overrides` must be present. `perf` is
**patch-only**: there is no `replace`, because "every managed field for
all ~100+ slots" cannot be enumerated without a device read, and the
switch echo fields (`axis`, `calibrate`, `axis_v2_id`,
`axis_range_max`, `axis_coefficient`) are device-owned, never
user-managed.

PerfSpec fields mirror `PerfWrite`/`apply_perf_spec` (`main.rs`) exactly:

| Field | Required | Meaning |
|---|---|---|
| `mode` | yes | `"fixed"` \| `"rt"` |
| `actuation_mm` | yes | primary depth: `normal_press` when `mode:"fixed"`, `rt_first_touch` when `mode:"rt"` — same aliasing `PerfWrite.actuation` performs |
| `rt_first_mm` | no | overrides `rt_first_touch` after `actuation_mm` is applied |
| `rt_press_mm` | no | `rt_press` |
| `rt_release_mm` | no | `rt_release` |
| `dz_press_mm` | no | `press_deadzone` |
| `dz_release_mm` | no | `release_deadzone` |

(`actuation_mm` required mirrors the CLI: `PerfWrite.actuation` is a
required arg even though the optional fields exist. A caller that wants to
change only deadzones must restate the current actuation — same constraint
as `kb perf set` today.)

- Range validation is unchanged: actuation/rt depths `0 < v ≤
  max_travel(axis_v2_id)` per switch, deadzones `0..=1.0 mm`
  (`apply_perf_spec`, `omg_model::DEADZONE_MAX`). The per-switch travel
  limit is only knowable after the per-key read, so depth-range checks are
  layer-2/engine, not pure schema.
- Apply lowering: each affected `(row, col)` becomes one
  `apply_perf_spec(current, spec)` → `HeWrite::Perf` — read-modify-write,
  echo fields carried, ~5 ms/key pacing, per-key read-back verify. All
  existing behavior, no new proto surface.
- **Export compatibility (resolved, D5):** `profile export`'s flat
  `state.perf` array is a snapshot format and stays unchanged. A future
  `omg config export` emits this `default`+`overrides` shape; it is
  lossless because apply is per-key read-modify-write — identical specs
  across keys may be grouped into `default`, the rest into `overrides`,
  and any grouping (including "everything in overrides, no default") is
  semantically identical.

### compx (mouse)

Generalizes the non-lighting fields of `mouse_config_from_json` (buttons
included — they are `MouseConfig` state, not lighting) — **except
colors**, which use the
envelope's `"#rrggbb"` string convention rather than the export's
`{r,g,b}` objects (engine converts; `config export` emits strings).
Field names otherwise match the shipped Rust struct on purpose. `mode` is
**required**:

```json
{
  "settings": {
    "mode": "replace",
    "report_rate": 1000, "stage_count": 4, "current_stage": 0,
    "dpi": [800, 1600, 3200, 6400, 0, 0, 0, 0],
    "dpi_colors": ["#ff0000", "…8 entries total…"],
    "lod": 2, "debounce": 4, "motion_sync": true, "sleep": 60,
    "angle_snap": false, "ripple": false, "light_off_moving": false,
    "sensor_mode": 0,
    "buttons": [ { "kind": "dpi", "param": "0x00" }, "…BUTTON_SLOTS total…" ]
  }
}
```

- `mode: "replace"` — **every** field above is required, exactly
  mirroring `mouse_config_from_json`'s current all-or-nothing parse (it
  already has no partial-config path — this draft does not relax that).
  `dpi`/`dpi_colors` must have exactly `pm::MAX_STAGES` (8) entries;
  `buttons` exactly `pm::BUTTON_SLOTS` (6) entries — short arrays are a
  validation error, never zero-padded (AGENTS.md §Safety 6). `dpi[]`
  entries are u32 (0..=4294967295).
- Field domains enforced by `mouse_validate` (`profile.rs`) and now by
  the schema: `report_rate` ∈ {125,250,500,1000,2000,4000,8000}
  (device `max_rate` may be lower — layer 2), `stage_count` 1–8,
  `current_stage` 0–7, `debounce` 0–15, `sleep` ∈ `pm::SLEEP_TIMES`
  raw values {1,3,6,12,18,30,60,90} (a wire byte, **not** the seconds
  list `mouse set sleep` takes), `sensor_mode` ∈ {0,1}, `lod` validated
  against the sensor's table (layer 2). `current_stage < stage_count` is
  **not** an invariant the shipped code enforces — `mouse_validate` only
  bounds each field independently — so the schema does not express it
  either; whether it *should* be one is an engine decision (§10).
- `mode: "patch"` — any subset of the fields. Same caveat as mouse
  lighting: not implementable until the engine gains a read-merge path
  (§10.4).
- `button.kind` uses the same string tokens as `mouse set button`
  (`disabled|mouse|dpi|shortcut|macro`), not the raw `pm::BUTTON_*`
  byte constants — keeps the config human-writable. `param` is optional;
  when absent the engine applies the same deterministic defaults as
  `parse_button` (`main.rs`: mouse/dpi → `0x0100`, others → `0`,
  disabled → ignored). These are fixed encoding defaults, not device-state
  guesses, so they don't violate the no-implicit-defaults rule — but they
  are documented here so the schema doesn't have to encode them.
  When present, `param` must satisfy `parse_button`'s u16 cast: hex
  `0x` ≤ 4 digits or decimal **0–65535**; names
  `left|right|middle|back|forward` are valid only for `kind: "mouse"`.
- The `light` object is **not** part of `settings` — it lives in the
  `lighting` section (§4 compx). A full mouse restore sets both sections
  to `"mode": "replace"`.

## 8. Cross-cutting rules (inherited, not new)

These restate existing AGENTS.md/SAFETY-SPEC.md rules as they apply to this
envelope — nothing below is a new policy, it's where each rule lands once
there's a specific document shape to apply it to:

- **Unknown tag ⇒ opaque, byte-preserved** (AGENTS.md §Safety 7). Any
  keymap/macro/advanced-key slot the decoder cannot name must round-trip
  through `patch`-mode apply unchanged and must never appear as an editable
  field in a generated "current state" view that feeds a `replace`. A
  `replace` whose base-read contains an unknown-tag slot is an ABORT, full
  stop — this is why §3's khub916 example calls out the Fn-layer tag-`0x08`
  case explicitly rather than leaving it implicit.
- **Keymap-slot write conflicts are validation errors** (D7). Collect all
  implicit keymap writers — `keymap` section itself, `macros[].bind`,
  `advanced_keys` entries (always arg 0) — and reject the document if two
  of them target the same `(layer, system, slot)`. Never resolve by
  ordering.
- **Read-back verify ≠ semantic preservation** (AGENTS.md §Safety 8). The
  apply engine's report for this envelope must include a per-section diff
  against the **pre-apply read**, not just a `verified: true/false` per
  write — so a caller can confirm untouched sections truly didn't move.
- **Capability/firmware gating is layer 2, not layer 1** (SAFETY-SPEC.md
  §2). A section being present and structurally valid does not mean it's
  appliable: `supported: false` in the capability registry rejects the
  whole section before any write is attempted (e.g. `he-v2` has no
  `advanced_keys`/`keymap` write path, `sparkplayjoy` has no writable
  section at all, khub916 wireless refuses writes).
- **L0–L4 policy** (SAFETY-SPEC.md §3) still applies per effective
  operation once a section is lowered to writes — `settings.polling_hz` is
  L3 regardless of being expressed as a flag or a JSON field; nothing in
  this envelope is L4 and none of the §2 blacklist commands
  (`kb916 reset`, `kb916 screen push|clock`, mouse pairing/bootloader/RF,
  HE factory reset/firmware/wipe/gamepad/restart/default-switch) has a
  representation here on purpose — they stay CLI action-verbs, never
  config fields (AGENTS.md §Safety 2).
- **Empty / no-op patches are validation errors.** A `mode: "patch"`
  section that declares no field capable of producing a change (e.g.
  `settings: {"mode": "patch"}` with nothing else) is rejected by the
  schema — silently accepting it would make "applied" reports meaningless.
  Objects that *are* allowed to be sparse (e.g. khub916 `lighting.keys`
  per-row overrides, `he-v2` lighting `areas[]` entries) require at least
  one effective property.
- **`--yes` is still required to apply**, `--dry-run` still only prints
  the plan. This envelope does not change how confirmation works, only
  what a single write path's input looks like.

## 9. Explicitly out of scope for this draft

- ~~No validator implementation~~ — done: `schemas/device-config.schema.json`
  (Draft 2020-12) is consumed by all three implementations — python
  `jsonschema` (`schemas/test_schema.py`, 106 cases), Rust `jsonschema`
  crate embedded via `include_str!` (`crates/omg/src/config.rs`), and Web
  AJV 2020-12 importing the same file (`web/src/device-config.ts`).
  `schemas/fixtures/cases.json` is exercised by all three so they must
  agree. Layer-2 semantic checks live in the apply engines — Rust
  `semantic_errors` (`crates/omg/src/config.rs`) and Web
  `configSemanticErrors` (`web/src/config-apply.ts`) — and both consume
  `schemas/fixtures/semantic-cases.json` (duplicate patch indices,
  (arg, slot) conflicts, duplicate macro indices, sparkplayjoy
  rejection) so the checks cannot drift apart. The capability gate is
  pinned the same way for the khub916 subset via
  `schemas/fixtures/plan-cases.json` (consumed by `plan_sections` tests
  and `planConfigSections` smoke); compx expectations legitimately
  differ — the web app has no Compx write path. The Web
  guided apply (`web/src/config-apply.ts` + the Custom config page)
  lowers the same khub916 subset through the safety transaction layer
  (confirm = approval, SnapshotStore restore point, journal,
  stop-on-first-failure); compx/he-v2 sections report `not_applied`
  there exactly as the CLI does.
- ~~No `omg config` command~~ — shipped as `omg config schema|validate|apply`
  (`crates/omg/src/config.rs`). Apply lowering exists for: compx
  `settings`/`lighting` `replace`, khub916 `keymap` patch/replace,
  `macros` (with the `StepKind::Other` fail-closed guard and post-write
  entry comparison), `macros[].bind`, and `settings.sleep_seconds`.
  Every other section reports `not_applied` with an explicit reason.
  `apply --yes` captures a read-only snapshot of every section BEFORE
  the first write and persists it as `<id>.snapshots.json` — if the
  snapshot cannot be written, the apply aborts before any bytes are
  sent. The journal `<id>.journal.json` is rewritten after every section
  (and once more with the final status), so a mid-apply crash still
  leaves an accurate record. Apply stops at the first hard failure AND
  at the first `applied_unverified` write; `omg config rollback <id>
  --yes` verifies the connected device's protocol + VID + PID + usage
  page against the recorded identity before restoring through the same
  verified write paths. The legacy `kb916 macro set|delete|bind|unbind`
  commands persist the same snapshot format before writing, so they are
  rollback-able via `omg config rollback` too. Approval is bound to the
  previewed plan: the dry run prints a `plan_hash` (sha256 over the
  canonical doc + selected device identity + section plan) and `--yes`
  refuses to run without `--approve <hash>` matching it — a changed
  doc, different device or different plan produces a different hash.
  The hash is recorded in `<id>.journal.json`. **Remaining gap vs
  §3.5:** per-op L0–L4 classification is implicit in the lowering
  rather than a separate policy table (the Web side has
  `web/src/safety/policy.ts`).
- ~~No decision on whether `omg plan`'s existing step-based format wraps
  this envelope~~ — decided (docs/plan-json.md "Relationship to `omg
  config`"): they are complementary. `config` is not a plan family yet —
  plan steps resolve one `DeviceKind` up front while a config document
  picks its device from its own `device` block; `config apply` keeps its
  own journal/snapshot/`rollback` instead.
- `kb916 light stream`/`light music` (live sessions) and
  `kb916 screen plan` (dry-run only, no real write path) are deliberately
  absent — per `docs/cli-flag-inventory.md`'s REVIEW disposition, they
  don't represent persisted device state.
- The future HE `hk` section (§6/D6) — name and per-mode shapes decided
  later; `advanced_keys` stays khub916-only.

## 10. Open questions (not resolved by this draft)

1. HE v2 macros need a `format` discriminator before they can share the
   `macros` section (§5) — defer until an HE macro write path exists.
2. Whether `macros` ever gains `replace` **or** `delete` — both are
   blocked on the same proof: `build_macro_region` repacking must be
   byte-faithful for entries the parser cannot fully name (`StepKind::Other`
   steps don't survive parse→re-encode), per §Safety 7/8. `delete` is
   additionally blocked on deciding what happens to keymap slots still
   bound to the deleted index (§5).
3. HE `hk` section shape — separate section name (`hk`) chosen by D6, but
   per-mode field details (`--kcs`/`--trps`/`--dbs`/`--time`/`--delay`/
   `--row2`/`--col2` across 7 modes) need their own pass.
4. Mouse `patch` mode — schema defines it, but the apply engine needs a
   read-merge path (`mouse_state().config` → merge → `mouse_config_from_json`
   shape) before patch applies can be honest. Until then layer 2 rejects
   `mode: "patch"` for `compx` lighting/settings.
5. Whether `omg plan` wraps or is superseded by this envelope (§9).
