# oh-my-gravastar

![Demo: music-reactive keyboard lighting (MVP)](docs/demo-music-lighting.webp)

*Demo: per-key RGB lighting driven by live music (`omg kb916 light music`) — keys flash on the beat and go dark between hits.*

Agent-first, offline CLI (`omg`) for configuring GravaStar keyboards and mice.
Every command supports `--json` so AI agents can drive it. No daemon, no
network, no telemetry.

> Not affiliated with GravaStar. "GravaStar" is a trademark of its owner.
> This project reimplements the wire protocols of the vendor's official
> configurators from publicly observable behaviour; it does not redistribute
> vendor software, firmware, or artwork.

## Feature matrix

| Device family | Protocol | CLI | Status |
| --- | --- | --- | --- |
| Mercury K1 / K1 Pro / K1 Lite / K98 Pro / NP series (wired USB) | khub916 feature reports | `omg kb916` | Implemented: reads plus verified writes (keymap, profile fields, light colours) — live-verified on a K1 Pro ([docs/protocol-khub916.md](docs/protocol-khub916.md)). Screen info/plan and wireless transport are separate |
| K1/K98 over 2.4 GHz dongle | khub916_wireLess (19-byte reports) | — | Code-verified only, no hardware yet (P2) |
| Mercury HE keyboards (V60 / V75 / V75 Pro / V75 Lite …) | HE "v2" 64-byte reports, usage page `0xFFB0` | `omg kb` | Implemented: info, layout, keys, live travel, profiles, polling, OS mode, per-key performance, lighting, calibration ([docs/protocol-he-v2.md](docs/protocol-he-v2.md)) |
| HE keyboards — SparkPlayJoy 6x21 analog | `0xFFA0:0001` framed reports | `omg kb travel-matrix` | Implemented read-only: identity proof + live 6×21 travel matrix in µm, `--watch` streams ~10 Hz ([docs/protocol-sparkplayjoy.md](docs/protocol-sparkplayjoy.md)); hardware verification pending on a V75 board |
| Mercury mice M1 / M2 / X / X Air / X Pro (Compx chipset) | `0xFF02` report `0x08`, 16-byte payloads | `omg mouse` | Implemented: info, battery, firmware, full config dump, settings writes ([docs/protocol-compx-mouse.md](docs/protocol-compx-mouse.md)) |

Scope, milestones and the safety contract live in [docs/plan.md](docs/plan.md);
work is tracked as beads (`br list`).

## Feature checklist

Status: ✅ live-verified on real hardware · 🧪 code + unit-tested, hardware
unverified · ⬜ not implemented

### khub916 — K1 / K98 wired USB (K1 Pro live-verified)

| Feature | CLI | Web UI | Status |
| --- | --- | --- | --- |
| Device detect / info (model, UUID, firmware) | `omg devices`, `kb916 info` | auto-connect + device page | ✅ |
| Keymap read (4 layers, 126 keys → labels) | `kb916 keys` | visualizer + keys page | ✅ |
| Key remap (single / full table) | `kb916 set key\|keymap` | keys page (217-key pool) | ✅ |
| Lighting modes + speed/brightness | `kb916 set light-mode`, `light apply` | lighting page (18 modes, HSV picker) | ✅ |
| Custom per-key RGB (126 slots) | `kb916 set light-color`, `light apply` | lighting page + presets | ✅ |
| JSON-driven lighting (`light apply file.json`) | ✅ solid/gradient/rainbow | ✅ same schema in UI | ✅ |
| Light-data blob decode (cmd 138, 23 groups) | `kb916 light get --json` | lighting page live state | ✅ |
| Macro region read/write/bind/delete | `kb916 macro …` | macro page (step editor) | ✅ |
| Profile fields (sleep, light mode) | `kb916 set sleep`, `profile get` | setting page | ✅ |
| Power / battery | `kb916 power` | device page | ✅ |
| Advanced keys MT / SOCD / TGL | `kb916 adv …` | advanced page | 🧪 keymap binding ✅, history blob (cmd 18) does not persist on K1 Pro |
| Fn-layer diagnostic ("Fn+X does nothing") | `kb916 doctor fn` | Fn doctor card | 🧪 read-only; K1 Pro names, other models slot indices |
| Profile export (schema v1 JSON) | `omg profile export` | config page export | ✅ |
| Profile import / restore | `omg profile import` | config page import | 🧪 CLI he-v2/compx only; khub916 export-only |
| K98 Pro screen info/plan | `kb916 screen info\|plan` | — | 🧪 push/clock stubbed (no read-back) |
| GIF light frames (cmd 8) — live RGB stream + music-reactive | `kb916 light stream`, `kb916 light music` | — | ✅ live-verified on K1 Pro (LEDs animate; auto-restore) |
| 2.4 GHz wireless dongle | — | — | 🧪 transport code done (`wireless.rs`), no hardware |

### HE keyboards — V60 / V75 / V75 Pro / V75 Lite (no hardware yet)

| Feature | CLI | Status |
| --- | --- | --- |
| Info / layout / keycodes | `kb info`, `kb layout`, `kb keys` | 🧪 |
| Per-key actuation + Rapid Trigger + deadzones | `kb perf get/set` | 🧪 |
| Lighting (16 effects, palette, 3 zones) | `kb light`, `kb lighting-areas` | 🧪 |
| Polling rate / profiles / OS mode | `kb polling`, `kb profile`, `kb os` | 🧪 |
| Advanced keys (DKS/MPT/MT/TGL/END/SOCD/RS) | `kb hk get/set` | 🧪 |
| Macros (paged actions) | `kb macro list/get/set` | 🧪 |
| Live analog travel 6×21 (SparkPlayJoy) | `kb travel-matrix --watch` | 🧪 read-only |
| Calibration | `kb calibration` | 🧪 |

### Compx mice — M1 / M2 / X / X Pro (no hardware yet)

| Feature | CLI | Status |
| --- | --- | --- |
| Info / battery / firmware / full config | `omg mouse info\|battery\|fw\|config` | 🧪 |
| DPI stages, buttons, lighting, polling, macros | `omg mouse set …` | 🧪 |

### Web UI (`web/`, Vite + WebHID PWA)

| Feature | Status |
| --- | --- |
| Offline PWA, vendor-clone shell + keyboard visualizer | ✅ |
| Silent auto-reconnect (WebHID permission persists) | ✅ |
| Lighting / Keys / Macro / Advanced / Device / Config pages | ✅ |
| Non-tech UX pass (plain language, guided flows) | 🚧 in progress |
| Writes in browser (confirm + read-back verify) | ✅ |

### Roadmap

Platform direction: **Full Custom + Agent-first + Safety-first** — see
[docs/vision-full-custom.md](docs/vision-full-custom.md) for the audit and
[docs/IMPLEMENTATION-ROADMAP.md](docs/IMPLEMENTATION-ROADMAP.md) (in progress)
for the phased plan. Active work is tracked in beads (`br list`).

- ✅ Agent config: schema-first desired-state CLI + Web guided apply
  (`omg config schema|validate|apply|rollback`, shared
  `schemas/device-config.schema.json` across Rust/Python/Web)
- 🚧 Capability registry + safety/transaction engine + spec docs (Phase 0)
- 🚧 Web UI: styled modals, profile history/diff, music-reactive page
- ⬜ Keymap/Macro/Lighting Studio upgrades (Phase 1)
- ⬜ HE keyboard hardware verification pass (needs a V75/V60 board)
- ⬜ Mouse hardware verification pass (needs a Mercury mouse)
- ⬜ SparkPlayJoy analog → visualizer/TUI feed (travel heatmap)
- ⬜ khub916 wireless dongle live verification
- ⬜ K98 Pro screen GIF editor (needs K98 Pro hardware)
- ⬜ khub916 whole-profile import mapping (export-only today)
- ⬜ TUI (`omg-tui`) feature parity pass
- ⬜ macOS `.app` bundle / Linux package for `omg`

## Install

```sh
cargo install --path crates/omg     # installs `omg` into ~/.cargo/bin
# or
cargo build --release                # binary at target/release/omg
```

Requires Rust 1.85+. Uses the system HID stack through `hidapi` — see
[docs/platforms.md](docs/platforms.md) for per-OS notes (macOS permissions,
Linux udev rules, Windows).

## Quickstart

```sh
omg devices                 # list detected GravaStar devices (all HID interfaces)
omg devices --json          # machine-readable
omg --version
```

`--device <selector>` picks one device when several are connected: a HID
path, `VID:PID` (hex, `0x` prefix optional), or a unique case-insensitive
substring of the product/manufacturer string. With exactly one matching
device it is auto-selected.

### Agent configuration surface

The Agent workflow is schema-driven desired state — see
[docs/device-config-schema.md](docs/device-config-schema.md). The shared
Draft 2020-12 artifact (`schemas/device-config.schema.json`) is consumed
identically by the CLI, `schemas/test_schema.py`, and the Web PWA (AJV):

```sh
omg config schema                    # print the desired-state JSON Schema
omg config validate dev.json         # offline structural validation (all errors + JSON paths)
omg config apply dev.json            # device match + per-section plan + plan_hash, no writes
omg config apply dev.json --yes --approve <hash>  # apply; approval bound to the dry-run hash
omg config rollback <id> --yes       # restore the pre-apply snapshot
```

`omg config` implements the desired-state envelope in
`docs/device-config-schema.md` (`schemas/device-config.schema.json`).
Apply coverage is intentionally partial — unsupported sections are
reported `not_applied` with a reason, never silently skipped. Every
apply persists a rollback snapshot before the first write; `config
rollback` verifies device identity (protocol + VID + PID + usage page)
before restoring. `omg cli-schema <cmd>` remains the machine-readable
*command* contract, and the command-plan format (`omg plan`,
[docs/plan-json.md](docs/plan-json.md)) remains the compatibility path
for multi-step CLI automation.

HE keyboard (needs an HE v2 board such as a Mercury V75):

```sh
omg kb info --json                    # firmware, board id, build time
omg kb layout                         # physical geometry + layer-0 key labels
omg kb keys get --row 1               # raw keycodes of layer 0, matrix row 1
omg kb keys get --row 1 --col 2       # single key (03 03)
omg kb keys set 1 2 0x1E --yes        # write one keycode (03 04, read-back verified)
omg kb keys set-row 1 <21 codes> --yes  # whole row (03 02)
omg kb keys default 1 2 3             # factory-default row (03 06)
omg kb hk get 1 2 --mode tgl          # advanced-key config (hardware-unverified)
omg kb hk set 1 2 --mode socd --kcs 30,31 --delay 100 --row2 1 --col2 3 --yes
omg kb macro list                     # macro slots + storage info
omg kb macro get 3                    # macro 3 header + action pages
omg kb macro set 3 --yes 10:0x1E 20:0x1E:up 30:0x04  # delay:keycode[:up] actions
omg kb live 1                         # live per-key travel (mm) + actuation
omg kb lighting-areas                 # list lighting zones
omg kb light get 0                    # zone 0 settings + 8-colour palette
omg kb perf get 1 2                   # performance settings of matrix row 1, col 2
omg kb polling get                    # supported + active polling rates
```

K1 / K98 series (khub916 wired protocol):

```sh
omg kb916 info                        # UUID, catalog model, firmware, screen
omg kb916 keys                        # full keymap decoded to labels (126 entries)
omg kb916 keys --layer 1 --system 1   # other layer/system read argument
omg kb916 profile get                 # light mode, sleep seconds, raw 128-byte blob
omg kb916 power                       # battery % and raw status flags
omg kb916 light-colors get            # 126 per-key RGB colours
omg kb916 light-data                  # per-mode light blob (hex dump)
omg kb916 macro                      # macro region (hex dump)
omg kb916 light get --json           # decoded lighting state (mode/speed/colours)
omg kb916 light apply fx.json --yes  # apply a lighting JSON (see docs/lighting-schema.md)
omg kb916 light stream --pattern rainbow --yes   # live cmd-8 RGB frames (Ctrl-C restores)
omg kb916 light music --yes                      # music-reactive: system audio → FFT → LEDs
omg kb916 light music --source mic --yes         # or microphone input
omg kb916 light music --effect pulse --yes       # beat flash | bass | ambient | spectrum | beat (hybrid)
omg kb916 macro list|get|set|bind    # named macros + tag-3 key binds (docs/macro-schema.md)
omg kb916 adv list|get|set          # MT/SOCD/TGL advanced keys (docs/advkeys-schema.md)
omg kb916 doctor fn                  # read-only Fn-layer check: dead slots, opaque tags, Fn-key binding
```

Writes require `--yes` and are verified by read-back (live-verified on a
K1 Pro):

```sh
omg kb916 set key 100 0x00000004 --yes          # one key slot (read-modify-write)
omg kb916 set keymap keys.txt --yes             # full table, one u32 per token
omg kb916 set sleep 30 --yes                    # sleep timeout (profile byte 24)
omg kb916 set light-mode 15 4 0 --yes           # light mode/speed/brightness
omg kb916 set light-color 0 '#FF0000' --yes     # one custom-light slot
```

Screen commands (`omg kb916 screen …` is info/plan only — actual screen
writes have no read-back path) and the 916_wireLess transport return
"not implemented" (exit code 3) — see docs/protocol-khub916.md §8 and
docs/protocol-khub916-screen.md.

SparkPlayJoy analog interface (V75 / V75 Pro / V75 Lite boards expose a
second HID interface on usage page `0xFFA0`):

```sh
omg kb travel-matrix --json           # prove identity, emit live 6x21 travel (µm)
omg kb travel-matrix --watch          # stream a sample every ~100 ms until Ctrl-C
```

Writes are opt-in and always read back:

```sh
omg kb perf set 1 2 --mode fixed --actuation 1.5 --yes --json
omg kb light set 0 --mode 12 --brightness 80 --yes
omg kb polling set 1k --yes           # keyboard restarts afterwards
omg kb os set macos --yes
```

Compx mouse (Mercury M/X series, wired or 2.4 GHz receiver):

```sh
omg mouse info --json                 # model, link type, sensor, max polling
omg mouse battery
omg mouse fw
omg mouse config --json               # full decoded settings flash map
omg mouse set polling 1000 --yes
omg mouse set dpi 0 1600 --yes
omg mouse set light mode 3 --yes
omg mouse set button 4 mouse back --yes
```

Diagnostics:

```sh
omg probe                             # per-protocol read-only summary of every detected device
omg probe --deep                      # adds the SparkPlayJoy travel-matrix read
omg probe --device <selector>         # probe one device
```

### Profile export / import

`omg profile` moves settings between machines as a portable JSON document
(schema v1): identity plus every readable setting — HE keyboards get
info/profiles/polling/OS/lighting/palette/keymap/per-key performance, mice
the full decoded config, khub916 the keymap/profile/power/light state,
SparkPlayJoy the sync/scale/maps (read-only).

```sh
omg profile export backup.json         # all connected GravaStar devices
omg profile export backup.json --full  # + khub916 light-data/macro hex blobs
omg profile import backup.json --dry-run   # print planned changes, write nothing
omg profile import backup.json --yes       # back up, then apply + verify
```

Import matches devices by protocol + VID/PID + usage page, snapshots the
current state to `~/.local/share/oh-my-gravastar/backups/<utc-ts>/` before
writing, and verifies every change by read-back. khub916 and SparkPlayJoy
are export-only for now (khub916 has verified `kb916 set` writes but no
whole-profile import mapping; SparkPlayJoy has no write path).

`omg profile watch` follows the foreground app and can auto-switch the
HE keyboard profile (macOS only for now — Linux/Windows detection is
documented future work):

```sh
omg profile watch --rules rules.json            # report app→profile matches
omg profile watch --rules rules.json --apply --yes   # switch on match
```

`rules.json` is `{"rules":[{"app"|"bundle": "substring", "profile": N}]}`
— first match wins, `profile` is the 0-based HE profile index. `--apply`
opens the HE keyboard once, range-checks every switch and verifies by
read-back; khub916 has no profile-switch command so apply is HE-only.

### JSON output

Every data-emitting command accepts `--json` (global flag, any position):

```json
$ omg devices --json
[
  {
    "path": "DevSrvsID:4295306107",
    "vid": 9610,
    "pid": 268,
    "product": "Gaming Keyboard",
    "manufacturer": "BY Tech",
    "usage_page": 65280,
    "usage": 1,
    "kind": "khub916_wired"
  }
]
```

(VID/PID are decimal in JSON; `omg devices` prints them as `VID:PID` hex.)

## Safety rules

`omg` is **read-only by default**. The rules below are enforced in code, not
just documented (see `docs/plan.md` §2 and `AGENTS.md`):

1. Every write command requires `--yes` and reports the exact change first;
   without `--yes` nothing is sent (`error: refusing to write without --yes`).
2. Every mutation is followed by a read-back; the result carries
   `verified: true/false` (polling-rate changes may legitimately report
   unverified because the keyboard restarts and drops off the bus).
3. Protocol-layer blacklists are never implemented: HE factory reset
   (`02 01`), firmware update (`08 xx`), settings wipe (`0A 01 01 FF`),
   gamepad mode (`10 02`), restart (`02 0F 01`), default-switch (`02 09`),
   categories `0x0C/0x0E/0x0F`; mouse pairing (`05/06`), factory reset (`09`),
   RF channel (`31`), bootloader range (`B0`–`B7`); khub916 commands stay
   unimplemented until proven read-only.
4. Range validation before send: actuation ≤ switch max travel, deadzone
   ≤ 1 mm, palette indices 0–7, mouse writes restricted to the known
   `WRITABLE` flash ranges.
5. Serial numbers and other device identifiers are not printed in normal
   output (plan.md reserves an explicit `--include-serial` opt-in for the
   day a command needs them).

## Platforms

| OS | HID backend | Notes |
| --- | --- | --- |
| macOS | hidapi / IOHIDManager | May need Input Monitoring permission for the terminal/app; see [docs/platforms.md](docs/platforms.md) |
| Linux | hidraw | udev `uaccess` rule recommended for non-root access ([docs/platforms.md](docs/platforms.md)) |
| Windows | HIDClass via hidapi | No driver needed |

## Development

```sh
cargo build            # debug build
cargo clippy           # workspace lints (clippy::all = warn)
cargo test             # unit tests incl. codec tests with mock Link
```

Workspace layout and contribution rules: [AGENTS.md](AGENTS.md).
Protocol specifications: [docs/](docs/). Design and milestone plan:
[docs/plan.md](docs/plan.md). Issue tracker (beads): `br list`, TUI `bv`.
