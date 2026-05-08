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
 *   - Fire `POST /run` with a SHORT --max-time so the SSE side is
 *     killed within 1s — simulates the user's network dropping.
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

interface DiscBody {
  id: string;
  message_count: number;
  messages?: Array<{ role: string; content: string }>;
}

async function readDisc(request: APIRequestContext, id: string): Promise<DiscBody | null> {
  const r = await request.get(`/api/discussions/${id}`);
  if (!r.ok()) return null;
  const j = await r.json();
  return (j?.data as DiscBody) ?? null;
}

let discId: string | null = null;

test.describe.configure({ timeout: 120_000, retries: 0 });

test.describe('Disc network-drop — agent task survives client SSE disconnect', () => {
  test.afterAll(async ({ request }) => {
    if (discId) {
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
        initial_prompt: 'Réponds par UN SEUL MOT: ok. Ne fais aucun appel d outil.',
      },
    });
    expect(create.ok()).toBe(true);
    const cj = await create.json();
    expect(cj?.success).toBe(true);
    discId = cj?.data?.id;
    expect(discId).toBeTruthy();

    // 2. Fire `POST /run` and immediately abort. This simulates a
    //    network drop / browser tab close mid-stream — the backend's
    //    `tokio::spawn` keeps the AgentProcess alive regardless of
    //    whether the SSE consumer is still listening.
    const controller = new AbortController();
    const runPromise = fetch(`http://localhost:3140/api/discussions/${discId}/run`, {
      method: 'POST',
      keepalive: true,
      signal: controller.signal,
    }).catch(() => { /* abort is expected */ });
    // 1s window is plenty for the backend to spawn the agent before
    // we close the connection — the cancel registry entry is set
    // synchronously inside `make_agent_stream`.
    setTimeout(() => controller.abort(), 1_000);
    void runPromise;

    // 3. Poll until the agent saves its reply. ClaudeCode answers a
    //    one-word prompt in ~5-15s; we give it 60s.
    let final: DiscBody | null = null;
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const d = await readDisc(request, discId!);
      if (d && d.message_count >= 2) {
        final = d;
        break;
      }
    }
    expect(final, 'Agent message must persist within 60s even after SSE was cut').toBeTruthy();

    // 4. The reply must be a real Agent message (not a "Erreur" /
    //    "interrupted" footer). A natural completion proves the
    //    detached task ran to its `insert_message` path independently
    //    of the SSE consumer.
    const lastAgent = final!.messages?.slice().reverse().find(m => m.role === 'Agent');
    expect(lastAgent).toBeTruthy();
    const content = lastAgent!.content;
    expect(content.length, 'Agent reply must contain real content, not just an error footer').toBeGreaterThan(0);
    expect(content).not.toMatch(/Erreur:/);
  });
});
