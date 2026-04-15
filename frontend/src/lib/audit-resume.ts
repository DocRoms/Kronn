/**
 * Per-project audit-in-progress checkpoint (localStorage).
 *
 * Solves the bug where navigating away from the Projects page (or even
 * refreshing) during an AI audit lost the progress bar — the SSE stream
 * was aborted, `ProjectCard` unmounted, the useState `auditActive` was
 * reset to false, and the user had to relaunch. The audit itself kept
 * running server-side but the UI had no way to "know" about it.
 *
 * How it's used now:
 *   1. `ProjectCard` writes a checkpoint every time a `step_start` SSE
 *      event arrives while actively streaming.
 *   2. On remount, `ProjectCard` reads the checkpoint; if present, it
 *      polls `GET /api/projects/:id/audit-status` every 2 s and paints
 *      the progress bar from the server's tracker — no SSE subscription
 *      required, which makes the reconnect bulletproof over flaky
 *      networks.
 *   3. When the audit finishes (server reports `null`), the checkpoint
 *      is cleared and the parent refetches projects.
 *
 * Stale checkpoints (older than 1 h) are ignored as a defensive measure:
 * if the backend crashed mid-audit there's no server-side progress to
 * poll against, and we don't want to lock the UI in a permanent "audit
 * in progress" state.
 */

const CHECKPOINT_KEY_PREFIX = 'kronn:audit:';
/// Checkpoints older than this are treated as orphaned and ignored.
/// An audit takes ~20 min tops; 1 h is a conservative upper bound that
/// leaves room for long-running full-audit flows on large repos.
const MAX_CHECKPOINT_AGE_MS = 60 * 60 * 1000;
const SCHEMA_VERSION = 1;

export type AuditCheckpointKind = 'full' | 'partial' | 'full_audit';

export interface AuditCheckpoint {
  projectId: string;
  kind: AuditCheckpointKind;
  startedAt: string;   // ISO-8601
  stepIndex: number;   // last step observed (1-based within the kind's own flow)
  totalSteps: number;
  currentFile: string | null;
}

interface StoredCheckpoint extends AuditCheckpoint {
  v: number;
}

function keyFor(projectId: string): string {
  return CHECKPOINT_KEY_PREFIX + projectId;
}

/** Persist / refresh the checkpoint for a project. */
export function saveAuditCheckpoint(cp: AuditCheckpoint): void {
  if (!cp.projectId) return;
  try {
    const stored: StoredCheckpoint = { v: SCHEMA_VERSION, ...cp };
    localStorage.setItem(keyFor(cp.projectId), JSON.stringify(stored));
  } catch {
    // Storage full or disabled — the live UI still reflects progress for
    // the current tab; only the cross-tab resume is affected.
  }
}

/**
 * Load the checkpoint for a project, or null if none / stale / malformed.
 * Stale entries are removed on the way out so the UI doesn't keep
 * re-polling a dead audit.
 */
export function loadAuditCheckpoint(
  projectId: string,
  now: Date = new Date(),
): AuditCheckpoint | null {
  if (!projectId) return null;
  try {
    const raw = localStorage.getItem(keyFor(projectId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredCheckpoint> | null;
    if (!parsed || typeof parsed !== 'object') return null;
    if (parsed.v !== SCHEMA_VERSION) return null;
    if (typeof parsed.projectId !== 'string'
      || typeof parsed.startedAt !== 'string'
      || typeof parsed.totalSteps !== 'number'
      || typeof parsed.stepIndex !== 'number'
      || typeof parsed.kind !== 'string') return null;

    const startedMs = Date.parse(parsed.startedAt);
    if (Number.isNaN(startedMs)) return null;

    if (now.getTime() - startedMs > MAX_CHECKPOINT_AGE_MS) {
      localStorage.removeItem(keyFor(projectId));
      return null;
    }

    return {
      projectId: parsed.projectId,
      kind: parsed.kind as AuditCheckpointKind,
      startedAt: parsed.startedAt,
      stepIndex: parsed.stepIndex,
      totalSteps: parsed.totalSteps,
      currentFile: parsed.currentFile ?? null,
    };
  } catch {
    return null;
  }
}

/** Drop the checkpoint — called on done / error / cancel. */
export function clearAuditCheckpoint(projectId: string): void {
  if (!projectId) return;
  try { localStorage.removeItem(keyFor(projectId)); } catch { /* ignore */ }
}

/** Exported for tests and for callers that need the TTL. */
export const AUDIT_CHECKPOINT_CONFIG = {
  KEY_PREFIX: CHECKPOINT_KEY_PREFIX,
  MAX_AGE_MS: MAX_CHECKPOINT_AGE_MS,
  SCHEMA_VERSION,
};
