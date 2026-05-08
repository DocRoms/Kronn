/**
 * Per-agent MCP introspection — real-agent E2E coverage.
 *
 * Generalisation of `codex-real-introspection.spec.ts` (which only
 * exercises ClaudeCode). For each *introspection-capable* installed
 * agent we:
 *
 *   1. Create a disc with a unique primer fact in the user prompt.
 *   2. Prompt the agent to call `mcp__kronn-internal__disc_get_message(0)`
 *      and quote the primer verbatim.
 *   3. Wait for the agent's reply.
 *   4. Assert: reply contains the primer string AND
 *      `discussions.introspection_call_count` rose by at least 1.
 *
 * # Why this exists
 *
 * Without per-agent coverage, MCP regressions are caught only on whichever
 * agent the canary spec uses (today: ClaudeCode). Real-world failures —
 * Gemini's `MCP issues detected. Run /mcp list for status.` prefix bug
 * (2026-05-10), the Codex sandbox block (TD-20260510), Kiro's empty saves
 * after subscription exhaustion — would have surfaced earlier with this.
 *
 * # Skipped agents
 *
 *   • **Codex** — `exec` mode sandbox cancels MCP subprocess spawn before
 *     the bridge runs (TD-20260510-codex-mcp-sandbox-block). Removing
 *     the skip-rule will fail the test until upstream fixes it. Tracked.
 *   • **Vibe / Ollama** — no MCP path; they use the slash-marker
 *     fallback (`KRONN:DISC_*`). Different code path; covered separately.
 *
 * # Cost
 *
 * Per agent: one short tool-call round-trip (~$0.005-0.05 depending
 * on model). The 4-agent worst case stays under $0.20. `retries: 0`
 * so a flaky run doesn't double-bill.
 *
 * # When to run
 *
 *   pnpm exec playwright test e2e/specs/per-agent-mcp-introspection.spec.ts
 *
 * Don't add to CI — needs the prod backend and burns real tokens.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';

interface DiscussionListed {
  id: string;
  title: string;
  agent: string;
  introspection_call_count?: number;
  message_count?: number;
}

async function readDisc(request: APIRequestContext, id: string): Promise<DiscussionListed | null> {
  const r = await request.get(`/api/discussions/${id}`);
  if (!r.ok()) return null;
  const j = await r.json();
  return (j?.data as DiscussionListed) ?? null;
}

async function discoverInstalledAgents(request: APIRequestContext): Promise<string[]> {
  const r = await request.get('/api/agents');
  if (!r.ok()) return [];
  const raw = await r.text();
  // Some agent version banners contain stray ASCII control chars that
  // break JSON.parse. Filter them out before parsing.
  const sanitised = Array.from(raw)
    .filter(c => c.charCodeAt(0) >= 32 || c === '\n' || c === '\r' || c === '\t')
    .join('');
  let parsed: { data?: Array<{ agent_type: string; installed?: boolean; runtime_available?: boolean; enabled?: boolean }> };
  try {
    parsed = JSON.parse(sanitised);
  } catch {
    return [];
  }
  return (parsed.data ?? [])
    .filter(a => (a.installed || a.runtime_available) && a.enabled)
    .map(a => a.agent_type);
}

// Agents that *do* speak MCP and *should* be able to call
// `kronn-internal`. Mirrors `agentSupportsIntrospection()` on the
// frontend — keep in sync.
const INTROSPECTION_AGENTS = ['ClaudeCode', 'Kiro', 'GeminiCli', 'CopilotCli'] as const;

// Tests share one Playwright worker (cf. `playwright.config.ts`,
// fullyParallel: false). We DO NOT use `mode: 'serial'` here because
// a failure in one agent (e.g. Kiro hitting "Monthly request limit
// reached" on the user's free tier) would halt the rest of the
// suite — we still want to know whether Claude/Gemini/Copilot work
// independently. Default mode + workers=1 already serialises in
// practice without the fail-fast trap.
test.describe.configure({ timeout: 240_000, retries: 0 });

/**
 * Probe an agent's CLI directly via `docker exec` with a 10s timeout.
 * Returns a short reason string if the CLI surfaced a known account-
 * side bailout (subscription, rate limit, expired session) — the
 * caller skips the test on a non-null return.
 *
 * Returns `null` when the probe succeeded OR when it was inconclusive
 * (Docker not running, exec error, weird stderr) — in those cases we
 * let the regular test flow run and rely on its own assertions.
 *
 * Probe time budget: ~10s × 4 agents = 40s worst case at suite start.
 * vs. the 3min × 4 timeout we used to eat when an agent was bailing.
 */
async function probeAgentBailout(agent: string): Promise<string | null> {
  const cmd = ({
    ClaudeCode: 'timeout 10 claude --print "test" 2>&1 | head -3',
    Codex: 'echo "test" | timeout 10 codex 2>&1 | head -3',
    Kiro: 'timeout 10 kiro-cli chat --no-interactive --trust-all-tools "ok" 2>&1 | head -8',
    GeminiCli: 'KEY=$(python3 -c "import json; print(json.load(open(\\"/home/kronn/.gemini/settings.json\\"))[\\"apiKey\\"])" 2>/dev/null); GEMINI_API_KEY=$KEY timeout 10 gemini -p "test" 2>&1 | head -3',
    CopilotCli: 'timeout 10 copilot --version 2>&1 | head -3',
  } as Record<string, string>)[agent];
  if (!cmd) return null;
  const { exec } = await import('node:child_process');
  return new Promise(resolve => {
    exec(`docker exec kronn-backend-1 sh -c '${cmd.replace(/'/g, `'\\''`)}'`, { timeout: 15_000 }, (_err, stdout, stderr) => {
      const out = `${stdout}\n${stderr}`.toLowerCase();
      if (out.includes('monthly request limit reached')) return resolve('Kiro free tier exhausted (resets monthly)');
      if (out.includes('rate limit') || out.includes('rate_limit')) return resolve('Rate limit reached upstream');
      if (out.includes('insufficient_quota') || out.includes('billing')) return resolve('Quota / billing issue upstream');
      if (out.includes('not authenticated') || out.includes('expired session')) return resolve('Session expired — re-run /login');
      if (out.includes('command not found')) return resolve('Agent CLI not installed in container');
      resolve(null);
    });
  });
}

for (const AGENT of INTROSPECTION_AGENTS) {
  test.describe(`Introspection — real ${AGENT} run`, () => {
    const TITLE = `Introspection ${AGENT} PW ${Date.now()}`;
    // Unique fact that proves the agent fetched message #0 via the tool
    // (the suffix must be machine-checkable — agents quote it back).
    const PRIMER_FACT = `Le hash de référence pour ce test est ${AGENT}-${Date.now()}-7777.`;
    let discId: string | null = null;
    let installed = false;
    let skipReason: string | null = null;

    test.beforeAll(async ({ request }) => {
      const agents = await discoverInstalledAgents(request);
      installed = agents.includes(AGENT);
      if (!installed) return;

      // Probe the agent CLI directly via `docker exec` with a short
      // timeout. If the bare CLI returns a known account-side bailout
      // (subscription limit, rate limit, expired session, billing),
      // skip the test here rather than burn 3 minutes of disc poll
      // waiting for a counter that will never bump. Way cheaper
      // signal AND clearer for the user. We swallow probe errors
      // (Docker not running, agent missing, etc.) — fall through to
      // the normal flow which has its own assertions.
      const probe = await probeAgentBailout(AGENT);
      if (probe) {
        // eslint-disable-next-line no-console
        console.log(`[per-agent-mcp] skipping ${AGENT}: ${probe}`);
        skipReason = probe;
        return;
      }

      const create = await request.post('/api/discussions', {
        data: {
          title: TITLE,
          agent: AGENT,
          language: 'fr',
          initial_prompt: PRIMER_FACT,
        },
      });
      if (!create.ok()) {
        const body = await create.text();
        throw new Error(`create failed (${create.status()}): ${body.slice(0, 200)}`);
      }
      const j = await create.json();
      expect(j?.success).toBe(true);
      discId = j?.data?.id;
      expect(discId).toBeTruthy();

      // summary_strategy=Off keeps the primer raw for the bridge fetch.
      const patch = await request.patch(`/api/discussions/${discId}`, {
        data: { summary_strategy: 'Off' },
      });
      expect(patch.ok()).toBe(true);
    });

    test.afterAll(async ({ request }) => {
      if (discId) {
        await request.delete(`/api/discussions/${discId}`);
      }
    });

    test(`${AGENT} calls disc_get_message(0) and quotes the primer`, async ({ page, request }) => {
      test.skip(!installed, `${AGENT} is not installed on this runner`);
      test.skip(!!skipReason, `${AGENT} probe bailed: ${skipReason} — skipping per-agent assertions (account-side, not a Kronn bug)`);
      test.skip(!discId, `${AGENT} disc setup failed in beforeAll`);

      const before = (await readDisc(request, discId!))?.introspection_call_count ?? 0;

      await page.addInitScript(() => {
        try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* incognito */ }
      });
      await page.goto('/');
      await page.locator('[data-tour-id="nav-discussions"]').click();
      await page.getByText(TITLE, { exact: true }).first().click({ timeout: 15_000 });

      const composer = page.locator('.disc-composer-textarea').first();
      await composer.waitFor({ state: 'visible', timeout: 10_000 });

      const prompt =
        `Test du wiring MCP \`kronn-internal\`. Appelle l'outil ` +
        `\`mcp__kronn-internal__disc_get_message\` avec idx=0 (le tout ` +
        `premier message User de cette discussion). Recopie le contenu ` +
        `du message qu'il te retourne, mot pour mot, préfixé par "MSG#0: ".` +
        `\n\nN'utilise pas ton contexte direct — utilise UNIQUEMENT l'outil.`;
      await composer.fill(prompt);
      await composer.press('Enter');

      // Counter rises = bridge reached. We poll the disc body too —
      // some agents fail their own pre-conditions before the bridge
      // can fire (Kiro on subscription exhaustion writes "Monthly
      // request limit reached" and exits) and Kronn would never see
      // a tool call. Skip the test in that case rather than fail it
      // 3 minutes later via timeout — the agent's CLI quota is
      // outside Kronn's control.
      const NON_KRONN_BAILOUTS = [
        'Monthly request limit reached', // Kiro AWS Builder ID free tier
        'Rate limit reached',            // Generic upstream throttle
        'insufficient_quota',            // OpenAI / Google billing
        'Unable to verify subscription', // Kiro auth glitch
      ];
      let bailoutHint: string | null = null;
      try {
        await expect.poll(async () => {
          const d = await readDisc(request, discId!);
          // If the agent's reply already landed and matches a bailout
          // message, surface the hint via a thrown sentinel so the
          // poll resolves immediately.
          const lastAgent = (d as DiscussionListed & { messages?: Array<{ role: string; content: string }> })
            ?.messages?.slice().reverse().find(m => m.role === 'Agent');
          if (lastAgent) {
            const hit = NON_KRONN_BAILOUTS.find(s => lastAgent.content.includes(s));
            if (hit) {
              bailoutHint = hit;
              return Number.POSITIVE_INFINITY; // unblock the poll
            }
          }
          return d?.introspection_call_count ?? 0;
        }, {
          timeout: 180_000,
          intervals: [3_000, 5_000, 8_000],
          message: `${AGENT} introspection_call_count should rise above ${before}`,
        }).toBeGreaterThan(before);
      } catch (e) {
        // Bailout-style errors (subscription, rate limit) are infra-side,
        // not Kronn-side — skip rather than fail.
        if (bailoutHint) {
          test.skip(true, `${AGENT}: upstream said "${bailoutHint}" — this is the user's account/billing, not a Kronn bug`);
        }
        throw e;
      }
      // Re-check bailout after the poll resolved (the agent may have
      // produced a tool call AND the bailout message).
      if (bailoutHint) {
        test.skip(true, `${AGENT}: upstream said "${bailoutHint}" — skipping per-agent assertions`);
      }

      // Final agent reply must include the primer hash. The post-stream
      // System message for the badge (ChatHeader pill) lands AFTER the
      // agent reply, so we look up the last Agent role explicitly.
      await expect.poll(async () => {
        const d = await readDisc(request, discId!);
        return d?.message_count ?? 0;
      }, { timeout: 60_000, intervals: [1_000, 2_000] })
        .toBeGreaterThanOrEqual(3);

      const fullResp = await request.get(`/api/discussions/${discId}`);
      const fullJ = await fullResp.json();
      const messages = (fullJ?.data?.messages ?? []) as Array<{ role: string; content: string }>;
      const lastAgent = [...messages].reverse().find(m => m.role === 'Agent');
      expect(lastAgent, `${AGENT}: expected at least one Agent message`).toBeTruthy();
      expect(
        lastAgent!.content,
        `${AGENT} reply must contain the primer hash — proof the bridge fetched message #0`,
      ).toContain('7777');

      // The MessageBubble badge must render the kronn-internal tool call.
      const badges = page.locator('[data-testid="kronn-tool-badge"]');
      await expect(badges.first()).toBeVisible({ timeout: 10_000 });
      const firstBadge = await badges.first().innerText();
      expect(firstBadge).toMatch(/disc_(meta|get_message|summarize)/);
    });
  });
}
