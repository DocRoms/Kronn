/**
 * Real-agent introspection — UI-driven Playwright spec on the user's
 * prod backend (:3140).
 *
 * Drives a real LLM call through the Kronn UI to prove the kronn-internal
 * MCP bridge works end-to-end:
 *
 *   1. Auto-injected MCP entry (`~/.codex/config.toml` for Codex,
 *      `cwd/.mcp.json` for Claude Code, etc.) — unit-tested in
 *      `mcp_scanner_test.rs`; this spec exercises the read path.
 *   2. Agent picks up the entry and *invokes* the tool when prompted.
 *   3. Python bridge calls Kronn's HTTP introspection endpoints with
 *      `KRONN_DISCUSSION_ID` + `KRONN_BACKEND_URL` from process env.
 *   4. `introspection_call_count` bumps and the `🔧 N` pill renders
 *      in `ChatHeader.tsx`.
 *
 * # Scope
 *
 * This older canary remains ClaudeCode-only. Codex support is covered by
 * `per-agent-mcp-introspection.spec.ts`; Codex 0.132 fixed the exec-mode
 * MCP spawn blocker that existed in 0.121.
 *
 * # Cost
 *
 * One real Claude Code run with tool use (~$0.05-0.15). Capped well
 * under the user's 2 € budget. `retries: 0` ensures a flaky run
 * doesn't double-bill.
 *
 * # Cleanup
 *
 * `afterAll` deletes the disc via the public API. If the spec
 * crashes mid-run, the user can clean up manually from the sidebar
 * (one swipe).
 *
 * # When to run
 *
 *   pnpm exec playwright test e2e/specs/codex-real-introspection.spec.ts
 *
 * Don't add to CI — needs the prod backend on :3140 with the user's
 * Anthropic creds, burns Claude tokens on every run.
 */

import { test, expect, type APIRequestContext } from '@playwright/test';

const TITLE = `Introspection real-agent PW ${Date.now()}`;
// Unique fact the agent can only retrieve correctly via the tool.
const PRIMER_FACT = 'Le hash de référence pour ce test est xyz9999.';

interface DiscussionListed {
  id: string;
  title: string;
  agent: string;
  introspection_call_count?: number;
  message_count?: number;
}

async function readDisc(request: APIRequestContext, id: string): Promise<DiscussionListed | null> {
  const r = await request.get('/api/discussions?page=1&per_page=50');
  if (!r.ok()) return null;
  const j = await r.json();
  const list: DiscussionListed[] = j?.data ?? [];
  return list.find(d => d.id === id) ?? null;
}

let discId: string | null = null;
let log = '';

// Claude Code tool-use round-trips in 30-90s; bump the per-test
// timeout from the default 30s. `retries: 0` so a transient OpenAI/
// Anthropic blip doesn't double the bill.
test.describe.configure({ mode: 'serial', timeout: 240_000, retries: 0 });

// This spec is explicitly NOT meant to run in CI (cf. file-level docstring
// "Don't add to CI — needs the prod backend on :3140 with the user's
// Anthropic creds, burns Claude tokens on every run"). Without this guard
// the spec ran on GitHub Actions and timed out at 240s on every push because
// no Claude binary is installed on the runner and no creds exist. CI-skip
// makes the spec a noop in CI while keeping it runnable locally where the
// user has both the binary and the budget.
test.skip(!!process.env.CI, 'real-agent spec — local-only (Anthropic creds + Claude binary required, tokens billed per run)');

test.describe('Introspection bridge — real ClaudeCode run on prod', () => {
  test.beforeAll(async ({ request }) => {
    const create = await request.post('/api/discussions', {
      data: {
        title: TITLE,
        agent: 'ClaudeCode',
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

    // summary_strategy=Off keeps the primer raw — we want the bridge
    // to fetch the verbatim message, not a summary.
    const patch = await request.patch(`/api/discussions/${discId}`, {
      data: { summary_strategy: 'Off' },
    });
    expect(patch.ok()).toBe(true);
    log += `\n• created disc ${discId}\n`;
  });

  test.afterAll(async ({ request }) => {
    if (discId) {
      const final = await readDisc(request, discId);
      log += `• final state: ${JSON.stringify(final)}\n`;
      const del = await request.delete(`/api/discussions/${discId}`);
      log += `• cleanup DELETE → ${del.status()}\n`;
    }
    // eslint-disable-next-line no-console
    console.log(log);
  });

  test('ClaudeCode calls disc_get_message(0) on demand and the pill bumps', async ({ page, request }) => {
    test.skip(!discId, 'beforeAll failed');
    const before = (await readDisc(request, discId!))?.introspection_call_count ?? 0;

    await page.addInitScript(() => {
      try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* incognito */ }
    });
    await page.goto('/');
    await page.locator('[data-tour-id="nav-discussions"]').click();
    await page.getByText(TITLE, { exact: true }).first().click({ timeout: 15_000 });

    const composer = page.locator('.disc-composer-textarea').first();
    await composer.waitFor({ state: 'visible', timeout: 10_000 });

    // Force a tool call. Two phrasings layered: a directive ("call the
    // tool") + a strict instruction to mirror its output verbatim.
    // Without explicit prompting Claude often answers from context
    // alone — which would still be correct, but wouldn't prove the
    // wiring. The unique "xyz9999" makes any cited content traceable.
    const prompt =
      'Test du wiring MCP `kronn-internal`. Appelle l\'outil ' +
      '`mcp__kronn-internal__disc_get_message` avec idx=0 (le tout ' +
      'premier message User de cette discussion). Recopie le contenu ' +
      'du message qu\'il te retourne, mot pour mot, préfixé par "MSG#0: ".' +
      '\n\nN\'utilise pas ton contexte direct — utilise UNIQUEMENT l\'outil.';
    await composer.fill(prompt);
    await composer.press('Enter');

    // Wait for the counter to bump (proves the bridge was reached).
    await expect.poll(async () => {
      const d = await readDisc(request, discId!);
      return d?.introspection_call_count ?? 0;
    }, {
      timeout: 180_000,
      intervals: [3_000, 5_000, 8_000],
      message: `introspection_call_count should rise above ${before}`,
    }).toBeGreaterThan(before);

    const after = await readDisc(request, discId!);
    log += `• counter ${before} → ${after?.introspection_call_count}, msgs=${after?.message_count}\n`;

    // Wait for the agent's final reply to land in the disc.
    await expect.poll(async () => {
      const d = await readDisc(request, discId!);
      return d?.message_count ?? 0;
    }, { timeout: 30_000, intervals: [1_000, 2_000] })
      .toBeGreaterThanOrEqual(3);

    // Read the full disc and find the last Agent message. We can't
    // use `/message/-1` because the new badge persistence (post-stream
    // capture in `streaming.rs`) inserts a System message immediately
    // after the Agent reply, so `-1` is now the badge body
    // `[kronn-internal: …]` rather than the agent's prose.
    const fullResp = await request.get(`/api/discussions/${discId}`);
    const fullJ = await fullResp.json();
    expect(fullJ?.success).toBe(true);
    const messages = (fullJ?.data?.messages ?? []) as Array<{ role: string; content: string }>;
    const lastAgent = [...messages].reverse().find(m => m.role === 'Agent');
    expect(lastAgent, 'expected at least one Agent message in the disc').toBeTruthy();
    const reply = lastAgent!.content;
    log += `• agent reply: ${reply.slice(0, 300)}…\n`;

    // Reply must carry the primer's verbatim hash. Hallucination would
    // produce a different string; "xyz9999" only enters the agent's
    // output via the bridge fetch.
    expect(
      reply,
      'agent reply must contain the verbatim primer fact (xyz9999) — proof the bridge fetched message #0',
    ).toContain('xyz9999');

    // Pill should reflect the SQL counter.
    const pill = page.locator('.disc-introspection-pill').first();
    await expect(pill).toBeVisible({ timeout: 10_000 });
    const pillText = (await pill.innerText()).trim();
    log += `• pill text: ${pillText}\n`;
    const pillNum = parseInt(pillText.replace(/[^\d]/g, ''), 10);
    expect(pillNum, 'pill should mirror the SQL counter').toBe(after?.introspection_call_count ?? 0);

    // ── Tool-call badge in transcript (A+B persistence) ────────────
    // The streaming hook in `streaming.rs` captures every
    // `mcp__kronn-internal__*` ToolEnd and persists it as a System
    // message with the `[kronn-internal: tool(args)]` shape. The
    // MessageBubble parses that shape and renders a yellow-tinted
    // badge with the wrench icon. Wait for it to appear inline in
    // the disc — the badge gives the user a permanent record of the
    // tool call (vs. the streaming log which is transient).
    const badges = page.locator('[data-testid="kronn-tool-badge"]');
    await expect(badges.first()).toBeVisible({ timeout: 10_000 });
    const badgeCount = await badges.count();
    log += `• transcript carries ${badgeCount} kronn-tool badge(s)\n`;
    expect(badgeCount, 'at least one badge should render after the tool call').toBeGreaterThanOrEqual(1);
    const firstBadge = await badges.first().innerText();
    log += `• first badge text: ${firstBadge.replace(/\n/g, ' ')}\n`;
    // The agent called disc_get_message — the badge label must
    // mention that tool name. (Codex variants might use
    // `disc_get_message` or `disc_meta` — accept either MCP tool.)
    expect(firstBadge).toMatch(/disc_(meta|get_message|summarize)/);
    expect(firstBadge).toContain('🔧');
  });
});
