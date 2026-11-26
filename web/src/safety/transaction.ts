/**
 * Transaction executor — vision-full-custom.md §4 layer 4
 * (backup → preview → approval → write → read-back verify) and §4.2
 * "lighting multi-write continues after failure → stop + report partial".
 *
 * `runTransaction` executes planned ops SEQUENTIALLY (concurrent feature
 * reports poison the khub916 session) and STOPS on the first failure.
 * The journal records every attempt honestly: `success`, `failed` or
 * `unverified` (the write was sent but read-back did not confirm — e.g.
 * adv-key cmd 18 on K1 Pro). A transaction that stopped early reports
 * `completed: false` and `ok: false`; callers must surface the partial
 * result, never claim full success.
 *
 * Policy is enforced here, not trusted to callers: L4 ops are refused,
 * L2/L3 ops require `approve()` to return true.
 */

import type { AuditLog } from './audit-log.ts';
import { classify, type OpDescriptor, type OpType, type PolicyDecision } from './policy.ts';

/** Minimal write outcome — matches `WriteOutcome` in hid.ts structurally. */
export interface WriteOutcomeLike {
  verified?: boolean;
}

/**
 * One planned operation. `apply` performs the actual write (and its
 * read-back verify, inside the session methods); returning
 * `{ verified: false }` marks the journal entry `unverified`.
 */
export interface PlannedOp extends OpDescriptor {
  label?: string;
  /** Optional payload echoed into dry-run plans / journal detail. */
  payload?: unknown;
  apply: () => Promise<WriteOutcomeLike | void>;
}

export type JournalResult = 'success' | 'failed' | 'unverified';

export interface JournalEntry {
  op: OpType;
  section?: string;
  label?: string;
  /** Policy class at execution time. */
  class: PolicyDecision['class'];
  /** ISO-8601 start time. */
  started: string;
  finished: string;
  result: JournalResult;
  error?: string;
}

export interface TransactionOptions {
  /**
   * Approval gate for L2/L3 ops. Called once per gated op, in order.
   * Returning false (or omitting `approve` entirely) denies the op and
   * stops the transaction — safety-first default.
   */
  approve?: (op: PlannedOp, decision: PolicyDecision) => boolean | Promise<boolean>;
  /** Optional audit sink — every journal entry is appended. */
  audit?: AuditLog;
  /**
   * Opaque session/context handle, accepted for API symmetry with the
   * page layer; not used by the executor itself.
   */
  session?: unknown;
  /**
   * Desired-state applies set this: an `unverified` write (sent but not
   * confirmed by read-back) stops the transaction like a failure and
   * makes `ok` false — continuing to write more sections on top of an
   * unconfirmed state is not acceptable for config apply.
   */
  stopOnUnverified?: boolean;
}

export interface TransactionResult {
  /** Per-op journal, in execution order. */
  journal: JournalEntry[];
  /** True when every op ran and none failed. */
  ok: boolean;
  /** True when the loop reached the end (no early stop). */
  completed: boolean;
  /** Count of ops by result — for honest partial reporting. */
  counts: Record<JournalResult, number>;
}

function msg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function emptyCounts(): Record<JournalResult, number> {
  return { success: 0, failed: 0, unverified: 0 };
}

/**
 * Execute `ops` sequentially under policy. Stops on the first `failed`
 * entry (including denied approval and blacklisted ops); `unverified`
 * does not stop the transaction but is reported honestly — unless
 * `opts.stopOnUnverified` is set (desired-state applies), in which case
 * it stops like a failure and `ok` is false.
 */
export async function runTransaction(
  ops: readonly PlannedOp[],
  opts: TransactionOptions = {},
): Promise<TransactionResult> {
  const journal: JournalEntry[] = [];
  const counts = emptyCounts();
  let completed = true;

  const record = (entry: JournalEntry): void => {
    journal.push(entry);
    counts[entry.result]++;
    opts.audit?.append({
      op: entry.op,
      class: entry.class,
      result: entry.result,
      detail: [entry.section, entry.label, entry.error].filter(Boolean).join(' — ') || undefined,
    });
  };

  for (const op of ops) {
    const decision = classify(op);
    const started = new Date().toISOString();
    const base = {
      op: op.type,
      section: op.section,
      label: op.label,
      class: decision.class,
      started,
    };

    // L4 — blacklisted, can never run.
    if (!decision.allowed) {
      record({
        ...base,
        finished: new Date().toISOString(),
        result: 'failed',
        error: `refused by policy: ${decision.reason}`,
      });
      completed = false;
      break;
    }

    // L2/L3 — explicit approval required.
    if (decision.requiresApproval) {
      let approved = false;
      try {
        approved = opts.approve ? await opts.approve(op, decision) : false;
      } catch (e) {
        record({
          ...base,
          finished: new Date().toISOString(),
          result: 'failed',
          error: `approval check failed: ${msg(e)}`,
        });
        completed = false;
        break;
      }
      if (!approved) {
        record({
          ...base,
          finished: new Date().toISOString(),
          result: 'failed',
          error: 'approval denied — operation not permitted',
        });
        completed = false;
        break;
      }
    }

    try {
      const out = await op.apply();
      const verified = out?.verified !== false;
      const result: JournalResult = verified ? 'success' : 'unverified';
      record({
        ...base,
        finished: new Date().toISOString(),
        result,
        ...(verified ? {} : { error: 'write sent but read-back did not confirm it' }),
      });
      if (!verified && opts.stopOnUnverified) {
        completed = false;
        break;
      }
    } catch (e) {
      record({
        ...base,
        finished: new Date().toISOString(),
        result: 'failed',
        error: msg(e),
      });
      completed = false;
      break;
    }
  }

  return {
    journal,
    ok: counts.failed === 0 && !(opts.stopOnUnverified === true && counts.unverified > 0),
    completed,
    counts,
  };
}

// ── Dry run ────────────────────────────────────────────────────────────

export interface DryRunOp {
  type: OpType;
  section?: string;
  label?: string;
  class: PolicyDecision['class'];
  requiresApproval: boolean;
  allowed: boolean;
  reason: string;
}

export interface DryRunPlan {
  ops: DryRunOp[];
  /** Ops that would need approval. */
  requiresApproval: number;
  /** Ops policy would refuse outright. */
  blocked: number;
}

/**
 * Classify a batch without executing anything — the "preview" half of
 * backup → preview → approval → write. `apply` is never invoked.
 */
export function dryRun(ops: readonly PlannedOp[]): DryRunPlan {
  const plan: DryRunOp[] = ops.map((op) => {
    const d = classify(op);
    return {
      type: op.type,
      section: op.section,
      label: op.label,
      class: d.class,
      requiresApproval: d.requiresApproval,
      allowed: d.allowed,
      reason: d.reason,
    };
  });
  return {
    ops: plan,
    requiresApproval: plan.filter((p) => p.requiresApproval).length,
    blocked: plan.filter((p) => !p.allowed).length,
  };
}
