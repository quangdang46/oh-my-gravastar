/**
 * Secondary pages for the khub916 configurator shell (bead jwb, P3).
 *
 * Vendor menu entries covered here (docs/web-ui-plan.md §2/§3):
 *   basicFunctions          → `basic`   (keycap/layout editor — vendor
 *                             persists it in browser storage only; the
 *                             `update:keyboard` handler is a no-op, so
 *                             nothing is ever written to the device)
 *   configurationManagement → `config`  (named config slots + JSON
 *                             import/export; we implement the device
 *                             profile export/import as schema-v1 JSON,
 *                             mirroring `omg profile export|import`)
 *   rhythm                  → `rhythm`  (MusicMAxisPlug companion — out
 *                             of scope per plan §3; stub with facts)
 *   setting                 → `setting` (vendor page is an empty
 *                             placeholder; we surface the one writable
 *                             setting the protocol exposes: sleep time)
 *
 * Device overview lives on the existing `device` page in pages.ts; this
 * module also owns `snapshotToJson()` / `buildProfileExport()` which the
 * device page reuses for its JSON view.
 *
 * DOM helpers are duplicated from pages.ts on purpose: this file must
 * stay self-contained so concurrent page work does not conflict.
 */

import { app, refresh, setStatus, subscribe, type DeviceSnapshot } from './state.ts';
import { profileWithSleepSeconds } from './proto/khub916.ts';
import { mountLayoutEditor } from './layout-editor.ts';
import { physicalLayoutForModel, PHYSICAL_LAYOUTS } from './layout-model.ts';
import { confirmDialog } from './modal.ts';
import { initProfileHistory, renderProfileHistory } from './profiles.ts';
import { renderProfileSwitcher } from './profile-switcher.ts';
import { renderCustomConfig } from './custom-config.ts';
import {
  SnapshotStore,
  rollbackSections,
  runTransaction,
  snapshotBeforeWrite,
  validateProfileJson,
  type DeviceSnapshot as SafetySnapshot,
  type PlannedOp,
  type SnapshotSectionName,
} from './safety/index.ts';
import {
  planProfileImport,
  type ParsedImport,
  type ProfileImportWriter,
  type SectionOutcome,
} from './profile-import.ts';

// ── DOM helpers (mirror pages.ts) ──────────────────────────────────────

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = '',
  text = '',
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text) e.textContent = text;
  return e;
}

function card(title: string): { card: HTMLElement; body: HTMLElement } {
  const c = el('section', 'card');
  c.append(el('h2', '', title));
  const body = el('div', 'card-body');
  c.append(body);
  return { card: c, body };
}

function kv(dl: HTMLElement, key: string, value: string): void {
  const dt = el('dt', '', key);
  dt.dataset.k = key.toLowerCase().replace(/\s+/g, '-');
  dl.append(dt, el('dd', '', value));
}

function disconnectedCard(title: string, blurb: string): HTMLElement {
  const { card: c, body } = card(title);
  body.append(el('p', 'hint', blurb));
  body.append(el('p', 'hint warn', 'Connect a keyboard first (top-right → Connect).'));
  return c;
}

// ── Shared JSON helpers ────────────────────────────────────────────────

function bytesToHex(b: Uint8Array): string {
  let s = '';
  for (const x of b) s += x.toString(16).padStart(2, '0');
  return s;
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Pretty JSON of everything the snapshot already holds — used by the
 * Device page's "Snapshot JSON" view.
 */
export function snapshotToJson(snap: DeviceSnapshot): string {
  const doc = {
    device_info: {
      uuid: snap.info.uuid,
      version: snap.info.versionHex,
      screen_size: snap.info.screenSize,
      screen: snap.info.screen,
      catalog_model: snap.info.catalogModel,
      catalog_name: snap.info.catalogName,
    },
    power: { percent: snap.power.percent, flags: `0x${snap.power.flags.toString(16).padStart(2, '0')}` },
    profile: {
      bytes: snap.profile.raw.length,
      light_mode: snap.profile.lightMode,
      sleep_seconds: snap.profile.sleepSeconds,
      hex: bytesToHex(snap.profile.raw),
    },
    light_colors: snap.colors.map((c) => ({ index: c.index, pos: c.pos, r: c.r, g: c.g, b: c.b })),
    light_data_bytes: snap.lightData.length,
    light_data_hex: bytesToHex(snap.lightData),
    macro_bytes: snap.macro.length,
    macro_hex: bytesToHex(snap.macro),
    keymaps_cached: [...snap.keymaps.entries()].map(([arg, keys]) => ({
      arg,
      entries: keys.length,
    })),
  };
  return JSON.stringify(doc, null, 2);
}

/**
 * Build a schema-v1 profile document for the connected khub916 device —
 * the web counterpart of `omg profile export --full` (crates/omg
 * profile.rs `khub916_state`). Reads keymaps for args 0–7 on demand;
 * per-section read failures land as `{"error": …}` like the Rust `sec()`.
 */
export async function buildProfileExport(): Promise<Record<string, unknown>> {
  const c = app.connected;
  const snap = app.snapshot;
  if (!c || !snap) throw new Error('Connect a keyboard first.');

  const sec = async <T>(fn: () => Promise<T>): Promise<unknown> => {
    try {
      return await fn();
    } catch (e) {
      return { error: e instanceof Error ? e.message : String(e) };
    }
  };

  const keymap: Record<string, unknown> = {};
  for (let arg = 0; arg < 8; arg++) {
    keymap[String(arg)] = await sec(async () =>
      (await c.session.keyValues(arg)).map(
        (v) => `0x${(v >>> 0).toString(16).toUpperCase().padStart(8, '0')}`,
      ),
    );
  }

  const state = {
    device_info: await sec(async () => {
      const i = snap.info;
      return {
        uuid: i.uuid,
        version: i.versionHex,
        screen_size: i.screenSize,
        screen: i.screen,
        catalog_model: i.catalogModel,
        catalog_name: i.catalogName,
      };
    }),
    keymap,
    profile: {
      bytes: snap.profile.raw.length,
      light_mode: snap.profile.lightMode,
      sleep_seconds: snap.profile.sleepSeconds,
      hex: bytesToHex(snap.profile.raw),
    },
    power: { percent: snap.power.percent, flags: snap.power.flags },
    light_colors: snap.colors.map((col) => ({
      index: col.index,
      pos: col.pos,
      r: col.r,
      g: col.g,
      b: col.b,
    })),
    light_data_hex: bytesToHex(snap.lightData),
    macro_hex: bytesToHex(snap.macro),
  };

  const coll = c.collection;
  return {
    schema: 1,
    exported_at: new Date().toISOString(),
    host: 'omg-web',
    devices: [
      {
        protocol: 'khub916',
        vid: c.device.vendorId,
        pid: c.device.productId,
        usage_page: coll.usagePage,
        usage: coll.usage,
        model: snap.info.catalogName ?? c.device.productName ?? 'unknown',
        profiles: [{ name: 'current', state }],
      },
    ],
  };
}

// ── Config import (schema-v1 → device writes) ──────────────────────────
//
// The section mapping itself lives in ./profile-import.ts (pure — no
// state/hid imports, smoke-tested by scripts/smoke-import.ts). This file
// only adapts `app`/the session into `planProfileImport`'s context and
// keeps the hardened `applyParsedImport` transaction below.

export type { ParsedImport } from './profile-import.ts';
export type { ImportSection, SectionOutcome } from './profile-import.ts';

// ── Write-path capability detection ────────────────────────────────────
//
// The khub916 write methods (`writeProfile`, `writeKeymap`, …) live on
// `Khub916Session` (hid.ts). `planProfileImport` takes them as a
// structural `ProfileImportWriter`: when a method is absent the section
// is reported as "pending" instead of failing the build or the import.

function sessionWrites(session: unknown): ProfileImportWriter {
  return session as ProfileImportWriter;
}

const PENDING = 'not available in this version yet';

/**
 * Parse a schema-v1 profile file and turn its khub916 `state` into the
 * list of writable sections (see profile-import.ts for the full
 * section → command map). Sections with no verified write path are
 * reported in `skipped`, never silently dropped.
 */
export function parseProfileImport(text: string, fileName: string): ParsedImport {
  const doc: unknown = JSON.parse(text);
  const c = app.connected;
  const snap = app.snapshot;
  if (!c || !snap) throw new Error('Connect a keyboard first.');
  return planProfileImport(doc, fileName, {
    device: {
      vendorId: c.device.vendorId,
      productId: c.device.productId,
      productName: c.device.productName ?? null,
      usagePage: c.collection.usagePage,
      usage: c.collection.usage,
      uuid: snap.info.uuid,
      catalogModel: snap.info.catalogModel,
      catalogName: snap.info.catalogName,
      screenSize: snap.info.screenSize,
    },
    writer: sessionWrites(c.session),
    liveColors: snap.colors,
    currentProfile: snap.profile.raw,
  });
}

/**
 * Apply a parsed import through the hardened write path — the SAME
 * pipeline the file-import button uses (bead th0 reuses it for profile
 * history restore, never bypassing it):
 *
 *   compatWarnings → explicit confirm dialog (model mismatch override)
 *   → `snapshotBeforeWrite` pre-write backup (aborts before any byte on
 *     failure) → `runTransaction` sequential multi-section apply with
 *     policy journal → on partial failure, best-effort
 *     `rollbackSections` of the touched snapshot sections (SAFETY-SPEC
 *     R2) → `refresh()`.
 *
 * Returns human-readable result lines for the caller to display; never
 * throws for ordinary write failures (they land in the journal).
 */
export async function applyParsedImport(p: ParsedImport): Promise<string[]> {
  const c = app.connected;
  const sections = p.sections;
  if (!c) return ['The keyboard disconnected — nothing was written.'];
  const results: string[] = [];

  // Model-mismatch override: the backup targets a different keyboard
  // model — require an explicit, informed confirm.
  if (p.compatWarnings.length) {
    const ok = await confirmDialog({
      title: 'Backup targets a different keyboard model',
      bodyText:
        'This backup was made for a different keyboard model. ' +
        'Applying it may produce unexpected results.',
      details: p.compatWarnings,
      impact: {
        affected: p.sections.map((s) => s.name),
        persistent: true,
        backupCaptured: true,
        rollback: 'partial',
        restartRisk: false,
      },
      confirmLabel: 'Apply anyway',
      cancelLabel: 'Cancel',
      danger: true,
    });
    if (!ok) return ['Import cancelled — the keyboard was not changed.'];
  }

  // SAFETY-SPEC §5 / vision §4 layer 4: snapshot every writable section
  // BEFORE the first write so the previous state can be restored — and
  // so a partial failure can be rolled back (R2). A snapshot failure
  // aborts the import — never write without a restore point.
  let backupNote = '';
  let preSnap: SafetySnapshot | null = null;
  try {
    preSnap = await snapshotBeforeWrite(
      new SnapshotStore(),
      c.session,
      `profile-import ${p.fileName}`,
    );
    const missing = preSnap.errors ? Object.keys(preSnap.errors) : [];
    backupNote = missing.length
      ? ` Pre-write backup ${preSnap.id} is partial (no ${missing.join(', ')} captured).`
      : ` Pre-write backup ${preSnap.id} saved.`;
  } catch (e) {
    results.push(
      `ABORTED before any write — could not save a backup: ${msg(e)}. ` +
        'Nothing was sent to the keyboard.',
    );
  }

  if (backupNote) {
    try {
      // Multi-section apply runs as a transaction (safety/
      // transaction.ts): ops execute sequentially, policy classes are
      // journaled, and the first failure stops the rest — the report
      // below says exactly which parts landed.
      const outcomes: (SectionOutcome | undefined)[] = sections.map(() => undefined);
      const ops: PlannedOp[] = sections.map((s, i) => ({
        type: s.opType,
        section: s.name,
        label: s.summary,
        apply: async () => {
          const out = await s.apply();
          outcomes[i] = out;
          return { verified: out.verified };
        },
      }));
      const tx = await runTransaction(ops, {
        // The two-step button (plus the model-mismatch dialog when
        // needed) is the human approval; every op is still journaled.
        approve: () => true,
      });
      tx.journal.forEach((entry, i) => {
        const s = sections[i];
        const detail = outcomes[i]?.text;
        if (entry.result === 'failed') {
          results.push(`${s.name}: FAILED — ${entry.error ?? 'unknown error'}`);
        } else if (entry.result === 'unverified') {
          results.push(
            `${s.name}: ${detail ?? 'written'} — sent but NOT confirmed on the keyboard`,
          );
        } else {
          results.push(`${s.name}: ${detail ?? 'done'}`);
        }
      });
      if (tx.completed && tx.ok) {
        results.push(`All sections saved.${backupNote}`);
      } else {
        const wrote = tx.counts.success + tx.counts.unverified;
        results.push(
          `Stopped early — ${tx.counts.failed} part(s) failed; ${wrote} of ` +
            `${sections.length} part(s) reached the keyboard.${backupNote} ` +
            'The keyboard may be in a mixed state.',
        );

        // SAFETY-SPEC §5 R2 — best-effort rollback: write the pre-import
        // values back for exactly the snapshot sections that received
        // (or may have received) writes. Every outcome is reported;
        // sections with no captured data are listed, never silently
        // skipped, and nothing here claims a full restore it did not do.
        if (preSnap) {
          const touched = new Set<SnapshotSectionName>();
          const untouchable: string[] = [];
          tx.journal.forEach((entry, i) => {
            if (entry.result !== 'success' && entry.result !== 'unverified') return;
            const s = sections[i];
            if (s?.snapSection) touched.add(s.snapSection);
            else untouchable.push(s?.name ?? entry.section ?? entry.op);
          });
          if (untouchable.length) {
            results.push(
              `No automatic rollback for: ${untouchable.join(', ')} — ` +
                'the pre-write backup does not cover those parts.',
            );
          }
          if (touched.size) {
            try {
              const rb = await rollbackSections(
                preSnap,
                sessionWrites(c.session),
                [...touched],
                // Rolling back the failed apply is part of the same
                // user-approved operation — no second prompt.
                { approve: () => true },
              );
              const restored = rb.sections.filter(
                (s) => s.result === 'success' || s.result === 'unverified',
              );
              const unverifiedN = restored.filter((s) => s.result === 'unverified').length;
              const stillBad = [
                ...rb.sections
                  .filter((s) => s.result !== 'success' && s.result !== 'unverified')
                  .map((s) => `${s.section} (${s.detail ?? s.result})`),
                ...rb.unavailable.map((u) => `${u.section} (${u.detail})`),
              ];
              if (stillBad.length === 0) {
                results.push(
                  `Rolled back ${restored.length}/${rb.requested.length} section(s)` +
                    `${unverifiedN ? ` (${unverifiedN} write(s) sent but not confirmed)` : ''}` +
                    ' — the keyboard is back to its pre-import state.',
                );
              } else {
                results.push(
                  `Rollback incomplete — restored ${restored.length}/${rb.requested.length} ` +
                    `section(s)${unverifiedN ? ` (${unverifiedN} unconfirmed)` : ''}. ` +
                    `Still not restored: ${stillBad.join('; ')}.`,
                );
              }
            } catch (e) {
              results.push(
                `Automatic rollback failed: ${msg(e)} — restore pre-write backup ` +
                  `${preSnap.id} manually from the Backup & restore page.`,
              );
            }
          }
        } else {
          results.push(
            'No pre-write backup was captured — the keyboard may be in a ' +
              'mixed state and cannot be rolled back automatically.',
          );
        }
      }
      await refresh();
    } catch (e) {
      results.push(`FAILED: ${msg(e)}`);
    }
  }
  return results;
}

// ── Page: Basic functions (vendor basicFunctions / keyCodeMove) ────────

function renderBasic(root: HTMLElement): void {
  root.replaceChildren();
  if (!app.connected || !app.snapshot) {
    root.append(
      disconnectedCard(
        'Keyboard layout',
        'Shows the physical layout of the connected keyboard — which keys ' +
          'exist and where they sit.',
      ),
    );
    return;
  }

  const model = app.snapshot.info.catalogModel;
  // Physical layout tables are ported per catalogModel (layout-model.ts);
  // models without a table can still pick any registered layout — the
  // choice is display metadata only.
  const matched = physicalLayoutForModel(model);
  const shownCount = matched
    ? matched.keys.filter((k) => k.display).length
    : PHYSICAL_LAYOUTS.reduce((n, l) => Math.max(n, l.keys.filter((k) => k.display).length), 0);

  const c = card('Keyboard layout');
  const dl = el('dl', 'kv');
  kv(dl, 'Keys', `${shownCount} physical keys`);
  kv(
    dl,
    'Layout table',
    matched
      ? `matched to this keyboard model (${matched.id})`
      : 'no table for this model — pick a layout below (reference geometry)',
  );
  kv(dl, 'Editing', 'visual only — saved in this browser, never sent to the keyboard');
  kv(dl, 'Live preview', 'the Keys page shows the real device layout with live key labels and lighting');
  c.body.append(dl);

  c.body.append(
    el(
      'p',
      'hint',
      'The editor below changes only how the keyboard looks on screen — ' +
        'positions, sizes, rotation, legends and hidden keycaps are stored ' +
        'in this browser (localStorage) and are never written to the device. ' +
        'Key assignments are edited on the Keys page.',
    ),
  );

  mountLayoutEditor(c.body, { model });
  root.append(c.card);
}

// ── Page: Configuration management ─────────────────────────────────────

/** Import files larger than this are rejected before parsing (SAFETY-SPEC R1). */
const MAX_IMPORT_BYTES = 1024 * 1024;
/** Cap on validation problems shown in the rejection modal. */
const MAX_MODAL_DETAILS = 40;

let importPreview: ParsedImport | null = null;
let importConfirm = false;
let importBusy = false;
let importLog: string[] = [];

function renderConfig(root: HTMLElement): void {
  root.replaceChildren();
  if (!app.connected || !app.snapshot) {
    importPreview = null;
    importConfirm = false;
    importBusy = false;
    importLog = [];
    root.append(
      disconnectedCard(
        'Backup & restore',
        'Save everything stored on the keyboard to a file, or restore a ' +
          'previously saved file.',
      ),
    );
    // Version history still works offline (diff two snapshots); only
    // capture/restore needs the device. The profile switcher lists
    // saved profiles with Apply disabled until a keyboard connects.
    renderProfileSwitcher(root);
    renderProfileHistory(root);
    return;
  }

  // ── Export ──
  const ex = card('Save a backup');
  ex.body.append(
    el(
      'p',
      'hint',
      'Downloads a JSON file containing everything currently stored on the ' +
        'keyboard — key assignments, lighting, macros and device info. ' +
        'Use it to restore later or to move settings between keyboards.',
    ),
  );
  const exBtn = el('button', 'sec-btn', 'Download backup file');
  exBtn.type = 'button';
  exBtn.disabled = app.busy;
  exBtn.addEventListener('click', () => {
    void (async () => {
      exBtn.disabled = true;
      exBtn.textContent = 'Reading device…';
      try {
        const doc = await buildProfileExport();
        const blob = new Blob([JSON.stringify(doc, null, 2) + '\n'], {
          type: 'application/json',
        });
        const a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        const stamp = new Date().toISOString().replace(/[:.]/g, '-');
        a.download = `omg-keyboard-backup-${stamp}.json`;
        a.click();
        setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
        setStatus('Backup file downloaded.');
      } catch (e) {
        setStatus(`Could not create the backup: ${e instanceof Error ? e.message : String(e)}`, true);
      } finally {
        exBtn.disabled = false;
        exBtn.textContent = 'Download backup file';
      }
    })();
  });
  ex.body.append(exBtn);
  root.append(ex.card);

  // ── Import ──
  const im = card('Restore from backup');
  im.body.append(
    el(
      'p',
      'hint',
      'Pick a backup file made by this page (or the omg CLI). The page shows ' +
        'exactly what will change before anything is written; each part is ' +
        'checked on the keyboard afterwards.',
    ),
  );

  const fileInput = el('input', 'sec-file');
  fileInput.type = 'file';
  fileInput.accept = '.json,application/json';
  fileInput.addEventListener('change', () => {
    const f = fileInput.files?.[0];
    importPreview = null;
    importConfirm = false;
    if (!f) {
      renderConfig(root);
      return;
    }
    // SAFETY-SPEC R1: reject oversized files before reading them —
    // real backups are a few KB, anything near 1 MiB is not one.
    if (f.size > MAX_IMPORT_BYTES) {
      importLog = [
        `That file is ${Math.ceil(f.size / 1024)} KB — the import limit is ` +
          `${MAX_IMPORT_BYTES / 1024} KB and real backups are only a few KB. Nothing was imported.`,
      ];
      renderConfig(root);
      return;
    }
    void f.text().then(
      (text) => {
        // SAFETY-SPEC R1: full schema-v1 validation before anything is
        // parsed into write sections; every problem is listed and the
        // import aborts.
        const v = validateProfileJson(text);
        if (!v.ok) {
          importPreview = null;
          importLog = [
            `Backup file rejected — ${v.errors.length} problem(s); nothing was imported.`,
          ];
          const details = v.errors.slice(0, MAX_MODAL_DETAILS);
          if (v.errors.length > details.length) {
            details.push(`…and ${v.errors.length - details.length} more problem(s)`);
          }
          void confirmDialog({
            title: 'Backup file rejected',
            bodyText: 'The file failed validation, so nothing was imported.',
            details,
            confirmLabel: 'OK',
            cancelLabel: 'Close',
          });
          renderConfig(root);
          return;
        }
        try {
          importPreview = parseProfileImport(text, f.name);
        } catch (e) {
          importPreview = null;
          importLog = [`That file cannot be used: ${msg(e)}`];
        }
        renderConfig(root);
      },
      (e) => {
        importLog = [`Could not read that file: ${msg(e)}`];
        renderConfig(root);
      },
    );
  });
  im.body.append(fileInput);

  if (importLog.length) {
    const pre = el('pre', 'sec-log');
    pre.textContent = importLog.join('\n');
    im.body.append(pre);
  }

  if (importPreview) {
    const p = importPreview;
    im.body.append(el('p', 'hint', `File: ${p.fileName} — model "${p.model}"`));

    // Compatibility problems (e.g. protocol mismatch) block the import:
    // the section list is still shown for information, but no apply UI.
    if (p.compatErrors.length) {
      im.body.append(
        el('p', 'hint warn', 'This backup cannot be applied to the connected keyboard:'),
      );
      const ul = el('ul', 'fact-list');
      for (const r of p.compatErrors) ul.append(el('li', '', r));
      im.body.append(ul);
    } else if (p.sections.length === 0) {
      im.body.append(el('p', 'hint warn', 'No writable sections found in this file.'));
    } else {
      const ul = el('ul', 'fact-list');
      for (const s of p.sections) ul.append(el('li', '', `${s.name}: ${s.summary}`));
      im.body.append(ul);
      const applyBtn = el(
        'button',
        'sec-btn sec-btn-danger',
        importConfirm ? `Yes — write ${p.sections.length} part(s) to the keyboard` : 'Write to keyboard',
      );
      applyBtn.type = 'button';
      applyBtn.disabled = importBusy || app.busy;
      applyBtn.addEventListener('click', () => {
        if (!importConfirm) {
          importConfirm = true;
          renderConfig(root);
          return;
        }
        // §7: the armed second click opens the shared write-confirmation
        // surface — the section list above is the preview; this dialog
        // states persistence / backup / rollback coverage explicitly.
        void confirmDialog({
          title: `Write ${p.sections.length} part(s) from "${p.fileName}" to the keyboard?`,
          bodyText:
            'Only the parts listed above change. A backup of the current ' +
            'state is saved before anything is written; each part is ' +
            'checked on the keyboard afterwards.',
          impact: {
            affected: p.sections.map((s) => `${s.name}: ${s.summary}`),
            persistent: true,
            backupCaptured: true,
            // rollbackSections covers the snapshot-mapped sections;
            // parts without a captured section are reported, not hidden.
            rollback: 'partial',
            restartRisk: false,
          },
          confirmLabel: 'Write to keyboard',
          danger: true,
        }).then((ok) => {
          if (!ok) return;
          void (async () => {
            importBusy = true;
            importConfirm = false;
            importLog = [];
            renderConfig(root);
            try {
              // Shared hardened apply path (applyParsedImport) — also used
              // by profile-history restore in profiles.ts.
              importLog = await applyParsedImport(p);
            } catch (e) {
              importLog = [`FAILED: ${msg(e)}`];
            } finally {
              importBusy = false;
              importPreview = null;
              renderConfig(root);
            }
          })();
        });
      });
      im.body.append(applyBtn);
      im.body.append(
        el('p', 'hint warn', 'Only the parts listed above change — everything else stays the same. A backup of the current state is saved before anything is written; each part is checked on the keyboard afterwards.'),
      );
    }
    if (p.compatWarnings.length) {
      im.body.append(
        el('p', 'hint warn', 'This backup targets a different keyboard model — applying it needs an extra confirmation:'),
      );
      const ul = el('ul', 'fact-list');
      for (const r of p.compatWarnings) ul.append(el('li', '', r));
      im.body.append(ul);
    }
    if (p.skipped.length) {
      const ul = el('ul', 'fact-list');
      for (const s of p.skipped) ul.append(el('li', '', `skipped — ${s}`));
      im.body.append(ul);
    }
  }
  root.append(im.card);
  // Game/app profile switcher (bead 0hl) — named snapshots, manual
  // apply through the same hardened restore path, active-profile
  // indicator. Browser pages cannot detect the foreground app, so
  // switching here is manual by design.
  renderProfileSwitcher(root);
  // Profile version history / diff / restore (bead th0) — profiles.ts.
  renderProfileHistory(root);
}

// ── Page: Rhythm (vendor rhythm — MusicMAxisPlug companion) ────────────

function renderRhythm(root: HTMLElement): void {
  root.replaceChildren();
  const c = card('Music-reactive lighting');
  c.body.append(
    el(
      'p',
      'hint',
      'Music-reactive lighting now lives on its own page — it listens through ' +
        'your microphone and animates the keyboard lights in real time.',
    ),
  );
  const link = el('a', '', 'Open the Music page');
  link.href = '#/music';
  const row = el('div', 'sec-actions');
  row.append(link);
  c.body.append(row);
  root.append(c.card);
}

// ── Page: Setting (vendor setting — empty placeholder upstream) ────────

let sleepInput = '';
let sleepConfirm = false;
let sleepBusy = false;
let sleepLog: string[] = [];

function renderSetting(root: HTMLElement): void {
  root.replaceChildren();
  if (!app.connected || !app.snapshot) {
    sleepInput = '';
    sleepConfirm = false;
    sleepBusy = false;
    sleepLog = [];
    root.append(
      disconnectedCard(
        'Settings',
        'Device settings that can be changed safely from the browser.',
      ),
    );
    return;
  }

  const snap = app.snapshot;
  const current = snap.profile.sleepSeconds;

  const c = card('Sleep timer');
  const dl = el('dl', 'kv');
  kv(dl, 'Current', current !== null ? `${current} seconds` : '—');
  kv(dl, 'What it does', 'how long the keyboard stays awake before sleeping');
  kv(
    dl,
    'Note',
    'The vendor app only shows this for wireless keyboards and warns it may ' +
      'not work while the keyboard is plugged in — results may vary.',
  );
  c.body.append(dl);

  const row = el('div', 'sec-actions');
  const input = el('input', 'sec-input');
  input.type = 'number';
  input.min = '0';
  input.max = '255';
  input.step = '1';
  input.placeholder = current !== null ? String(current) : 'seconds';
  input.value = sleepInput || (current !== null ? String(current) : '');
  input.addEventListener('input', () => {
    sleepInput = input.value;
  });
  row.append(input);

  const applyBtn = el(
    'button',
    'sec-btn sec-btn-danger',
    sleepConfirm ? 'Yes — save to keyboard' : 'Set sleep time',
  );
  applyBtn.type = 'button';
  applyBtn.disabled = sleepBusy || app.busy;
  applyBtn.addEventListener('click', () => {
    const seconds = Number(input.value);
    if (!Number.isInteger(seconds) || seconds < 0 || seconds > 255) {
      sleepLog = ['Enter a whole number of seconds between 0 and 255.'];
      renderSetting(root);
      return;
    }
    if (!sleepConfirm) {
      sleepConfirm = true;
      renderSetting(root);
      return;
    }
    // §7: the armed second click opens the shared write-confirmation
    // surface before any byte is sent.
    void confirmDialog({
      title: `Set the sleep timer to ${seconds} seconds?`,
      bodyText:
        'Only the sleep time changes — lighting and key assignments stay ' +
        'the same. The change is saved on the keyboard and checked on the device.',
      impact: {
        affected: ['Sleep timer'],
        persistent: true,
        backupCaptured: true,
        rollback: 'full',
        restartRisk: false,
      },
      confirmLabel: 'Save to keyboard',
      danger: true,
    }).then((ok) => {
      if (!ok) {
        sleepConfirm = false;
        renderSetting(root);
        return;
      }
      sleepBusy = true;
      sleepConfirm = false;
      sleepLog = [];
      renderSetting(root);
      void (async () => {
      const session = app.connected?.session;
      const writeProfile = session ? sessionWrites(session).writeProfile : undefined;
      if (!session || !writeProfile) {
        sleepBusy = false;
        sleepLog = [`Saving sleep time is ${PENDING}.`];
        renderSetting(root);
        return;
      }
      try {
        const prof = await session.profile();
        const blob = profileWithSleepSeconds(prof.raw, seconds);
        const out = await writeProfile.call(session, blob);
        sleepInput = '';
        await refresh();
        sleepLog = [`Sleep time set to ${seconds} seconds${out.verified ? ' — checked on the keyboard' : ' — sent but not confirmed'}.`];
      } catch (e) {
        sleepLog = [`Could not save the sleep time: ${e instanceof Error ? e.message : String(e)}`];
      } finally {
        sleepBusy = false;
        renderSetting(root);
      }
      })();
    });
  });
  row.append(applyBtn);
  c.body.append(row);
  c.body.append(
    el('p', 'hint', 'Only the sleep time changes — lighting and key assignments stay the same.'),
  );

  if (sleepLog.length) {
    const pre = el('pre', 'sec-log');
    pre.textContent = sleepLog.join('\n');
    c.body.append(pre);
  }
  root.append(c.card);

  const facts = card('Other settings available elsewhere');
  const ul = el('ul', 'fact-list');
  ul.append(
    el('li', '', 'Lighting effects, speed and brightness — see the Lighting page.'),
    el('li', '', 'Connection details, firmware version and battery — see the Device page.'),
    el('li', '', 'Key assignments, macros and advanced keys — see the Keys, Macro and Advanced pages.'),
    el('li', '', 'Factory reset and firmware updates are deliberately not offered here for safety.'),
  );
  facts.body.append(ul);
  root.append(facts.card);
}

// ── Registry ───────────────────────────────────────────────────────────

export type SecondaryPageId = 'basic' | 'config' | 'rhythm' | 'setting' | 'customconfig';

export const SECONDARY_PAGES: readonly { id: SecondaryPageId; path: string; title: string }[] = [
  { id: 'basic', path: '/basic', title: 'Keyboard layout' },
  { id: 'config', path: '/config', title: 'Backup & restore' },
  { id: 'rhythm', path: '/rhythm', title: 'Music lighting' },
  { id: 'setting', path: '/setting', title: 'Sleep timer' },
  { id: 'customconfig', path: '/customconfig', title: 'Custom config (JSON)' },
] as const;

export const SECONDARY_RENDERERS: Record<SecondaryPageId, (root: HTMLElement) => void> = {
  basic: renderBasic,
  config: renderConfig,
  rhythm: renderRhythm,
  setting: renderSetting,
  customconfig: renderCustomConfig,
};

// Profile history auto-capture (bead th0): wraps session write methods
// once; captures post-write snapshots into localStorage. Deferred past
// module evaluation — profiles.ts ↔ pages-secondary.ts is an import
// cycle, and calling synchronously here can hit `historyStarted`'s TDZ.
queueMicrotask(initProfileHistory);

// Clear module state when the device disconnects.
subscribe(() => {
  if (!app.connected) {
    importPreview = null;
    importConfirm = false;
    importBusy = false;
    importLog = [];
    sleepInput = '';
    sleepConfirm = false;
    sleepBusy = false;
    sleepLog = [];
  }
});
