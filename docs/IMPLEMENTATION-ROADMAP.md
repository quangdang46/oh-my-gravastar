# Implementation Roadmap

Maps `docs/vision-full-custom.md` §8 phases to beads. Existing bead IDs are
used where they match; **proposed new beads** are listed per phase (create
with `br create` before starting work — they are not yet in the tracker).
Ordering is P0-first; dependencies noted as `→`.

Status snapshot: only wired khub916 (K1 Pro) is live-verified. HE v2,
Compx, SparkPlayJoy, wireless are code-tested at best — every phase that
touches them stays behind the capability registry so unverified features
are gated, never faked.

## Phase 0 — Safety & Architecture (P0/P1)

Foundation everything else stands on. No user-visible features except the
modal swap and validation hardening.

| Bead | Title | Deps | Notes |
|---|---|---|---|
| `zlb` | capability registry + device model + feature gating | — | `web/src/core/*`; export `getCapabilities(device)` + tests; page gating follow-up |
| `sid` | validation + policy + snapshots + transaction + journal | `zlb` | `web/src/safety/*`; L0–L4 per SAFETY-SPEC §3 |
| `sfa` | styled confirm/preview modal replacing native dialogs | — | can land before `sid`; modal shell reused by transaction preview later |
| `y6s` | harden profile import validation | `sid` (or minimal local validators first) | fixes §4.2 R1 |
| *(new)* `snp-full` | multi-section restore: journal + partial-failure rollback | `sid` | §4.2 R2 |
| *(new)* `light-abort` | lighting multi-write stop-on-failure + partial report | `sid` | §4.2 R4 |
| *(new)* `reconnect-ux` | show reconnected device, drop queued writes | — | §4.2 R6 |

Exit criteria: every web write flows through transaction executor;
import validation rejects bad profiles before planning; `cargo test` +
`npm build` clean.

## Phase 1 — Core Customization (P0/P1)

| Bead | Title | Deps | Notes |
|---|---|---|---|
| `c8y` | keymap studio: layer manager + undo/redo + keycode search | `sid` (snapshots for undo) | remap.ts/keypool.ts only; no new protocol writes |
| `uol` | macro studio: timeline + validator | — | UI+logic on existing codec |
| `nha` | lighting studio: named scenes | — | localStorage scenes |
| `th0` | profiles: version history + diff + auto-snapshot | `sid` | depends on snapshots.ts |
| `8r8` | music-reactive lighting web page | — | **in progress**; port `crates/music` DSP to TS, cmd-8 frames |
| *(new)* `keymap-diff` | system-level change preview/diff before Apply | `sid` | vision "top UX problem #2" |
| *(new)* `layout-editor` | physical/visual/mapping separation + layout JSON | `c8y` | vision problem #3; I-section items |
| *(new)* `khub916-profile-import` | whole-profile import mapping for khub916 | `y6s`, protocol work | currently export-only (README) |

## Phase 2 — Advanced Behaviors (P1)

| Bead | Title | Deps | Notes |
|---|---|---|---|
| *(new)* `combo-engine` | combo/shortcut detection spec + feasibility | protocol research first | no codec support exists — start with docs/protocol evidence, may be `[missing]`-forever for khub916 |
| *(new)* `behavior-editor` | per-key "Configure key" easy/advanced flow | `c8y`, capability flags | covers C-section; MT/SOCD/TGL reuse `advkeys.ts` |
| *(new)* `advkey-verify` | resolve cmd-18 persistence on K1 Pro | hardware session | decides `verified` flag (risk R5) |

## Phase 3 — HE Performance (P1)

All gated by capability registry; **blocked-by-hardware** for verification.

| Bead | Title | Deps | Notes |
|---|---|---|---|
| `xp0` | HE performance studio web UI | `zlb`, HE WebHID transport (`0xFFB0` in hid.ts) | actuation/RT/deadzone/DKS/MPT, travel monitor, calibration wizard |
| *(new)* `he-web-transport` | WebHID feature-report transport for HE v2 | — | prerequisite for `xp0`; mirror `crates/proto-he` |
| *(new)* `sparkplayjoy-feed` | 6×21 travel → web/TUI heatmap | hardware | roadmap item in README |
| *(new)* `he-hw-verify` | HE keyboard hardware verification pass | hardware | unblocks all 🟡 → ✅ |

## Phase 4 — Agent-first (P1/P2)

| Bead | Title | Deps | Notes |
|---|---|---|---|
| `ak2` | action schema + dry-run plan/execute API | `zlb`, `sid` | `web/src/agent/*`; ops per AGENT-ARCHITECTURE §3 |
| *(new)* `agent-validator` | validator: schema+capability+policy+conflicts | `ak2`, `sid` | planner may emit, validator decides |
| `7js` | NL→plan assistant + "Explain my keyboard" | `ak2`, `zlb` | rule-based first, P4 |
| *(new)* `simulator` | config simulator (preview effects without write) | `ak2` | differentiator; agent foundation |
| `oe4` | `docs/agent-api.md` (op vocabulary, lifecycle, policy, examples) | `ak2` | **done** — schema v1 documented from `action-schema.ts`; MCP tool manifest still pending |
| *(new)* `desired-state-config` | schema-first desired-state JSON config (Rust `omg config` + Web Custom config) | `sid`, `ak2` | **partial** — Draft 2020-12 artifact shared by python/Rust/AJV; apply lowered for compx settings/lighting replace + khub916 keymap/macros/sleep (CLI) and khub916 subset (web, via safety tx); snapshot+journal+`config rollback` shipped; he-v2/lighting/advkeys/patch-mode still `not_applied` (`docs/device-config-schema.md`) |

## Phase 5 — Advanced Product (P2)

| Bead | Title | Deps | Notes |
|---|---|---|---|
| *(new)* `game-profiles` | per-game/app/OS profile switching | `th0` | needs OS-level trigger story (web limits) |
| *(new)* `preset-library` | template/community presets | `nha` scenes | local first; marketplace later |
| *(new)* `multi-device` | multi-device plans + per-device context | `ak2`, `zlb` | plan schema already has device_selector |
| *(new)* `extended-diag` | latency/matrix/chatter/rollover tests | `oq2` + protocol research | mostly `[missing]` today |
| *(new)* `cloud-sync` | opt-in sync | explicit opt-in design + privacy review | deferred; offline-first stands |
| *(new)* `wireless-verify` | khub916_wireLess live verification | hardware (dongle) | transport code done (`wireless.rs`) |
| *(new)* `k98-screen` | K98 screen GIF editor | K98 Pro hardware + `protocol-khub916-screen.md` | info/plan stubbed only |
| *(new)* `mouse-hw-verify` | Compx mouse hardware pass | hardware | all mouse features 🟡 today |

## Cross-cutting (any phase)

| Bead | Title | Notes |
|---|---|---|
| `utk` | dashboard | P3; can pull earlier if UX review favors it |
| `oq2` | diagnostics page | P3; L0 read-only only |
| *(new)* `tui-parity` | TUI feature parity | README roadmap; `crates/tui` exists |
| *(new)* `state-split` | `state.ts` → device/draft/execution | vision §9 refactor; do alongside `sid` |

## Dependency graph (critical path)

```
zlb ──→ sid ──→ y6s ──→ th0 ──→ game-profiles
 │        │
 │        ├──→ c8y ──→ behavior-editor ──→ combo-engine(research)
 │        ├──→ light-abort, snp-full, reconnect-ux (risk fixes)
 │        └──→ ak2 ──→ agent-validator ──→ 7js ──→ simulator
sfa ─────┘ (independent, do early — pure UI)
8r8, nha, uol ── independent feature beads (anytime Phase 1)
xp0 ← he-web-transport ← zlb; verification blocked on hardware
```

## Ordering rules

1. `sfa` + `zlb` + `sid` first — everything else consumes them.
2. Risk fixes R1–R6 (`y6s`, `snp-full`, `light-abort`, `reconnect-ux`,
   `advkey-verify`) before or alongside Phase 1 feature work — they are
   P1 safety debt, not optional polish.
3. Never gate shipped khub916 features behind unverified HE/mouse paths;
   capability registry hides, not blocks.
4. Hardware-blocked items (`xp0` verify, `he-hw-verify`, `mouse-hw-verify`,
   `wireless-verify`, `k98-screen`, `sparkplayjoy-feed`) stay code-tested
   and flagged until a device session happens.
