/**
 * Discussion network-drop resilience — agent task survives client-side
 * disconnect.
 *
 * Pins the user-reported worry from the 2026-05-10 multi-agent runs:
 * "j'ai l'impression qu'on ne gère pas correctement toutes ces
 *  déconnexions". The actual contract is that:
 *
 *   1. The frontend's SSE consumer (`fetch` against
 *      `/api/discussions/:id/run`) can disappear mid-stream — the
 *      backend keeps the agent task alive via `tokio::spawn` and
 *      `kill_on_drop(true)` on the AgentProcess.
 *   2. When the agent finishes naturally OR is cancelled, it persists
 *      its message via `insert_message` regardless of whether the
 *      original SSE consumer is still listening.
 *   3. The frontend learns about the new message either via the WS
 *      `BatchRunFinished` / `BatchRunProgress` event OR a manual
 *      `GET /api/discussions/:id` refresh.
 *
 * This spec exercises that contract end-to-end on a real ClaudeCode
 * disc:
 *
 *   - Fire `POST /run`, observe the durable provider-start boundary,
 *     then abort the SSE side — simulates the user's network dropping.
 *   - Wait for the agent to save its reply via the polling path.
 *   - Verify the disc has a saved Agent message even though no SSE
 *     consumer was listening when it landed.
 *
 * # Cost
 *
 * ~$0.01-0.03 per run (one short Claude reply).
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

const TITLE = `Network-drop PW ${Date.now()}`;
const BACKEND_URL = process.env.KRONN_BACKEND_URL ?? 'http://localhost:3140';

interface DiscBody {
  id: string;
  message_count: number;
  messages?: Array<{ role: string; content: string }>;
}

interface DiscussionListed {
  id: string;
  agent_running?: boolean;
}

async function readDisc(request: APIRequestContext, id: string): Promise<DiscBody | null> {
  const r = await request.get(`/api/discussions/${id}`);
  if (!r.ok()) return null;
  const j = await r.json();
  return (j?.data as DiscBody) ?? null;
}

async function waitForProviderStart(request: APIRequestContext, id: string): Promise<void> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const r = await request.get('/api/discussions?page=1&per_page=50');
    if (r.ok()) {
      const j = await r.json();
      const list: DiscussionListed[] = j?.data ?? [];
      if (list.find(d => d.id === id)?.agent_running) return;
    }
    await new Promise(resolve => setTimeout(resolve, 250));
  }
  throw new Error(`provider did not start within 60s for discussion ${id}`);
}

let discId: string | null = null;

test.describe.configure({ timeout: 120_000, retries: 0 });

test.skip(
  process.env.KRONN_REAL_AGENT_E2E !== '1',
  'real-agent spec — set KRONN_REAL_AGENT_E2E=1 (credentials + billed tokens required)',
);

test.describe('Disc network-drop — agent task survives client SSE disconnect', () => {
  test.afterAll(async ({ request }) => {
    if (discId) {
      await request.post(`/api/discussions/${discId}/stop`).catch(() => { /* idempotent */ });
      await request.delete(`/api/discussions/${discId}`).catch(() => { /* idempotent */ });
    }
  });

  test('SSE killed mid-stream still persists agent reply via tokio detached task', async ({ request }) => {
    // 1. Create the disc.
    const create = await request.post('/api/discussions', {
      data: {
        title: TITLE,
        agent: 'ClaudeCode',
        language: 'fr',
        initial_prompt:
          'Réponds avec exactement vingt lignes numérotées décrivant brièvement ' +
          'les étapes d une revue de code. Ne fais aucun appel d outil.',
      },
    });
    expect(create.ok()).toBe(true);
    const cj = await create.json();
    expect(cj?.success).toBe(true);
    const createdDiscId: string | undefined = cj?.data?.id;
    expect(createdDiscId).toBeTruthy();
    if (!createdDiscId) throw new Error('discussion creation returned no id');
    discId = createdDiscId;

    // 2. Fire `POST /run`, wait until the provider really starts, then abort.
    //    This simulates a network drop / browser tab close mid-stream — the
    //    backend's
    //    `tokio::spawn` keeps the AgentProcess alive regardless of
    //    whether the SSE consumer is still listening.
    const controller = new AbortController();
    const runPromise = fetch(`${BACKEND_URL}/api/discussions/${createdDiscId}/run`, {
      method: 'POST',
      keepalive: true,
      signal: controller.signal,
    }).catch(() => { /* abort is expected */ });
    // A claimed job can wait behind the per-agent capacity gate. Polling the
    // DB-backed start boundary avoids disconnecting while the job is merely
    // queued, which would make this a false-positive resilience test.
    try {
      await waitForProviderStart(request, createdDiscId);
    } finally {
      // Also close the client connection if the start wait fails, so a failed
      // canary never leaves an unobserved SSE consumer behind.
      controller.abort();
      await runPromise;
    }

    // 3. Poll until the agent saves its reply. The bounded twenty-line prompt
    //    normally completes quickly; we give it 60s.
    let final: DiscBody | null = null;
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const d = await readDisc(request, createdDiscId);
      if (d && d.message_count >= 2) {
        final = d;
        break;
      }
    }
    expect(final, 'Agent message must persist within 60s even after SSE was cut').toBeTruthy();
    if (!final) throw new Error('agent message did not persist after SSE disconnect');

    // 4. The reply must be a real Agent message (not a "Erreur" /
    //    "interrupted" footer). A natural completion proves the
    //    detached task ran to its `insert_message` path independently
    //    of the SSE consumer.
    const lastAgent = final.messages?.slice().reverse().find(m => m.role === 'Agent');
    expect(lastAgent).toBeTruthy();
    if (!lastAgent) throw new Error('expected an Agent message after SSE disconnect');
    const content = lastAgent.content;
    expect(content.length, 'Agent reply must contain real content, not just an error footer').toBeGreaterThan(0);
    expect(content).not.toMatch(/Erreur:/);
  });
});
