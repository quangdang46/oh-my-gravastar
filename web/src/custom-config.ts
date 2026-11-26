/**
 * Custom desired-state config page (docs/device-config-schema.md).
 *
 * Paste a desired-state JSON document → structural validation with the
 * SAME Draft 2020-12 artifact the Rust CLI embeds (`device-config.ts`,
 * AJV) → capability plan (config-apply.ts, same gate as `omg config`)
 * → guided apply for the khub916 wired subset through the safety
 * transaction layer (confirm dialog = approval, SnapshotStore restore
 * point, journal, stop-on-first-failure). Sections without a proven
 * lowering are reported `not_applied`, never silently skipped. The web
 * app never sends raw HID bytes — only the session's verified write
 * methods. Sections the web cannot lower yet are listed honestly and
 * point at `omg config apply` / the per-feature pages.
 */

import { app, subscribe } from './state.ts';
import { detectDevice, type DeviceLike } from './core/device-model.ts';
import {
  validateDeviceConfig,
  deviceConfigSchema,
  DEVICE_CONFIG_SCHEMA_ID,
} from './device-config.ts';
import {
  planConfigSections,
  configSemanticErrors,
  configToOps,
  configPlanHash,
  missingSnapshotSections,
} from './config-apply.ts';
import { dryRun, runTransaction } from './safety/transaction.ts';
import { confirmDialog } from './modal.ts';
import {
  SnapshotStore,
  snapshotBeforeWrite,
  type DeviceSnapshot,
} from './safety/snapshots.ts';

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

const SECTION_KEYS = ['keymap', 'lighting', 'settings', 'macros', 'advanced_keys'] as const;

function deviceLike(): DeviceLike | null {
  if (!app.connected || !app.snapshot) return null;
  return {
    vendorId: app.connected.device.vendorId,
    productId: app.connected.device.productId,
    productName: app.connected.device.productName ?? null,
    uuid: app.snapshot.info.uuid,
    catalogModel: app.snapshot.info.catalogModel,
    catalogName: app.snapshot.info.catalogName,
    screenSize: app.snapshot.info.screenSize ?? 0,
  };
}

function buildAiInstructions(docText: string, doc: Record<string, unknown>): string {
  const like = deviceLike();
  const detected = like ? detectDevice(like) : null;
  const deviceLines = detected
    ? [
        `Connected device: ${detected.model ?? 'unknown model'}`,
        `Protocol: ${detected.protocol}`,
        `Firmware: ${app.snapshot?.info.versionHex ?? 'unknown'}`,
        `Model id: ${app.snapshot?.info.catalogModel ?? 'unknown'}`,
      ]
    : ['No device connected — ask me to fill in the device block for my model.'];
  const sections = SECTION_KEYS.filter((k) => k in doc);
  return [
    'You are writing a device desired-state JSON document for oh-my-gravastar.',
    'It must validate against this JSON Schema (Draft 2020-12):',
    '',
    JSON.stringify(deviceConfigSchema()),
    '',
    'Rules:',
    '- Return ONLY the JSON document, no prose, no markdown fences.',
    '- "schema_version" must be 1.',
    `- Device block: ${JSON.stringify(doc.device ?? {})}`,
    ...deviceLines.map((l) => `- ${l}`),
    `- Sections present in my draft: ${sections.join(', ') || '(none)'}`,
    '- Section "mode" rules: patch+replace sections require "mode"; patch-only sections must NOT have "mode".',
    '- Colors are "#rrggbb" strings. Numbers must satisfy the schema ranges exactly.',
    '- I want these changes: ',
    '(describe your desired changes here before sending to the AI)',
    '',
    'My current draft (fix it, do not start over unless needed):',
    docText,
  ].join('\n');
}

export function renderCustomConfig(root: HTMLElement): void {
  root.replaceChildren();

  const intro = card('Custom config (JSON)');
  intro.body.append(
    el(
      'p',
      'hint',
      'Paste a desired-state JSON document (schema: device-config.schema.json, ' +
        'Draft 2020-12 — the same artifact the omg CLI validates against). ' +
        'Validation is structural only; what can actually be applied depends ' +
        'on the connected device and is checked separately.',
    ),
  );
  root.append(intro.card);

  const edit = card('Document');
  const ta = el('textarea', 'sec-log sec-json') as HTMLTextAreaElement;
  ta.rows = 14;
  ta.style.width = '100%';
  ta.style.resize = 'vertical';
  ta.placeholder = '{\n  "schema_version": 1,\n  "device": { "protocol": "khub916", "vid": 7773, "pid": 62721 },\n  "settings": { "sleep_seconds": 30 }\n}';
  const row = el('div', '');
  row.style.cssText = 'display:flex;gap:8px;margin-top:8px;flex-wrap:wrap';
  const validateBtn = el('button', 'btn btn-primary', 'Validate');
  const clearBtn = el('button', 'btn', 'Clear');
  const aiBtn = el('button', 'btn', 'Copy AI instructions');
  aiBtn.disabled = true;
  row.append(validateBtn, clearBtn, aiBtn);
  edit.body.append(ta, row);
  root.append(edit.card);

  const out = card('Result');
  const outBody = out.body;
  root.append(out.card);

  const applyCard = card('Apply');
  const applyBody = applyCard.body;
  applyCard.card.style.display = 'none';
  root.append(applyCard.card);

  let lastValidDoc: Record<string, unknown> | null = null;
  let lastValidText = '';
  let applying = false;

  function showIssues(issues: { path: string; message: string }[]): void {
    outBody.replaceChildren();
    outBody.append(el('p', 'hint', `${issues.length} problem(s) found:`));
    const ul = el('ul', 'fact-list');
    for (const i of issues.slice(0, 50)) {
      ul.append(el('li', '', `${i.path === '/' ? '(root)' : i.path} — ${i.message}`));
    }
    if (issues.length > 50) ul.append(el('li', '', `…and ${issues.length - 50} more`));
    outBody.append(ul);
    aiBtn.disabled = true;
    lastValidDoc = null;
  }

  function showValid(doc: Record<string, unknown>): void {
    outBody.replaceChildren();
    const r = validateDeviceConfig(doc);
    const protocol = (doc.device as Record<string, unknown> | undefined)?.protocol;
    const like = deviceLike();
    const detected = like ? detectDevice(like) : null;
    const match =
      detected && detected.protocol !== 'unknown'
        ? detected.protocol === protocol
          ? 'matches'
          : `does NOT match (connected: ${detected.protocol})`
        : 'no device connected to compare';
    outBody.append(el('p', '', 'Valid against the device-config schema.'));
    const ul = el('ul', 'fact-list');
    ul.append(el('li', '', `Device protocol in document: ${String(protocol)}`));
    ul.append(el('li', '', `Connected device: ${match}`));
    const sections = SECTION_KEYS.filter((k) => k in doc);
    ul.append(
      el(
        'li',
        '',
        `Sections: ${sections.length ? sections.join(', ') : '(envelope only)'}`,
      ),
    );
    outBody.append(ul);
    aiBtn.disabled = false;
    lastValidDoc = doc;
    renderApplyPanel(doc);
  }

  /** Capability plan + apply controls for a structurally valid doc. */
  function renderApplyPanel(doc: Record<string, unknown>): void {
    applyBody.replaceChildren();
    applyCard.card.style.display = '';
    const plan = planConfigSections(doc);
    const sem = configSemanticErrors(doc);
    const like = deviceLike();
    const detected = like ? detectDevice(like) : null;
    const dev = doc.device as Record<string, unknown> | undefined;
    const protocol = dev?.protocol;
    const vid = dev?.vid;
    const pid = dev?.pid;
    // Optional usage_page in the doc must match the live interface —
    // same check `matching_devices` does on the CLI.
    const wantPage = dev?.usage_page;
    const pageOk =
      wantPage === undefined ||
      wantPage === null ||
      !app.connected ||
      app.connected.collection.usagePage === wantPage;
    const livePage = app.connected?.collection.usagePage;
    const deviceMatch =
      detected && detected.protocol !== 'unknown' && app.connected
        ? detected.protocol === protocol &&
          app.connected.device.vendorId === vid &&
          app.connected.device.productId === pid &&
          pageOk
          ? 'matches'
          : `does NOT match (connected: ${detected.protocol} ${app.connected.device.vendorId.toString(16)}/${app.connected.device.productId.toString(16)} page ${livePage?.toString(16) ?? '?'})`
        : 'no editable device connected';

    const ul = el('ul', 'fact-list');
    ul.append(el('li', '', `Connected device: ${deviceMatch}`));
    if (sem.length) {
      ul.append(el('li', 'hint', `Semantic problems: ${sem.join('; ')}`));
    }
    for (const p of plan) {
      ul.append(
        el(
          'li',
          p.status === 'planned' ? '' : 'hint',
          `${p.name}: ${p.status}${p.detail === 'supported' ? '' : ` — ${p.detail}`}`,
        ),
      );
    }
    applyBody.append(ul);

    const planned = plan.filter((p) => p.status === 'planned');
    const canApply =
      planned.length > 0 &&
      sem.length === 0 &&
      !!app.connected &&
      !app.connected.session.writesBlocked &&
      detected !== null &&
      detected.protocol !== 'unknown' &&
      detected.protocol === protocol &&
      app.connected.device.vendorId === vid &&
      app.connected.device.productId === pid &&
      pageOk;

    if (!app.connected) {
      applyBody.append(
        el('p', 'hint', 'Connect a keyboard above to preview and apply.'),
      );
    } else if (app.connected.session.writesBlocked) {
      applyBody.append(
        el(
          'p',
          'hint',
          'This session is read-only — use "Enable editing" on the Connect banner first.',
        ),
      );
    }
    if (planned.length === 0) {
      applyBody.append(
        el(
          'p',
          'hint',
          'No section in this document has an apply path yet — see `omg config apply` on the CLI.',
        ),
      );
    }
    if (sem.length) {
      applyBody.append(
        el('p', 'hint', 'Fix the semantic problems above before applying.'),
      );
    }
    if (!canApply || planned.length === 0) return;

    const btnRow = el('div', '');
    btnRow.style.cssText = 'display:flex;gap:8px;margin-top:8px;flex-wrap:wrap';
    const previewBtn = el('button', 'btn', 'Preview changes');
    const applyBtn = el('button', 'btn btn-primary', 'Apply to keyboard');
    btnRow.append(previewBtn, applyBtn);
    applyBody.append(btnRow);
    const statusLine = el('p', 'hint', '');
    applyBody.append(statusLine);

    previewBtn.addEventListener('click', () => {
      const ops = configToOps(doc, app.connected!.session);
      const plan2 = dryRun(ops);
      statusLine.textContent =
        `${ops.length} write op(s): ${plan2.requiresApproval} need approval, ` +
        `${plan2.blocked} blocked by policy. ` +
        ops.map((o) => `${o.label ?? o.type} [${o.section ?? ''}]`).join('; ');
    });

    applyBtn.addEventListener('click', () => {
      if (applying || !app.connected) return;
      // Lock synchronously, before any `await`/`.then()` boundary: the
      // dialog below is async, and a second click (or double Enter)
      // while it is still open must not slip past this guard and start
      // a second concurrent plan/approve/write flow against the same
      // session. Every exit path below (cancel, error, race-abort,
      // completion) must release the lock exactly once.
      applying = true;
      const sessionAtPlan = app.connected.session;
      const ops = configToOps(doc, sessionAtPlan);
      if (ops.length === 0) {
        applying = false;
        return;
      }
      const summary = ops.map((o) => `• ${o.label ?? o.type}`).join('\n');
      // Approval is bound to this exact plan: the hash covers the doc,
      // the section plan and the connected device — shown in the dialog
      // so the user approves a specific, reproducible state.
      const c = app.connected;
      void configPlanHash(doc, plan, {
        vendorId: c.device.vendorId,
        productId: c.device.productId,
        usagePage: c.collection.usagePage,
      })
        .then((hash) =>
          confirmDialog({
            title: 'Apply this config to the keyboard?',
            bodyText:
              'This writes the changes below to the keyboard and checks each ' +
              'step was saved. A restore point is saved first — you can roll ' +
              'back from the Backup & restore page.',
            preText: `${summary}\nplan ${hash.slice(0, 16)}`,
            hint: 'Sections marked not_applied are skipped and reported.',
            confirmLabel: 'Apply to keyboard',
          }).then((ok) => {
            if (!ok) {
              applying = false;
              return;
            }
            // The dialog was open for an arbitrary amount of time —
            // re-check the session is still the one the plan/approval
            // was built against. A disconnect/reconnect (or a different
            // device) during approval must not let a stale `ops` closure
            // (captured over `sessionAtPlan`) write to the wrong session.
            if (!app.connected || app.connected.session !== sessionAtPlan) {
              applying = false;
              statusLine.textContent =
                'Device changed while the approval dialog was open — apply ' +
                'aborted, nothing was written. Re-open Apply to try again.';
              return;
            }
            // doApply owns the lock from here (including its release).
            void doApply(doc, ops, statusLine, hash);
          }),
        )
        .catch((e) => {
          applying = false;
          statusLine.textContent = `Apply aborted: ${e instanceof Error ? e.message : String(e)}`;
        });
    });
  }

  async function doApply(
    doc: Record<string, unknown>,
    ops: ReturnType<typeof configToOps>,
    statusLine: HTMLElement,
    planHash?: string,
  ): Promise<void> {
    // `applying` is already true here — the caller (applyBtn handler)
    // sets the lock before this is invoked and it owns releasing it on
    // every exit path below; doApply never sets it, only clears it.
    const c = app.connected;
    if (!c) {
      applying = false;
      return;
    }
    statusLine.textContent = 'Saving a restore point…';
    let snap: DeviceSnapshot;
    try {
      snap = await snapshotBeforeWrite(
        new SnapshotStore(),
        c.session,
        'Custom config apply',
      );
    } catch (e) {
      applying = false;
      statusLine.textContent = `Restore point failed (${e instanceof Error ? e.message : e}) — apply aborted, nothing was written.`;
      return;
    }
    // Hard gate: every section the ops will touch must be captured
    // cleanly, or there is no proven rollback — do not write.
    const missing = missingSnapshotSections(ops, snap);
    if (missing.length > 0) {
      applying = false;
      statusLine.textContent =
        `Restore point incomplete (missing/failed: ${missing.join(', ')}) — ` +
        'apply aborted, nothing was written. Try again or use the CLI.';
      return;
    }
    statusLine.textContent = `Restore point saved (${snap.id}); applying…`;
    const result = await runTransaction(ops, {
      approve: () => true, // the confirmDialog above IS the approval
      // Desired-state: an unverified write stops the apply and is not
      // reported as success — same stop policy as `omg config apply`.
      stopOnUnverified: true,
    });
    applying = false;
    const lines = result.journal.map(
      (j) =>
        `${j.section ?? j.op}: ${j.result}${j.error ? ` — ${j.error}` : ''}`,
    );
    const verdict = result.ok
      ? 'Done'
      : result.completed
        ? 'Finished with unverified writes'
        : 'Stopped early';
    statusLine.textContent =
      `${verdict}: ${result.counts.success} ok, ${result.counts.unverified} unverified, ` +
      `${result.counts.failed} failed${result.completed ? '' : ' (stopped early)'}. ` +
      `Restore point: ${snap.id} (Backup & restore page).` +
      (planHash ? ` Plan: ${planHash.slice(0, 16)}.` : '');
    const ul = el('ul', 'fact-list');
    for (const l of lines) ul.append(el('li', '', l));
    applyBody.append(ul);
    void doc;
  }

  validateBtn.addEventListener('click', () => {
    const text = ta.value.trim();
    if (!text) {
      outBody.replaceChildren(el('p', 'hint', 'Paste a JSON document first.'));
      return;
    }
    let doc: unknown;
    try {
      doc = JSON.parse(text);
    } catch (e) {
      showIssues([{ path: '/', message: `not valid JSON: ${(e as Error).message}` }]);
      return;
    }
    const r = validateDeviceConfig(doc);
    if (!r.valid) {
      showIssues(r.issues);
      return;
    }
    lastValidText = text;
    showValid(doc as Record<string, unknown>);
  });

  clearBtn.addEventListener('click', () => {
    ta.value = '';
    outBody.replaceChildren();
    aiBtn.disabled = true;
    lastValidDoc = null;
  });

  aiBtn.addEventListener('click', () => {
    if (!lastValidDoc) return;
    const text = buildAiInstructions(lastValidText, lastValidDoc);
    void navigator.clipboard.writeText(text).then(() => {
      aiBtn.textContent = 'Copied — paste into any AI chat';
      setTimeout(() => (aiBtn.textContent = 'Copy AI instructions'), 2000);
    });
  });

  // Keep the connected-device hint fresh.
  subscribe(() => {
    if (lastValidDoc) showValid(lastValidDoc);
  });
}
