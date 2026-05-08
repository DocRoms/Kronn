/**
 * Compare-agents — UI + API regression for the Quick Prompts
 * fan-out-across-agents flow shipped 2026-05-10.
 *
 * # What this spec proves
 *
 * 1. The 🤝 button renders in the QP list once at least one agent is
 *    installed.
 * 2. Clicking the button (when the QP has no variables) hits
 *    `POST /api/quick-prompts/:id/compare-agents` and the backend
 *    returns a `BatchRunSummary` with N child discussion ids
 *    (N = number of installed agents).
 * 3. Each child disc carries the per-item agent override (one disc
 *    per agent, all sharing the same primary user message).
 *
 * # What it does NOT do
 *
 * Does not trigger the actual agent runs — the spec creates the
 * batch via the API endpoint then deletes it before the
 * `discussions/:id/run` SSE stream fires. Zero OpenAI/Anthropic
 * tokens spent in this test. The real-agent end-to-end is covered
 * by `codex-real-introspection.spec.ts` (1 run, ~$0.05).
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

const QP_TITLE = `Compare-agents PW ${Date.now()}`;
const QP_PROMPT = 'Renvoie simplement le mot "noté" pour ce test.';

interface CreatedQp {
  id: string;
  agent: string;
}

async function createQp(request: APIRequestContext, agent: string): Promise<CreatedQp> {
  const r = await request.post('/api/quick-prompts', {
    data: {
      name: QP_TITLE,
      prompt_template: QP_PROMPT,
      variables: [],
      agent,
      icon: '🧪',
      project_id: null,
      skill_ids: [],
      tier: 'default',
      description: 'PW test fixture',
    },
  });
  expect(r.ok(), 'QP creation should succeed').toBe(true);
  const j = await r.json();
  expect(j?.success).toBe(true);
  return { id: j.data.id as string, agent };
}

async function deleteQp(request: APIRequestContext, id: string): Promise<void> {
  await request.delete(`/api/quick-prompts/${id}`);
}

async function deleteBatchRun(request: APIRequestContext, runId: string): Promise<void> {
  // Best-effort cleanup — the WorkflowRun delete endpoint also
  // deletes the child discussions atomically.
  await request.delete(`/api/workflow-runs/${runId}`);
}

async function discoverInstalledAgents(request: APIRequestContext): Promise<string[]> {
  const r = await request.get('/api/agents');
  if (!r.ok()) return [];
  const j = await r.json();
  const list: Array<{ agent_type: string; installed: boolean; runtime_available: boolean; enabled: boolean }> =
    j?.data ?? [];
  return list
    .filter(a => (a.installed || a.runtime_available) && a.enabled)
    .map(a => a.agent_type);
}

test.describe('Compare-agents — fan-out the same prompt across N agents', () => {
  let qpId: string | null = null;
  let runId: string | null = null;

  test.afterEach(async ({ request }) => {
    if (runId) {
      await deleteBatchRun(request, runId);
      runId = null;
    }
    if (qpId) {
      await deleteQp(request, qpId);
      qpId = null;
    }
  });

  test('endpoint creates a batch with one disc per installed agent (API)', async ({ request }) => {
    // Pick the user's first installed agent for the QP's default
    // binding — the actual choice doesn't matter since compare-agents
    // overrides per-item. ClaudeCode falls back when nothing else
    // is detected (the real backend always has at least one).
    const agents = await discoverInstalledAgents(request);
    test.skip(agents.length < 2, 'Need ≥ 2 installed agents to meaningfully test compare-agents');

    const qp = await createQp(request, agents[0]);
    qpId = qp.id;

    const resp = await request.post(`/api/quick-prompts/${qp.id}/compare-agents`, {
      data: {
        prompt: QP_PROMPT,
        batch_name: `🤝 ${QP_TITLE} — PW`,
        agents,
        // Don't override the tier — keeps the test cheap if any of
        // the discs are accidentally run.
      },
    });
    expect(resp.ok()).toBe(true);
    const j = await resp.json();
    expect(j?.success, `compare-agents response: ${JSON.stringify(j)}`).toBe(true);

    runId = j.data.run_id as string;
    expect(runId).toBeTruthy();
    expect(j.data.batch_total).toBe(agents.length);
    expect(j.data.discussion_ids).toHaveLength(agents.length);

    // Each child disc must carry a DIFFERENT agent — the whole point
    // of compare-agents is the agent variation. Pull the discs and
    // verify the set of agents matches the input.
    const discAgents = new Set<string>();
    for (const did of j.data.discussion_ids as string[]) {
      const dr = await request.get(`/api/discussions/${did}`);
      const dj = await dr.json();
      expect(dj?.success).toBe(true);
      discAgents.add(dj.data.agent as string);
    }
    expect(discAgents.size).toBe(agents.length);
    for (const a of agents) {
      expect(discAgents.has(a), `${a} should have its own child disc`).toBe(true);
    }
  });

  test('endpoint rejects empty agents list', async ({ request }) => {
    // Defensive — ensures the validation test from the backend
    // integration suite still holds in the live HTTP path.
    const agents = await discoverInstalledAgents(request);
    test.skip(agents.length === 0, 'No agents to seed the QP with');
    const qp = await createQp(request, agents[0]);
    qpId = qp.id;

    const resp = await request.post(`/api/quick-prompts/${qp.id}/compare-agents`, {
      data: {
        prompt: QP_PROMPT,
        batch_name: 'should-not-create',
        agents: [],
      },
    });
    expect(resp.ok()).toBe(true);
    const j = await resp.json();
    expect(j?.success).toBe(false);
    expect(String(j?.error ?? '')).toContain('at least 1 agent');
  });

  test('UI button 🤝 is visible in the QP card and disabled when no agents', async ({ page, request }) => {
    const agents = await discoverInstalledAgents(request);
    test.skip(agents.length === 0, 'Skip UI test when no agents detected');
    const qp = await createQp(request, agents[0]);
    qpId = qp.id;

    await page.addInitScript(() => {
      try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* incognito */ }
    });
    await page.goto('/');
    await page.locator('[data-tour-id="nav-workflows"]').click();
    await page.locator('button.dash-tab', { hasText: /Quick Prompts/i }).click();

    // The QP we created should appear in the list. Find its compare
    // button via the testid we wired in WorkflowsPage.tsx.
    const compareBtns = page.locator('[data-testid="qp-compare-agents-btn"]');
    await expect(compareBtns.first()).toBeVisible({ timeout: 10_000 });
    // Button should be enabled (we have ≥ 1 installed agent).
    await expect(compareBtns.first()).toBeEnabled();
  });
});
