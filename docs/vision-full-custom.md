# oh-my-gravastar → Full Custom + Agent-first + Safety-first

Audit & product vision (user-provided, 2026-11-24). This is the strategic
direction document. Implementation is tracked as beads; protocol facts live
in `docs/protocol-*.md`, current shipped state in `README.md` checklist.

## Goal

Not a HubX clone — a **Keyboard Customization Platform**:

- Non-technical users customize everything in natural language.
- Power users go deep: keymap, lighting, macro, analog, performance,
  firmware-exposed config.
- AI agents read state, understand device capabilities, plan changes,
  preview, apply, verify.
- Every change undoable/restorable within hardware limits.
- No dangerous commands executed because an agent misunderstood.

## Audit findings (current state)

Present: DOM keyboard visualizer, key remap (4 layers, key pool, staged
edits, dup warning), lighting (18 modes, HSV picker, per-key RGB, presets,
JSON), macro CRUD + bind, advanced keys SOCD/MT/TGL, device info/battery,
JSON backup/import, sleep timer, music-reactive (`kb916 light music`,
cmd-8 stream — live-verified on K1 Pro), CLI `omg` all protocols, TUI.

### Top UX problems to fix first

1. **No native `window.confirm()`/`window.prompt()`** — replace with styled
   modal showing: what will be written, which keys affected, overwrite
   warning, recoverability, clear Cancel/Confirm.
2. **No system-level change preview/diff** — agent must produce a diff
   (Layer0: Caps→Ctrl; Layer1: WASD→arrows; Fn→layer) before Apply.
3. **Layout editor not real** — separate physical layout (hardware truth)
   from visual layout (display only) from key mapping (function).
4. **No profile/history system** — need auto-snapshot before every write,
   version history, compare, partial restore, rollback, presets,
   per-game/app profiles.
5. **No capability-aware UI** — features must show state: supported /
   unsupported-by-hardware / unsupported-by-firmware / protocol-unknown /
   experimental / unverified. K1 Pro must NOT show HE features
   (actuation, RT, DKS/MPT, live travel, calibration).

## Feature inventory (checklists — track in beads/docs)

### A. Key remapping & keycodes (P0, 51 items)

- A1 basic mapping (10): remap any key, unassign, swap, copy/paste,
  multi-select, drag&drop, undo/redo, reset key, reset layer, search by
  name+HID.
- A2 extended keycodes (15): F1–F24, nav, arrows, numpad, L/R modifiers,
  media, volume/mute, brightness, browser controls, system power,
  consumer HID, mouse buttons/wheel, intl keys, custom firmware codes.
- A3 modifier combos (10): Ctrl/Alt/Shift+key presets, GUI combos,
  3–4 modifiers, Hyper/Meh, custom shortcut editor, suppression,
  negative conditions.
- A4 key behavior (16): tap/hold, tap-hold threshold, double/triple tap,
  tap dance, long press, sticky modifier, one-shot modifier/layer,
  key repeat, Caps Word, Grave Escape, Mod-Morph, Key Override.

### B. Layers (P0, 20 items)

Create/rename/delete/clone/copy/import/export layers (firmware permitting),
visualize all, Fn switching, MO/TG/TO/LT, one-shot/sticky/conditional
layers, OS/profile-conditional, activation indicator, priority +
transparent key, conflict detector. NOTE: cannot create layers in UI if
firmware doesn't support — capability flag required.

### C. Advanced behaviors (P1, 14)

Mod-Tap, Layer-Tap, Tap Dance, Combo, Key Override, One-Shot, Sticky,
Caps Word, Auto Shift, Leader Key, Repeat Key, Mod-Morph, Conditional
Layer, Tap-Hold, Macro Behavior.

UI pattern: per-key "Configure key" flow — Easy mode (one action /
tap=hold) vs Advanced mode (timing, interruption, conditions).

### D. Combo & shortcut engine (P1)

Combos of 2–4 keys, timeout, layer/modifier conditions, output = keycode /
macro / layer, conflict detection, visual key display, simulator.

### E. Macro Studio (P0/P1, 28 items)

Record from keyboard input / host keys / mouse, down/up, modifiers,
delays (manual + auto), text sequences, mouse move/wheel (if firmware),
repeat N/∞, nested, layer/combo triggers, library, templates,
import/export, duplicate, timeline preview, simulator, validator
(missing release, stuck modifier, overlong delay, storage overflow,
unsupported keycode), storage usage, undo/redo steps, search/replace,
batch bind.

### F. Lighting Studio (P0/P1)

- F1 color (15): per-key, global, palette, position/row/col gradients,
  rainbow, random, HSV/RGB/HEX picker, eyedropper, copy/paste, history,
  favorites, presets, export/import.
- F2 effects (18): static, breathing, wave, ripple, reactive typing,
  rain, rainbow, spectrum, snake, spiral, meteor, fire, aurora, custom
  animation, speed, direction, brightness, intensity.
- F3 advanced: per-layer/state/CapsLock/game/profile/time/battery,
  zones, per-key timeline, music/sound reactive.
- Scene editor: named scenes (Gaming Red, Cyberpunk…) = color+effect
  config, not just RGB lists.

### G. Analog / HE / Rapid Trigger (HE devices only)

Actuation, RT, per-key actuation/RT, press/release sensitivity,
top/bottom deadzone, DKS, MPT, Mod-Tap, SOCD, snap-tap/LIP, analog
output, travel monitor, calibration, travel viz, per-key perf presets.

### H. Device config (P1/P2)

Polling, link status, OS mode, battery/charging, sleep, power saving,
debounce, NKRO/6KRO, key repeat, calibration, switch/firmware/hw rev
info, diagnostics, latency, matrix/chatter/response/rollover tests,
connection quality. NOTE: polling may restart device; calibration changes
hardware behavior — separate confirmations.

### I. Layout editor (P1, 17 items)

Physical layout select, ANSI/ISO/JIS, model, position/size/gap/rotation,
hidden+virtual keys, custom legends, multi-layer view, pressed/selected
highlight, HID usage display, layout JSON import/export, validation,
theme. Physical vs visual vs mapping separation mandatory.

### J. Profile/preset management (P0/P1, 24 items)

Multi-profile CRUD, name/duplicate/delete/switch, per game/app/OS/user/
work, full+section backup/restore, compare, version history, auto
backup before write, import/export, compatibility validation, migration,
opt-in cloud sync, template library.

## Agent-first architecture

Pipeline: device understanding → plan generation → validation &
simulation → preview & approval → execute → verify & recover.

Agent checklist (62 items) categories: device intelligence (10),
natural-language customization (10), planning (10), execution (10),
learning/assistance (10), tools & interoperability (12).

Key scenarios: coding layout, FPS profile, navigation layer, WASD-only
lighting, Caps→Ctrl, factory restore, RT tuning, unused-key analysis,
device diagnostics. **"Explain my keyboard"** — list what THIS device
actually supports (capability registry output, not theory).

## Safety architecture (5 layers)

1. Input validation (types, ranges, sizes, enums, schema)
2. Capability & compatibility (model, firmware, protocol, state)
3. Policy engine — operation classes:
   - L0 read-only → automatic
   - L1 draft/preview → automatic
   - L2 remap/lighting/macro → approval before write
   - L3 polling/calibration → separate confirmation + impact explanation
   - L4 factory reset/firmware/bootloader → never for agent (blacklist stays)
4. Transaction executor — backup → preview → approval → write →
   read-back verify
5. Recovery & audit — rollback, journal, history, diagnostics

Known risks to fix (§4.2):
- Profile import validation gaps (model compat, u32 bounds, color index
  dup checks, size limits).
- Multi-section restore partial failure → journal + rollback plan.
- Macro region full-512B rewrite → snapshot + diff before send.
- Lighting multi-write continues after failure → stop + report partial.
- Adv-key cmd-18 history unverified on K1 Pro → capability flag,
  not "stable".
- Auto-reconnect UX → show which device, easy disconnect, no silent writes.

Recovery types (undo ≠ reset): undo, redo, discard draft, restore point,
factory defaults (only via proven-safe path; K1 Pro restore = write back
old values, not zero-fill).

## Capability matrix (initial)

| Feature | K1/K1 Pro (916) | HE v2 |
|---|---|---|
| Basic remap | ✅ | ✅ |
| Key layers | 4 known | firmware-dependent |
| Macro | ✅ wired | ⚠️ some writes unverified |
| Per-key RGB | ✅ | ✅ |
| Lighting effects | ✅ by mode | ✅ by zone |
| SOCD/MT/TGL | codec+UI; history write unverified | implemented |
| Tap Dance/Combos/Overrides | unconfirmed | needs firmware check |
| Actuation/RT/DKS/MPT/travel/calibration | ❌ mechanical | ✅ implemented |
| Polling | unconfirmed | ✅ |
| Profiles | backup/import base | ✅ |

Architecture decision: `Keyboard Core → Capability Registry → Feature
Modules → Device Protocol Adapter → Hardware`. One UI, capability-driven.

```json
{
  "device": "Mercury K1 Pro", "protocol": "khub916",
  "capabilities": {
    "keymap": {"supported": true, "layers": 4, "verified": true},
    "macro": {"supported": true, "verified": true},
    "rapid_trigger": {"supported": false, "reason": "mechanical switch"},
    "advanced_keys": {"supported": true, "verified": false,
                      "reason": "history persistence unconfirmed"}
  }
}
```

## Target UI — 7 areas

1. **Dashboard** — device, firmware, battery, profile, quick actions,
   health, recent changes.
2. **Keymap Studio** — visual keyboard, layer manager, keycode picker,
   behavior editor, combo editor, shortcut builder, diff/preview.
3. **Lighting Studio** — scene editor, per-key, effect library, palettes,
   animation editor.
4. **Performance Studio** — actuation/RT/SOCD/DKS-MPT/travel/calibration
   (HE only, capability-gated).
5. **Macro Studio** — recording, timeline, step editor, library,
   validation.
6. **AI Assistant** — NL config, explain device, profile generation,
   troubleshooting, preview+apply.
7. **Profiles & Safety** — backup/restore, history, compare, rollback,
   diagnostics, permissions, activity log.

Two modes, ONE engine: **Simple** (visual, presets, agent-guided, no raw
protocol) and **Expert** (full keycodes, HID usage, behaviors, raw values,
diagnostics, JSON, execution log).

## High-value differentiators

Capability Explorer, Config Diff, Visual Conflict Detector, Macro
Debugger, Key Usage Analytics, Smart Layout Generator, Profile Generator,
Config Simulator, Device Health Monitor, Compatibility Checker, Profile
Migration, Preset Marketplace, Learning Mode, Safe Apply, Recovery Wizard.
Simulator + Safe Apply are agent foundations, not extras.

## Implementation roadmap

- **Phase 0 (P0) Safety & Architecture**: Capability Registry, schema
  validation, device/model compat, pre-write snapshot, unified execution
  engine, read-back verify, execution journal, partial-failure recovery,
  dry-run API, agent action policy.
- **Phase 1 (P0) Core Customization**: keymap editor, layer manager,
  keycode search, Macro Studio, Lighting Studio, profile backup/restore,
  undo/redo, model layouts.
- **Phase 2 (P1) Advanced Behaviors**: Mod-Tap, Layer-Tap, Tap Dance,
  Combo, One-Shot, Sticky, Key Overrides, Conditional Layers, advanced
  macro.
- **Phase 3 (P1) HE Performance**: actuation, RT, DKS/MPT, SOCD/RS,
  travel viz, calibration wizard, matrix diagnostics.
- **Phase 4 (P1) Agent-first**: NL customization, device understanding,
  planning, diff, conflict detection, safe execution, recovery, presets.
- **Phase 5 (P2) Advanced Product**: marketplace, opt-in cloud sync,
  game/app profiles, lighting animation, community presets, multi-device,
  extended diagnostics.

## Repo work items (§9)

`state.ts` → split device/draft/execution state. `hid.ts` → transport +
write policy boundary. `proto/*` → codec only, no UI logic. New modules:

```
web/src/core/        capabilities.ts, feature-registry.ts, device-model.ts, compatibility.ts
web/src/safety/      validation.ts, policy.ts, snapshots.ts, transaction.ts, rollback.ts, audit-log.ts
web/src/agent/       device-context.ts, action-schema.ts, planner.ts, validator.ts, executor.ts
web/src/features/    keymap/ layers/ behaviors/ combos/ macros/ lighting/ performance/ profiles/ diagnostics/
web/src/ui/          components/ layouts/ design-system/
```

Build interfaces + tests first, migrate gradually.

## References

QMK docs (behaviors/combo/overrides), VIA docs (remap UX), Vial manual
(tap dance/combos/macros), ZMK docs + ZMK Studio (behaviors, capability
model), MDN WebHID (permissions/security).

## Top-10 priorities

1. Capability Registry · 2. Safety & Transaction Engine · 3. Keymap
Studio · 4. Profile Manager · 5. Behavior Engine · 6. Macro Studio ·
7. Lighting Studio · 8. HE Performance Studio · 9. Diagnostics ·
10. AI Agent.

Strategic rules: "supported" requires protocol evidence + verification
level; agents never call HID writes directly (same engine as UI); never
pretend to create firmware features the hardware lacks.

## Deliverable docs to produce

`FEATURE-INVENTORY.md` (all checklists), `CAPABILITY-MATRIX.md`
(model×protocol), `SAFETY-SPEC.md` (policy/validation/backup/rollback),
`AGENT-ARCHITECTURE.md` (tool schema + workflow), `UIUX-SPEC.md`
(screens/components/interaction/design system), `IMPLEMENTATION-ROADMAP.md`
(P0/P1/P2 + deps). Start with Capability Registry + Safety Engine +
Feature Inventory.

## Product direction updates (post-audit, user-approved)

**Three interfaces, one core.** UI = visual configurator (deep but
progressive disclosure, no protocol jargon required). CLI = agent
control surface. Shared core = capability registry + validation +
safety policy + transaction executor — no duplicated logic.

**Agent interface = CLI + JSON Schema + JSON payload** (not CLI-only,
not schema-only): `omg cli-schema <cmd>` emits JSON Schema generated from
the same clap definitions that execute; `omg device discover` for
capability discovery; `omg plan create/inspect/validate/apply` for
multi-step workflows with diff/risk/verify. JSON payloads carry complex
configs (lighting scenes, plans); profile JSON stays the storage
format. No monolithic config.json as the sole agent interface.

**Connection lifecycle (P0, bead grd):** auto-reconnect opens a
read-only session; one explicit "Enable editing" unlocks writes per
session; HID connect/disconnect events update UI instantly; last-used
device persisted; never auto-write on reconnect.

**UX model:** every page converges on Edit → Preview → Apply → Verify;
progressive disclosure replaces jargon-first (Normal key / Layer
switch / Tap-or-hold / Toggle / More customization).
