/**
 * Audit launch — SSE smoke + cancel.
 *
 * Real audits cost serious tokens (~$1-3) and minutes of agent time;
 * we don't run them to completion in this regression net. What we DO
 * exercise:
 *
 *   1. `POST /api/projects/:id/full-audit` opens an SSE stream.
 *   2. The first `progress` event arrives within 30s — proves the
 *      audit pipeline (template install + agent spawn) is wired.
 *   3. `POST /api/projects/:id/cancel-audit` short-circuits the run
 *      cleanly — the `audit-status` flips out of in-progress.
 *
 * Closes regression scope of the SSE plumbing AND the cancel path —
 * which are the most fragile bits when refactoring `audit/full.rs`.
 *
 * # Cost
 *
 * ~$0.05-0.20 per run. We cancel within 30s, before the agent gets
 * deep into the 10-step pipeline.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { execSync } from 'node:child_process';

/**
 * Mirrors `AuditProgress` from `backend/src/models/projects.rs`. The
 * endpoint returns `data: null` when no audit is in progress for this
 * project, OR `data: { phase, step_index, total_steps, … }` while a
 * run is active. We treat ANY non-null `data` as "in progress".
 */
interface AuditProgress {
  project_id: string;
  phase: string;
  step_index: number;
  total_steps: number;
}

async function readAuditProgress(request: APIRequestContext, projectId: string): Promise<AuditProgress | null> {
  const r = await request.get(`/api/projects/${projectId}/audit-status`);
  if (!r.ok()) return null;
  const j = await r.json();
  return (j?.data as AuditProgress | null) ?? null;
}

let projectId: string | null = null;
let projectPath: string | null = null;

test.describe.configure({ timeout: 120_000, retries: 0 });

// Skip in CI : the test calls POST /full-audit with `agent: "ClaudeCode"`
// which expects a real Claude binary on PATH to spawn. GitHub runners
// don't have Claude installed, so the subprocess spawn hangs/fails and
// the cancel-then-idle polling never returns → 120s timeout × retries
// inflates the whole CI run to ~16 minutes for nothing. This spec is
// designed to run locally where the user has Claude installed; the
// audit pipeline already has unit-test coverage on the runner side.
test.skip(!!process.env.CI, 'real-agent spec — local-only (requires Claude binary on PATH)');

test.describe('Audit — SSE launch + cancel smoke', () => {
  test.beforeAll(({ request: _ }) => {
    void _;
    // Create a small fixture project. Audit needs a real on-disk
    // directory to scan + write output to.
    const reposBase = process.env.KRONN_REPOS_DIR ?? join(homedir(), 'Repositories');
    projectPath = mkdtempSync(join(reposBase, '_kronn_pw_audit_'));
    execSync('git init -q .', { cwd: projectPath });
    writeFileSync(join(projectPath, 'README.md'), '# Audit fixture\n\nSmall project for audit launch smoke.\n');
    writeFileSync(join(projectPath, 'index.ts'), 'export const ping = () => "pong";\n');
  });

  test.afterAll(async ({ request }) => {
    if (projectId) {
      await request.delete(`/api/projects/${projectId}`).catch(() => { /* idempotent */ });
    }
    if (projectPath) {
      rmSync(projectPath, { recursive: true, force: true });
    }
  });

  test('full-audit SSE fires + cancel returns disc to idle', async ({ request }) => {
    test.skip(!projectPath, 'beforeAll failed');

    // Register the project.
    const create = await request.post('/api/projects/add-folder', {
      data: { path: projectPath, name: 'PW audit fixture' },
    });
    expect(create.ok()).toBe(true);
    const cj = await create.json();
    expect(cj?.success).toBe(true);
    projectId = cj?.data?.id;
    expect(projectId).toBeTruthy();

    // Fire-and-forget the full-audit POST. We intentionally don't
    // consume the SSE stream end-to-end — we just need it to START
    // (= the backend kicks off the audit pipeline) and then we
    // cancel.
    const runPromise = fetch(`http://localhost:3140/api/projects/${projectId}/full-audit`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ agent: 'ClaudeCode', briefing_notes: '' }),
      keepalive: true,
    }).catch(() => { /* SSE close is expected */ });
    void runPromise;

    // Wait until the audit pipeline reports a non-null progress.
    let started: AuditProgress | null = null;
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      started = await readAuditProgress(request, projectId!);
      if (started) break;
    }
    expect(started, 'audit-status should return a non-null progress within 60s — proves the pipeline started').toBeTruthy();
    expect(started!.total_steps).toBeGreaterThan(0);

    // Cancel — proves the cancel-audit endpoint exists AND interrupts
    // the running pipeline.
    const cancel = await request.post(`/api/projects/${projectId}/cancel-audit`);
    expect(cancel.ok()).toBe(true);

    // Status should flip back to null within 30s of cancel.
    let idle = false;
    for (let i = 0; i < 30; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const st = await readAuditProgress(request, projectId!);
      if (!st) { idle = true; break; }
    }
    expect(idle, 'audit-status should clear (null) within 30s of cancel').toBe(true);
  });
});
