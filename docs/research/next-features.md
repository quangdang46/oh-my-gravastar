# Next-feature survey — what is feasible next

> **Note (2026-11-26):** written against the earlier config surface
> (`omg schema`/`guide`/`config get|diff|verify`), which was replaced by the
> schema-first `omg config schema|validate|apply|rollback`. Items below are
> historical unless re-verified against `docs/device-config-schema.md`.


Date: 2026-11-25. Status of the agent surface after `config apply`
(a770aad): `omg schema → guide → config get → validate → diff → apply`
is complete for `khub916-k1pro` (the only live-verified writable
family). This doc ranks the remaining candidate features by feasibility
and verification cost. Levels follow AGENTS.md: code-verified /
synthetic-tested / live-verified.

## Status update (2026-11-25, later same day)

- **#1 done** — `config verify` shipped (93c4964); rollback = apply a
  `config get` snapshot (documented in README).
- **#2 done** — web pixel-art shipped: `web/src/proto/pixelart.ts` (port
  of pixelart.rs) + a "Pixel art" card on the Music page (0bb3c7f).
- **#3 done** — `omg kb916 light pixel-art` CLI (45086be).
- **#4 partially** — web `SpectrumViz` already emits the live-verified
  interleaved cmd-8 order; full `mixer.rs` preset parity still open.
- **#8 done (macOS)** — `omg profile watch` polls the frontmost app via
  System Events, matches `--rules` JSON (app/bundle substring → HE
  profile index), `--apply --yes` switches + verifies; report-only mode
  needs no device. Linux/Windows detection documented as future work.

## Tier 1 — no extra hardware needed, K1 Pro in hand

### 1. `omg config verify` + `omg config rollback`

`apply` stops on first failure and reports per-section verify, but there
is no standalone "re-check current vs doc" (`verify`) and no snapshot/
restore for the config surface. `config verify <file>` ≈ `diff` with a
pass/fail verdict (cheap — diff already exists; mostly a wrapper +
exit-code policy). `rollback` needs a snapshot store: `config get`
output IS a valid snapshot — a `omg config apply snapshot.json --yes`
is already a restore. The missing piece is only *discoverability*
(`omg config snapshots` dir convention + docs), not new protocol work.
Effort: small. Value: completes the agent write loop honestly.

### 2. Web pixel-art page (port `crates/music/src/pixelart.rs`)

CLI runtime sessions already drive LED pixel-art (`omg runtime start`
kind `pixel-art`: 5×7 font text, scroll, per-key colours, cmd-8 frames).
The web app has music mode (`web/src/music.ts`) but no pixel-art
surface. Porting needs: 5×7 font table + key-grid mapping
(`grid_geometry`/keymap positions — `web/src/k1pro-layout.ts` already
has physical positions), frame encoder (interleaved RGB — live-verified
byte order), and a small page under Device or its own goal slot.
Effort: medium (~400-600 lines TS + tests). Value: the "agent lights
letters on keys" use case becomes usable without the CLI.

### 3. `omg kb916 light pixel-art` convenience subcommand

`omg runtime start` covers pixel-art via JSON only. A direct
`light pixel-art --text "HI" --scroll` flag would reuse
`crates/music::pixelart` + the existing stream/restore path in
`kb916_light.rs`. Cheap once (2) exists in Rust — it already does; this
is pure CLI wiring (~150 lines). Value: parity with `light music`.

### 4. Music engine presets exposed to web

Web music mode uses its own simplified pipeline (mic-only, basic FFT).
The Rust `mixer` engine has 9 presets + modes shaders. Options:
(a) leave as-is (web = simple, CLI = full — matches spec §12
"Web remains independently usable"), or (b) port `mixer.rs` presets
to `web/src/proto/music.ts` incrementally, starting with `spectrum` +
`pulse`. Effort: medium per preset. Value: visual parity for users
who never install the CLI.

## Tier 2 — needs the other device or deeper protocol work

### 5. `config apply` for `he_keyboard_v2` / `compx_mouse`

Both families are declared code-tested/read-only in the schema
registry. HE has real write paths (`kb set`, profiles, polling, OS
mode) but no hardware has ever verified them — AGENTS.md requires
live verification before claiming writable. Blocked on an HE board.

### 6. K98 Pro screen GIF editor (bead `9he`)

`proto-khub916` already has `screen_begin/frame/push/clock` and the
K98 catalog entries; `omg kb916 screen` exists but push/clock are
stubbed pending read-back. Effort is mostly hardware verification on a
K98 Pro + possibly a second HID interface (screen is a separate
interface per lib.rs notes). Blocked on hardware.

### 7. SparkPlayJoy 6×21 travel feed to web/TUI (bead `urr`)

`omg kb travel-matrix` streams live 6×21 µm travel on HE devices.
Feeding it to web needs a WebHID reader for `0xFFA0:0001` (new protocol
code in TS — `web/src/proto/` has khub916 only) or a localhost bridge
(explicitly against spec §12 "no Rust service"). TUI feed is cheaper:
`crates/tui` exists. Blocked on an HE board for any live check.

### 8. `omg profile watch` — foreground-app profile auto-switch (bead `of1`)

macOS focus monitoring (NSWorkspace notifications) → map app → profile
→ `omg profile` switch. Self-contained Rust, no device protocol risk,
but macOS-only and needs care not to fight manual switches. Effort:
medium. Value: niche convenience.

### 9. Wireless dongle `916_wireLess` (bead `w80`)

Transport code exists (`wireless.rs`), zero hardware verification.
Blocked on the dongle.

## Tier 3 — explicitly out of scope / not recommended now

- **Cloud sync** (bead `0l4`): spec §12 excludes cloud dependencies;
  local-only profile export already covers backup.
- **Config apply for sparkplayjoy**: read-only protocol, nothing to
  write.
- **Daemon/always-on agent**: project is deliberately daemon-free
  (README: "No daemon, no network").
- **`omg config apply` partial/patch semantics**: ADR mandates full
  desired state; do not add patch mode — it reintroduces the
  base-guessing failure Safety §6 forbids.

## Recommended order

1. `config verify` + snapshot/rollback docs (small, completes loop).
2. `kb916 light pixel-art` CLI flag (small, high demo value).
3. Web pixel-art page (medium, closes the web/CLI feature gap).
4. Research/he-wire verification pass when an HE board is available.
