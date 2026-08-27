/**
 * Discussion cancel + partial-response recovery — real ClaudeCode run.
 *
 * Validates the kill-mid-stream path end-to-end:
 *
 *   1. Start a real ClaudeCode disc.
 *   2. Once Claude has crossed the durable provider-start boundary,
 *      hit `POST /api/discussions/:id/stop`. This triggers the
 *      cancel_token registered by `make_agent_stream`, which:
 *        - breaks the stream loop
 *        - SIGKILLs the child process via `kill_on_drop(true)`
 *        - saves the partial response with the interruption footer
 *   3. Verify `stop` returned `cancelled: true`; the provider-start
 *      observation above is what proves the agent was actually running.
 *   4. Verify the disc has a saved Agent message with the cancel
 *      footer in its content.
 *   5. Verify `partial_response` is cleared after the save (no
 *      double-write at next backend boot).
 *
 * # Why ClaudeCode
 *
 * Same reasoning as `codex-real-introspection.spec.ts` — Claude's
 * sandbox/spawn behaviour is the most reliable canary. The cancel
 * pathway is shared across ALL agents (cancel_token is generic),
 * so a green test here covers the others by construction.
 *
 * # Cost
 *
 * ~$0.01-0.05 per run. We use a long-running prompt (the agent
 * reasons through ~30s of token output) so we have time to interrupt
 * before completion. Capped via the spec timeout.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

const TITLE = `Cancel-recovery PW ${Date.now()}`;
const BACKEND_URL = process.env.KRONN_BACKEND_URL ?? 'http://localhost:3140';

interface DiscBody {
  id: string;
  message_count: number;
  partial_response?: string | null;
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

test.describe.configure({ timeout: 240_000, retries: 0 });

test.skip(
  process.env.KRONN_REAL_AGENT_E2E !== '1',
  'real-agent spec — set KRONN_REAL_AGENT_E2E=1 (credentials + billed tokens required)',
);

test.describe('Discussion cancel — kills child + saves partial + clears partial_response', () => {
  test.afterAll(async ({ request }) => {
    if (discId) {
      await request.post(`/api/discussions/${discId}/stop`).catch(() => { /* idempotent */ });
      await request.delete(`/api/discussions/${discId}`).catch(() => { /* idempotent */ });
    }
  });

  test('Stop mid-stream → cancelled=true + partial saved with footer + partial_response cleared', async ({ request }) => {
    // 1. Create the disc with a prompt long enough that Claude can't
    //    finish before we cancel. Counting to 200 in French + reason
    //    on each step is reliably 60-90s of streaming.
    const create = await request.post('/api/discussions', {
      data: {
        title: TITLE,
        agent: 'ClaudeCode',
        language: 'fr',
        initial_prompt:
          'Compte de 1 à 200, en mots français, un nombre par ligne. ' +
          'Avant chaque nombre, écris une phrase courte qui le décrit. ' +
          'Ne saute aucun nombre, pas de raccourci.',
      },
    });
    expect(create.ok()).toBe(true);
    const cj = await create.json();
    expect(cj?.success).toBe(true);
    const createdDiscId: string | undefined = cj?.data?.id;
    expect(createdDiscId).toBeTruthy();
    if (!createdDiscId) throw new Error('discussion creation returned no id');
    discId = createdDiscId;

    // 2. Trigger the run with a fire-and-forget POST. The backend's detached
    //    task owns the provider lifecycle independently of this response.
    const runPromise = fetch(`${BACKEND_URL}/api/discussions/${createdDiscId}/run`, {
      method: 'POST',
      keepalive: true,
    }).catch(() => { /* abort/disconnect is expected */ });
    void runPromise;

    // 3. Wait for the durable provider-start boundary. A claimed dispatch can
    //    remain queued behind the per-agent capacity gate for an arbitrary
    //    duration, so a fixed sleep cannot prove that Claude was launched.
    await waitForProviderStart(request, createdDiscId);

    // 4. Stop after the provider is known to be live. The long prompt makes a
    //    natural completion between the observation and this request
    //    unrealistic, so this canary can require an effective cancellation.
    const stop = await request.post(`/api/discussions/${createdDiscId}/stop`);
    expect(stop.ok()).toBe(true);
    const sj = await stop.json();
    expect(sj?.success).toBe(true);
    expect(sj?.data?.cancelled).toBe(true);

    // 5. Wait for the agent to finalize → save → clear partial.
    let final: DiscBody | null = null;
    for (let i = 0; i < 60; i++) {
      await new Promise(r => setTimeout(r, 1_000));
      const d = await readDisc(request, createdDiscId);
      if (d && d.message_count >= 2 && (!d.partial_response || d.partial_response.length === 0)) {
        final = d;
        break;
      }
    }
    expect(final, 'Agent message must be saved AND partial_response cleared within 60s of stop').toBeTruthy();
    if (!final) throw new Error('agent message was not saved after stop');

    const lastAgent = final.messages?.slice().reverse().find(m => m.role === 'Agent');
    expect(lastAgent, 'expected at least one Agent message after cancel').toBeTruthy();
    if (!lastAgent) throw new Error('expected at least one Agent message after cancel');

    // 6. The durable message must carry the explicit interruption marker.
    //    Accepting any non-empty natural reply here used to let the test pass
    //    without exercising the cancellation finalization path at all.
    const content = lastAgent.content;
    expect(content, `expected cancel footer, got: ${content.slice(0, 200)}`).toContain(
      'Exécution interrompue.',
    );
  });
});
