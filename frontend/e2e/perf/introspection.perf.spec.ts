/**
 * Introspection regression — locks the public surface of the
 * `/api/discussions/{id}/{meta,message/:idx,summarize}` triplet that the
 * `kronn-internal` MCP bridge exposes to agents.
 *
 * # Why this lives in `e2e/perf/`
 *
 * The spec needs a deterministic, seeded discussion (20 specific
 * messages, summary_strategy=Off) — running it against the user's real
 * DB would either fail (no such disc) or pollute it. Like the other
 * perf specs, it targets the perf-sandbox at /tmp/kronn-perf-sandbox.
 *
 * # What this spec does NOT do
 *
 * It does not spawn a real agent. The Phase B-5 manual run with
 * Claude Code on 2026-05-10 already proved end-to-end that the agent
 * picks up the MCP tools and calls `disc_get_message`. Re-running that
 * burns ~$0.10 and 30+ s per spec invocation, which doesn't belong in a
 * regression suite. Instead we hit the HTTP routes the bridge wraps,
 * asserting they return what the agent needs and that
 * `introspection_call_count` increments on each call (the UI pill in
 * `ChatHeader.tsx`).
 *
 * # How to run
 *
 *   python3 frontend/e2e/perf/seed_introspection.py
 *   env KRONN_DATA_DIR=/tmp/kronn-perf-sandbox \
 *       backend/target/debug/kronn &
 *   env KRONN_BACKEND_URL=http://localhost:3142 \
 *       pnpm --filter kronn-frontend dev &
 *   cd frontend && pnpm exec playwright test e2e/perf/introspection.perf.spec.ts
 *
 * If the seed has not been run, the test skips cleanly with a hint.
 */
import { test, expect } from '@playwright/test';

// Optional override: hit the backend directly instead of through the
// Vite proxy. Useful for CI or quick local runs where Vite isn't up but
// the perf-sandbox backend is reachable on :3142. Empty string keeps
// the Playwright baseURL ('http://localhost:5173') as the route prefix.
const API_BASE = process.env.KRONN_INTROSPECTION_BASE ?? '';

const url = (path: string) => `${API_BASE}${path}`;

const TITLE = 'Introspection E2E test';

// Index 4 in the seed is the canonical assertion target — specific
// enough that an agent answering from raw context still has to quote it
// verbatim, and robust to future tweaks of the surrounding messages.
const FACT_AT_INDEX_4 = 'Le commit fautif est probablement abc1234 sur la branche feat/header-redesign.';
const FACT_AT_INDEX_18 = 'Bon, dernière info : la deadline est mardi 14 mai à 17h CET.';

interface DiscMeta {
  id: string;
  title: string;
  message_count: number;
  summary_strategy: string;
  has_cached_summary: boolean;
  summary_up_to_msg_idx: number | null;
}

async function findSeededDisc(request: import('@playwright/test').APIRequestContext): Promise<DiscMeta | null> {
  // Page through discussions; the seed-test disc is small and was
  // inserted recently, so a single page hits it. Bail early on empty
  // backend (perf sandbox not booted) so the skip message is friendly.
  const r = await request.get(url('/api/discussions?page=1&per_page=200'));
  if (!r.ok()) return null;
  const j = await r.json();
  const list: Array<{ id: string; title: string }> = j?.data ?? [];
  if (list.length === 0) return null;
  const hit = list.find(d => d.title === TITLE);
  if (!hit) return null;
  // Re-fetch via the meta endpoint so we read through the same code
  // path the MCP bridge exercises — and so the counter starts bumping.
  // (We pull the count fresh after each call below to verify deltas.)
  const m = await request.get(url(`/api/discussions/${hit.id}/meta`));
  if (!m.ok()) return null;
  const mj = await m.json();
  return mj?.data ?? null;
}

async function readCount(request: import('@playwright/test').APIRequestContext, id: string): Promise<number> {
  // The introspection_call_count is exposed on the full Discussion row
  // (used by the UI pill), not on the compact meta payload. Read it via
  // the standard list endpoint with a single-item filter — cheaper than
  // the full discussion fetch and stable across versions.
  const r = await request.get(url(`/api/discussions?page=1&per_page=200`));
  const j = await r.json();
  const list: Array<{ id: string; introspection_call_count?: number }> = j?.data ?? [];
  const row = list.find(d => d.id === id);
  return row?.introspection_call_count ?? 0;
}

test.describe('Introspection bridge — API regression', () => {
  test('seeded disc returns meta, message-by-index, and cached summary', async ({ request }) => {
    const disc = await findSeededDisc(request);
    test.skip(
      disc === null,
      `No "${TITLE}" discussion in the sandbox — run \`python3 frontend/e2e/perf/seed_introspection.py\` against /tmp/kronn-perf-sandbox/kronn.db first`,
    );
    const did = disc!.id;

    // ── meta ─────────────────────────────────────────────────────────
    expect(disc!.message_count, 'seed has 20 non-system messages').toBe(20);
    expect(disc!.summary_strategy).toBe('Off');
    expect(disc!.has_cached_summary).toBe(false);

    const beforeCount = await readCount(request, did);

    // ── message at fixed index ───────────────────────────────────────
    const m4 = await request.get(url(`/api/discussions/${did}/message/4`));
    expect(m4.ok()).toBe(true);
    const m4j = await m4.json();
    expect(m4j?.success).toBe(true);
    expect(m4j?.data?.idx).toBe(4);
    expect(m4j?.data?.role).toBe('User');
    expect(m4j?.data?.content).toContain('abc1234');
    expect(m4j?.data?.content).toBe(FACT_AT_INDEX_4);

    // ── negative index (last message) ────────────────────────────────
    const last = await request.get(url(`/api/discussions/${did}/message/-1`));
    expect(last.ok()).toBe(true);
    const lastj = await last.json();
    expect(lastj?.success).toBe(true);
    // The seed ends with an Agent reply ("Deadline ... noté."). The
    // spec asserts on the User fact at -2 to keep the regression
    // pinned to a content-bearing message rather than the agent ack.
    expect(lastj?.data?.idx).toBe(19);
    const m18 = await request.get(url(`/api/discussions/${did}/message/-2`));
    expect(m18.ok()).toBe(true);
    const m18j = await m18.json();
    expect(m18j?.data?.content).toBe(FACT_AT_INDEX_18);

    // ── out-of-range guard ───────────────────────────────────────────
    const oob = await request.get(url(`/api/discussions/${did}/message/999`));
    const oobj = await oob.json();
    expect(oobj?.success).toBe(false);
    expect(String(oobj?.error ?? '')).toMatch(/out of range/i);

    // ── ranged summarize (cold + warm) ───────────────────────────────
    // Only run the cold path when the user has a backend agent
    // configured for summary generation — otherwise the test would
    // depend on Vibe/Claude Code creds being set up. We detect that by
    // looking for a non-error response on the first call; if it's an
    // auth error we skip the rest.
    const summCold = await request.post(url(`/api/discussions/${did}/summarize`), {
      data: { from: 0, to: 10 },
    });
    const sj = await summCold.json();
    if (!sj?.success) {
      test.info().annotations.push({
        type: 'skipped-section',
        description: `summarize cold-path skipped — backend returned: ${String(sj?.error ?? 'unknown')}. The agent generation step needs at least one credentialed agent.`,
      });
    } else {
      expect(sj?.data?.from_idx).toBe(0);
      expect(sj?.data?.to_idx).toBe(10);
      expect(sj?.data?.generated).toBe(true);
      expect(typeof sj?.data?.summary).toBe('string');
      expect((sj?.data?.summary as string).length).toBeGreaterThan(20);

      // Warm path: same range must hit the ranged cache. `generated:
      // false` is the Phase B-1 invariant — the cache returned the
      // stored summary verbatim instead of regenerating. The cache
      // also replays the original `tokens_used` so the agent sees
      // cost (not 0), so we don't assert on tokens here.
      const summWarm = await request.post(url(`/api/discussions/${did}/summarize`), {
        data: { from: 0, to: 10 },
      });
      const wj = await summWarm.json();
      expect(wj?.success).toBe(true);
      expect(wj?.data?.generated).toBe(false);
      expect(wj?.data?.summary).toBe(sj?.data?.summary);
    }

    // ── counter incrementing (Phase B-4) ─────────────────────────────
    // Bump-eligible calls between beforeCount and afterCount:
    //   m4 (+1), last (+1), m18 (+1), oob (+1),         → 4 bumps
    //   summarize cold (+1, even on err — the handler bumps before
    //   cache lookup or generation),                    → 5
    //   summarize warm (+1, only if cold succeeded).    → 5 or 6
    // The initial /meta inside findSeededDisc also bumps, but it
    // happens before beforeCount is captured, so it doesn't count
    // toward the delta. Lower bound for the delta is therefore 5.
    const afterCount = await readCount(request, did);
    expect(
      afterCount - beforeCount,
      `introspection_call_count should bump by ≥ 5 (was ${beforeCount}, now ${afterCount})`,
    ).toBeGreaterThanOrEqual(5);
  });
});
