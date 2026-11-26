# oh-my-gravastar web UI

Offline-capable web frontend for GravaStar keyboards, speaking the **khub916
wired** protocol (Web HubX Pro `916` engine) over **WebHID**.

This is an offline PWA for GravaStar devices (khub916 wired writes live;
HE/Compx/SparkPlayJoy surfaces vary by feature). Reads cover device info /
battery / profile / keymap / custom light colours / raw light & macro
regions; writes go through the safety layer (`web/src/safety/`) — a
confirm dialog + pre-write snapshot stand in for the CLI's `--yes`. The
**Custom config (JSON)** page validates desired-state documents against
the shared `schemas/device-config.schema.json` artifact (AJV, Draft
2020-12 — the same file `omg config` uses) and applies the khub916 wired
subset (keymap patch/replace, macros + binds, `sleep_seconds`) through
the safety transaction layer — confirm dialog = approval, a complete
restore point is required before any write (apply aborts if a needed
section fails to snapshot), journal, and stop-on-first-failure including
unverified writes. Sections without a proven lowering are listed as
`not_applied`, never silently skipped.

## Requirements

- **Chrome or Edge** (desktop) — WebHID is not available in Firefox/Safari.
- A **wired** K1 / K98-series keyboard (GravaStar Mercury K1, K1 Pro, K1
  Lite, K98 Pro, Mercury NP…). USB cable required; the 2.4 GHz dongle
  (`916_wireLess` transport) is **not implemented yet** and will be refused
  with an explicit error.
- A secure context: `http://localhost` or HTTPS (plain `file://` does not
  expose WebHID — serve the build, see below).

## Run

```sh
cd web
npm install
npm run dev        # http://localhost:5173
```

Production build + preview:

```sh
npm run build      # tsc typecheck + vite build → dist/
npm run preview    # serve dist/ on http://localhost:4173
```

Any static file server works for `dist/` (`npx serve dist`, `python3 -m
http.server`, nginx, …). The app registers a service worker in production
builds, so after the first load it works **fully offline** — WebHID still
requires the physical device, of course.

Pure-function smoke test (no browser/device needed):

```sh
npm run smoke      # node >= 22.18, native TS type stripping
```

## What it does

1. `navigator.hid.requestDevice()` with the VID/PID filter tables from the
   vendor web app (`docs/research/khub-webapp/index.js` — same tables as
   `crates/hid/src/devices.rs`).
2. Picks the vendor HID collection
   (`featureReports>0 && inputReports>0 && usagePage>=0xFF00`).
3. Probes the feature-report id (declared ids first, then 6, then 1–32) using
   only the read-only device-info query — same approach as
   `probe_report_id` in `crates/proto-khub916`.
4. Sends 519-byte feature reports `[reportId][payload]`; requests are
   `[cmd, arg, 0x00, 0x01, 0x00, tagA, tagB, payload…]` zero-padded, and the
   device echoes the 7-byte header. 3 s timeout, 3 attempts.
5. Renders: model (via the UUID→catalog lookup), UUID, firmware version,
   battery, screen geometry, the 126-slot custom-light colour grid, the
   decoded keymap (layer arg 0–3) and raw byte counts for the profile /
   light-data / macro regions.

## Layout

```
web/
  index.html            single page (#app mount)
  src/main.ts           entry — mounts the shell, registers sw.js (prod)
  src/shell.ts          vendor MainLayout clone: collapsible sidebar nav
                        (device name + icon items + tooltips), topbar with
                        brand/connect/disconnect/refresh, status line
  src/pages.ts          hash router + page renderers (home, lighting, keys,
                        macro, advanced, device); unimplemented pages are
                        read-only placeholders
  src/state.ts          shared app state: connect/refresh/disconnect,
                        snapshot (info/power/profile/light/macro/colors),
                        layer keymap cache
  src/keyboard.ts       DOM-grid keyboard visualizer (per-key RGB from
                        cmd 134, live keymap labels, luminance contrast)
  src/k1pro-layout.ts   K1 Pro key geometry table (vendor keycodeConfig
                        port; pos = 8 + n*4 keymap domain)
  src/hid.ts            WebHID types usage: filters, collection pick,
                        report-id probe, Khub916Session (transact + reads)
  src/proto/khub916.ts  pure protocol: packet builders/parsers, UUID
                        catalog, key-value decode + EN labels
  src/webhid.d.ts       minimal WebHID ambient types
  src/style.css         design tokens + component styles (docs/web-ui-plan.md §1)
  public/               manifest.webmanifest, icon.svg, sw.js (offline)
  scripts/smoke.ts      node smoke test of the pure functions
```

`src/proto/khub916.ts` is a TypeScript port of
`crates/proto-khub916/src/lib.rs` (read path). The binding spec is
`docs/protocol-khub916.md`; the vendor JS reference lives in
`docs/research/khub-webapp/`.

## Not in the MVP (stubs)

- **Writes** — deliberately absent (safety contract). Keymap/profile/light/
  macro writes are the next phase.
- **`916_wireLess`** (2.4 GHz dongle) — different transport (19-byte output
  reports, 14-byte chunked writes, additive checksum); device selection will
  match it but `openKhub916` refuses with an explicit error.
- **Macro region parsing** — returned raw (header semantics unproven,
  spec §6).
- **Light-data (138) decoding** — raw bytes only; per-mode 21-byte RGB
  groups are documented but not rendered.
- **Regional legend tables** (`keyType` 1/2) — EN labels only.
- **Screen / GIF commands** (11/12/13, cmd 8) — K98 Pro screen is a separate
  HID interface anyway.
- **HE keyboards / Compx mice / SparkPlayJoy** — other protocol families,
  out of scope for this scaffold.
