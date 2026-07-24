/**
 * Linear workflow — 3 steps end-to-end, zero agent tokens.
 *
 * Builds a tiny linear workflow with three deterministic `JsonData` steps,
 * triggers it via the public API, and verifies that:
 *
 *   1. The workflow run reaches `Success` (all steps passed).
 *   2. Every step result is persisted in order.
 *   3. The public run endpoint exposes each deterministic output.
 *
 * # Why no Agent step
 *
 * Linear workflow plumbing (run lifecycle, state.json, SSE stream,
 * trigger endpoint, status transitions) is the same regardless of
 * step type. Agent steps add LLM cost + flakiness without adding
 * test coverage of the runner itself. Agent-step coverage is in
 * `disc-cancel-recovery.spec.ts` and `per-agent-mcp-introspection.spec.ts`.
 *
 * # Why no Notify step
 *
 * Browser CI must not depend on public network availability. The complete
 * runner-to-Notify chain, including template propagation and persisted results,
 * is covered in Rust against a process-local HTTP sink with a test-only
 * loopback policy. Production still rejects private targets.
 *
 * # Cost
 *
 * Zero $. No network and no LLM.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

let workflowId: string | null = null;

interface RunStatus {
  status: string; // 'Running' | 'Success' | 'Failed' | …
  step_results: StepResult[];
  state: Record<string, unknown>;
}

interface StepResult {
  step_index: number;
  step_name: string;
  status: string;
  output?: unknown;
  step_kind?: string;
}

async function readRun(request: APIRequestContext, wfId: string, runId: string): Promise<RunStatus | null> {
  const r = await request.get(`/api/workflows/${wfId}/runs/${runId}`);
  if (!r.ok()) return null;
  const j = await r.json();
  return (j?.data as RunStatus) ?? null;
}

test.describe.configure({ timeout: 90_000, retries: 0 });

test.describe('Workflow linear runner — 3 deterministic steps end-to-end', () => {
  test.afterAll(async ({ request }) => {
    if (workflowId) {
      await request.delete(`/api/workflows/${workflowId}`).catch(() => { /* idempotent */ });
    }
  });

  test('JsonData → JsonData → JsonData reaches Success with persisted results', async ({ request }) => {
    // 1. Create the workflow.
    const create = await request.post('/api/workflows', {
      data: {
        name: `PW linear 3-step ${Date.now()}`,
        project_id: null,
        trigger: { type: 'Manual' },
        steps: [
          {
            name: 'seed',
            step_type: { type: 'JsonData' },
            agent: 'ClaudeCode', // unused for JsonData but required by schema
            prompt_template: '',
            mode: { type: 'Normal' },
            json_data_payload: { ticket: 'EW-9999', priority: 'P1', count: 3 },
          },
          {
            name: 'middle',
            step_type: { type: 'JsonData' },
            agent: 'ClaudeCode',
            prompt_template: '',
            mode: { type: 'Normal' },
            json_data_payload: { phase: 'middle', deterministic: true },
          },
          {
            name: 'finalize',
            step_type: { type: 'JsonData' },
            agent: 'ClaudeCode',
            prompt_template: '',
            mode: { type: 'Normal' },
            json_data_payload: { phase: 'final', complete: true },
          },
        ],
      },
    });
    expect(create.ok(), `create returned ${create.status()}`).toBe(true);
    const cj = await create.json();
    expect(cj?.success, `create body: ${JSON.stringify(cj)}`).toBe(true);
    workflowId = cj?.data?.id;
    expect(workflowId).toBeTruthy();

    // 2. Trigger via SSE — fire-and-forget; we read run state via the
    //    list endpoint instead of consuming the stream (simpler).
    const trigger = await fetch(`http://localhost:3140/api/workflows/${workflowId}/trigger`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ variables: {} }),
      keepalive: true,
    }).catch(() => null);
    void trigger;

    // 3. Find the run id (fresh runs surface in /api/workflows/:id/runs).
    let runId: string | null = null;
    for (let i = 0; i < 20; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const list = await request.get(`/api/workflows/${workflowId}/runs`);
      const lj = await list.json();
      const runs = (lj?.data ?? []) as Array<{ id: string }>;
      if (runs.length > 0) { runId = runs[0].id; break; }
    }
    expect(runId, 'trigger should create a run within 20s').toBeTruthy();

    // 4. Wait for run to reach Success or Failed.
    let final: RunStatus | null = null;
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const r = await readRun(request, workflowId!, runId!);
      if (r && (r.status === 'Success' || r.status === 'Failed')) {
        final = r;
        break;
      }
    }
    expect(final, 'run should terminate within 60s').toBeTruthy();

    expect(
      final!.status,
      `expected Success, got ${final!.status} with results ${JSON.stringify(final!.step_results)}`,
    ).toBe('Success');

    // 5. All 3 steps must be marked Success in step_results.
    expect(final!.step_results, 'expected 3 step results').toHaveLength(3);
    for (const sr of final!.step_results) {
      expect(sr.status, `step ${sr.step_name} must succeed`).toBe('Success');
    }

    // 6. Persisted public results preserve step order and payloads.
    const stepNames = final!.step_results.map(s => s.step_name);
    expect(stepNames).toEqual(['seed', 'middle', 'finalize']);
    expect(String(final!.step_results[0].output)).toContain('EW-9999');
    expect(String(final!.step_results[1].output)).toContain('middle');
    expect(String(final!.step_results[2].output)).toContain('final');
  });
});
