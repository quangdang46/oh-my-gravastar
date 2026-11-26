# Capability Matrix

Model × protocol × feature, with **verification level** per cell.

Verification levels (strongest first):

- **live-verified** — exercised on real hardware with read-back confirmation.
  Only the wired khub916 path on a K1 Pro qualifies today
  (`docs/protocol-khub916.md` §5, 2026-11-24).
- **code-verified** — transport/codec proven against captured traces or
  reversed spec; no device attached.
- **code-tested** — compiles + unit/synthetic tests pass; hardware unverified.
- **unverified** — spec or partial impl exists; semantics uncertain.
- **unsupported-hardware** — feature cannot exist on this hardware
  (mechanical switch vs hall-effect, etc.).

Hardware actually available: **K1 Pro wired only** (`VID 0x258A:0x010C`,
`docs/plan.md` §4). HE keyboards, Compx mice, K98 screen, wireless dongle:
**no hardware** — everything for them is at best code-tested.

## 1. Device families

| Model family | Protocol | Transport | Verification |
|---|---|---|---|
| K1 / K1 Pro / K1 Lite / NP series (wired) | khub916 `916` | HID feature reports, 519 B (`docs/protocol-khub916.md` §2) | **live-verified** on K1 Pro |
| K98 Pro (wired) | khub916 `916` + screen topology A | same + separate screen interface (`docs/protocol-khub916-screen.md` §1) | screen path **code-verified**, no hardware |
| K1/K98 2.4 GHz dongle | khub916_wireLess | HID output reports, 19 B (`docs/protocol-khub916.md` §3, `crates/proto-khub916/src/wireless.rs`) | **code-verified**, no hardware |
| Mercury HE (V60/V75/V75 Pro/V75 Lite…) | HE "v2" | `0xFFB0`, 64 B reports (`docs/protocol-he-v2.md`) | **code-tested** |
| HE + SparkPlayJoy analog | `0xFFA0:0001` framed | 5C-len-cmd-cksum frames (`docs/protocol-sparkplayjoy.md`) | **code-verified** transport; sustained runtime pending (plan.md §1b) |
| Compx mice M1/M2/X/X Air/X Pro | Compx `0xFF02` report `0x08` | 16 B payloads (`docs/protocol-compx-mouse.md`) | **code-tested** |
| Sibling SDK protocols `8815`, `901` (+wireless) | same envelope family | (`docs/protocol-khub916.md` header table) | **unsupported** — out of scope |

## 2. Feature × device matrix

Legend: ✅ live-verified · 🟡 code-tested · 🔵 code-verified only · ⚪ unverified/partial · ❌ unsupported-hardware · — not implemented

| Feature | K1 Pro (916 wired) | K98 screen | khub916 wireless | HE v2 keyboard | SparkPlayJoy analog | Compx mouse |
|---|---|---|---|---|---|---|
| Device detect/info (model, fw, battery) | ✅ `kb916 info/power` | 🟡 | 🔵 | 🟡 `kb info` | 🔵 identity proof | 🟡 `mouse info/battery/fw` |
| Keymap read (layers) | ✅ 4 layers, 126 keys (`kb916 keys`) | 🟡 | 🔵 | 🟡 `kb keys/layout` | 🔵 active map `0x23` | — |
| Keymap write | ✅ cmd 3/4, read-back verified | 🟡 | 🔵 | 🟡 `03 02/04` | ❌ read-only | — |
| Layer count | 4 (fixed, firmware) | 4 | 4 | firmware-dependent | Fn0 map | — |
| Basic macro CRUD + key bind | ✅ cmd 133/5 ≤512 B region | 🟡 | 🔵 | 🟡 `kb macro` paged | — | 🟡 mouse macros (flash) |
| Lighting modes + speed/brightness | ✅ 18 modes | 🟡 | 🔵 | 🟡 `kb light` 16 effects, 3 zones | — | 🟡 `mouse set light` |
| Per-key RGB | ✅ 126 slots, cmd 134/6+138/10 | 🟡 | 🔵 | 🟡 palette+zones | — | — |
| Lighting JSON apply (solid/gradient/rainbow) | ✅ `light apply` | 🟡 | 🔵 | — | — | — |
| Live RGB stream / music-reactive | ✅ cmd 8 + `crates/music`, live-verified | 🟡 | 🔵 | — | — | — |
| Screen info/plan | — | 🟡 push/clock stubbed, no read-back (`docs/protocol-khub916-screen.md`) | ❌ likely absent (§6) | — | — | — |
| Screen GIF editor | — | ⚪ spec'd, not implemented | ❌ | — | — | — |
| Advanced keys MT/SOCD/TGL | ⚪ codec+UI shipped; **cmd-18 history write does not persist on K1 Pro** (`docs/advkeys-schema.md`) | ⚪ | 🔵 | 🟡 `kb hk` DKS/MPT/MT/TGL/SOCD/RS | — | — |
| Polling rate | ⚪ unconfirmed, no command known | — | — | 🟡 `kb polling` (restart → unverified read-back is expected) | — | 🟡 `mouse set polling` |
| OS mode | — | — | — | 🟡 `kb os` | — | — |
| Actuation / Rapid Trigger / deadzones | ❌ mechanical | ❌ | ❌ | 🟡 `kb perf` | — | — |
| Live analog travel | ❌ | ❌ | ❌ | 🟡 `kb live` (04 03 mm) | 🔵 6×21 µm stream | — |
| Calibration | ❌ | ❌ | ❌ | 🟡 `kb calibration` | — | — |
| SOCD/RS snap-tap | ⚪ via advkey only | ⚪ | 🔵 | 🟡 `hk` modes | — | — |
| Sleep timer / power saving | ✅ `set sleep`, `profile get` | 🟡 | 🔵 | 🟡 profile fields | — | 🟡 |
| Profile export (schema v1) | ✅ `omg profile export` | 🟡 | 🔵 | ✅ export | ✅ export (read-only) | ✅ export |
| Profile import / restore | ❌ export-only (no whole-profile mapping) | ❌ | ❌ | 🟡 `profile import` he-v2 | ❌ no write path | 🟡 import |
| Desired-state config apply (`omg config` + Web Custom config) | 🟡 keymap patch/replace, macros+binds, sleep_seconds — pre-write snapshot, per-section journal, `config rollback` with device-identity check (code-tested; no live apply evidence yet) | — (screen iface has own protocol id) | ❌ session gate refuses 916_wireLess | ⚪ schema-valid, apply-rejected (no lowering yet) | ❌ read-only | 🟡 settings/lighting replace |
| Factory reset / firmware update | ⛔ blacklisted | ⛔ | ⛔ (cmd 6 not implemented, `wireless.rs:68`) | ⛔ `02 01/08/0A/10 02/02 0F 01/02 09`, cat `0x0C/0E/0F` | ⛔ no write path | ⛔ `05/06/09/31/B0–B7`, non-`WRITABLE` ranges |

## 3. Capability-registry proposal (consumed by UI + agent)

JSON shape from `docs/vision-full-custom.md` §"Architecture decision" —
proposed `web/src/core/capabilities.ts` output (bead `zlb`):

```json
{
  "device": "Mercury K1 Pro", "protocol": "khub916",
  "verification": "live-verified",
  "capabilities": {
    "keymap":     {"supported": true, "layers": 4, "verified": true},
    "macro":      {"supported": true, "max_bytes": 512, "verified": true},
    "per_key_rgb":{"supported": true, "slots": 126, "verified": true},
    "lighting_modes": {"supported": true, "count": 18, "verified": true},
    "music_reactive":{"supported": true, "verified": true, "cli_only": true},
    "advanced_keys": {"supported": true, "verified": false,
                      "reason": "cmd-18 history write unconfirmed on K1 Pro"},
    "screen":     {"supported": false, "reason": "K98 topology only; not this device"},
    "rapid_trigger":{"supported": false, "reason": "unsupported-hardware: hall-effect only"},
    "actuation":  {"supported": false, "reason": "unsupported-hardware"},
    "calibration":{"supported": false, "reason": "unsupported-hardware"},
    "polling":    {"supported": false, "reason": "protocol-unknown for khub916"},
    "wireless":   {"supported": false, "reason": "wired interface only"}
  }
}
```

Rules:

1. `supported:false` requires a `reason` from the fixed enum:
   `unsupported-hardware | unsupported-firmware | protocol-unknown |
   experimental | unverified | not-implemented`.
2. `verified` reflects the strongest evidence level above — never claim
   live-verified for HE/mouse/sparkplayjoy/wireless.
3. The registry is the single source for UI gating AND agent planning;
   agents must not offer ops whose capability is false.

> **Per-property metadata:** this matrix stays at feature granularity.
> The authoritative per-property
> read/write/verification/persistence/reversibility metadata lives in
> the shared desired-state schema (`schemas/device-config.schema.json`,
> surfaced by `omg config schema`; per-property coverage notes in
> `docs/specs/schema-coverage-and-reset-decision.md` §4.1). When the
> two disagree, the schema wins — update this file to match.
