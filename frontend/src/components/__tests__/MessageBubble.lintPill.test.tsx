/**
 * 0.8.7 anti-hallucination — per-message lint pill regression suite.
 *
 * Pins:
 *   - fabricated citations (high-confidence) render a red pill, severity="fabricated"
 *   - only-unsourced (low-confidence heuristic) renders an amber pill, severity="unsourced"
 *   - no lint_report (or empty) → no pill at all
 *   - clicking the pill toggles a detail panel listing the bad sources + flagged spans
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage, LintReport } from '../../types/generated';

function makeAgentMessage(lint: LintReport | null): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'Agent',
    content: 'Some answer.',
    agent_type: 'ClaudeCode',
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
    lint_report: lint,
  };
}

const baseProps = {
  idx: 0,
  isLastUser: false,
  isLastAgent: false,
  isEditing: false,
  isCopied: false,
  isTtsActive: false,
  ttsState: 'idle' as const,
  isExpandedSummary: false,
  prevUserTs: null,
  defaultAgent: 'ClaudeCode' as const,
  summaryCache: null,
  language: 'fr',
  sending: false,
  editingText: '',
  hasFullAccess: false,
  onCopy: () => {},
  onTts: () => {},
  onEditStart: () => {},
  onEditCancel: () => {},
  onEditSubmit: () => {},
  onEditTextChange: () => {},
  onRetry: () => {},
  onExpandSummary: () => {},
  discussionId: 'disc-test',
  projectId: null,
  onNavigate: () => {},
  t: (key: string) => key,
};

function renderBubble(lint: LintReport | null) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} msg={makeAgentMessage(lint)} />
    </I18nProvider>
  );
}

describe('MessageBubble — anti-hallucination lint pill', () => {
  it('renders a fabricated (red) pill when a citation did not verify', () => {
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'ghost.rs:1', kind: 'file', status: 'not_found', detail: 'file not found: ghost.rs' },
      ],
      fabricated_count: 1,
    });
    const pill = screen.getByTestId('lint-pill');
    expect(pill.getAttribute('data-severity')).toBe('fabricated');
    expect(pill.textContent).toContain('1');
    expect(pill.textContent).toContain('disc.lintFabricated');
  });

  it('renders an unsourced (amber) pill when only the heuristic flagged claims', () => {
    renderBubble({
      unsourced_count: 2,
      flagged_spans: [{ text: 'The retry uses backoff', reason: 'uses ' }],
      sources: [],
      fabricated_count: 0,
    });
    const pill = screen.getByTestId('lint-pill');
    expect(pill.getAttribute('data-severity')).toBe('unsourced');
    expect(pill.textContent).toContain('2');
    expect(pill.textContent).toContain('disc.lintUnsourced');
  });

  it('fabricated takes precedence over unsourced for the headline color', () => {
    renderBubble({
      unsourced_count: 3,
      flagged_spans: [{ text: 'x', reason: 'y' }],
      sources: [{ raw: 'a.rs:9', kind: 'file', status: 'out_of_bounds', detail: 'beyond length' }],
      fabricated_count: 1,
    });
    expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('fabricated');
  });

  it('renders NO pill when there is no lint report', () => {
    renderBubble(null);
    expect(screen.queryByTestId('lint-pill')).toBeNull();
  });

  it('renders NO pill when the report carries no sources and no flags (truly silent)', () => {
    // The "empty report" case — nothing to surface, no pill. NB: as of 0.8.7
    // a report with at least one mechanically-verified source DOES render a
    // green pill (see "renders a verified (green) pill …" below), so the
    // pre-0.8.7 shape (verified source = no pill) is no longer the contract.
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [],
      fabricated_count: 0,
    });
    expect(screen.queryByTestId('lint-pill')).toBeNull();
  });

  it('clicking the pill toggles a detail panel with bad sources + flagged spans', () => {
    renderBubble({
      unsourced_count: 1,
      flagged_spans: [{ text: 'The cache lives in memory', reason: 'lives in' }],
      sources: [
        { raw: 'ghost.rs:1', kind: 'file', status: 'not_found', detail: 'file not found: ghost.rs' },
        { raw: 'real.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
      ],
      fabricated_count: 1,
    });
    // Collapsed by default.
    expect(screen.queryByTestId('lint-detail')).toBeNull();
    fireEvent.click(screen.getByTestId('lint-pill'));
    const detail = screen.getByTestId('lint-detail');
    // The fabricated source is shown; the verified one is filtered out.
    expect(detail.textContent).toContain('ghost.rs:1');
    // 0.8.8 (honesty): the verified source is now ALSO listed (its own group)
    // even on a red report — "say what's good AND what's bad". It used to be hidden.
    expect(detail.textContent).toContain('real.rs:1');
    // The heuristic-flagged span is shown too.
    expect(detail.textContent).toContain('The cache lives in memory');
    // Toggle closes it.
    fireEvent.click(screen.getByTestId('lint-pill'));
    expect(screen.queryByTestId('lint-detail')).toBeNull();
  });

  it('renders NO pill on a non-Agent message even if a report is attached', () => {
    // Defense-in-depth: the backend only attaches reports to Agent messages,
    // but the render must not surface a pill on a User/System message.
    const userMsg = {
      ...makeAgentMessage({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'x.rs:9', kind: 'file' as const, status: 'not_found' as const, detail: 'nope' }],
        fabricated_count: 1,
      }),
      role: 'User' as const,
    };
    render(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={userMsg} />
      </I18nProvider>
    );
    expect(screen.queryByTestId('lint-pill')).toBeNull();
  });

  it('detail panel surfaces the "verified ≠ true" caveat', () => {
    renderBubble({
      unsourced_count: 1,
      flagged_spans: [{ text: 'x', reason: 'y' }],
      sources: [],
      fabricated_count: 0,
    });
    fireEvent.click(screen.getByTestId('lint-pill'));
    // identity translator → the i18n key renders verbatim
    expect(screen.getByTestId('lint-detail').textContent).toContain('disc.lintCaveat');
  });

  // 0.8.7 — positive signal : every citation verified, no unsourced flag.
  // Green pill ("verified"), shows the source list when expanded.
  it('renders a verified (green) pill when every cited source resolved', () => {
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'src/foo.rs:14', kind: 'file', status: 'verified', detail: 'file exists' },
        { raw: 'src/bar.rs:1-9', kind: 'file', status: 'verified', detail: 'lines 1-9 within 200 lines' },
      ],
      fabricated_count: 0,
    });
    const pill = screen.getByTestId('lint-pill');
    expect(pill.getAttribute('data-severity')).toBe('verified');
    // count + i18n key (identity-translated)
    expect(pill.textContent).toContain('2');
    expect(pill.textContent).toContain('disc.lintVerified');
  });

  it('unchecked-only sources do NOT earn a green pill (they get the neutral one)', () => {
    // Reply citing only urls / users → none mechanically verified, so NO green.
    // Option B (2026-05-30): instead of being hidden, they get the NEUTRAL
    // "unverifiable" pill — never a green chip Kronn didn't earn.
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'https://example.com/doc', kind: 'url', status: 'unchecked', detail: 'URL — not network-checked' },
        { raw: 'user:rom:2026-05-28: disc-1', kind: 'user', status: 'unchecked', detail: 'user-declared' },
      ],
      fabricated_count: 0,
    });
    const pill = screen.getByTestId('lint-pill');
    expect(pill.getAttribute('data-severity')).toBe('unchecked'); // neutral, not green
    expect(pill.getAttribute('data-severity')).not.toBe('verified');
  });

  it('fabricated outranks verified — mixed report stays red', () => {
    // Priority guard : one bad citation must not be hidden under a green chip
    // just because another verified. Color encodes the worst signal.
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'src/ok.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
        { raw: 'src/ghost.rs:1', kind: 'file', status: 'not_found', detail: 'file not found' },
      ],
      fabricated_count: 1,
    });
    expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('fabricated');
  });

  it('green pill click expands a positive "Verified sources" list', () => {
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'src/foo.rs:14', kind: 'file', status: 'verified', detail: 'file exists' },
      ],
      fabricated_count: 0,
    });
    fireEvent.click(screen.getByTestId('lint-pill'));
    const group = screen.getByTestId('lint-verified-group');
    expect(group.textContent).toContain('src/foo.rs:14');
    expect(group.textContent).toContain('disc.lintVerifiedTitle'); // i18n key
  });

  it('verified drawer carries data-severity="verified" (green bg, not amber)', () => {
    // Regression: the detail drawer used a hardcoded amber background, so a
    // GREEN pill expanded onto an orange panel that read as "not good".
    // The drawer must echo the pill's severity so the CSS can colour it green.
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [{ raw: 'src/foo.rs:14', kind: 'file', status: 'verified', detail: 'file exists' }],
      fabricated_count: 0,
    });
    fireEvent.click(screen.getByTestId('lint-pill'));
    expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('verified');
  });

  it('fabricated drawer carries data-severity="fabricated"', () => {
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [{ raw: 'ghost.rs:1', kind: 'file', status: 'not_found', detail: 'nope' }],
      fabricated_count: 1,
    });
    fireEvent.click(screen.getByTestId('lint-pill'));
    expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('fabricated');
  });

  // ───────────────────────────────────────────────────────────────────────
  // 0.8.7+ — exhaustive severity matrix, count plumbing, drawer grouping,
  // per-status colour hooks, unchecked suppression and toggle lifecycle.
  // The pill is a 3-state binary-coloured signal (red>amber>green) — every
  // transition below pins both the pill AND drawer data-severity so the CSS
  // can never re-introduce the "green pill on amber panel" regression.
  // ───────────────────────────────────────────────────────────────────────

  // ── 1. Severity priority matrix ──────────────────────────────────────────
  describe('severity priority matrix (fabricated > unsourced > verified)', () => {
    it('all three present → pill AND drawer are fabricated (red)', () => {
      renderBubble({
        unsourced_count: 4,
        flagged_spans: [{ text: 'claim alpha', reason: 'heuristic' }],
        sources: [
          { raw: 'src/ok.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'src/ghost.rs:1', kind: 'file', status: 'not_found', detail: 'file not found' },
        ],
        fabricated_count: 2,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('fabricated');
      fireEvent.click(pill);
      expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('fabricated');
    });

    it('unsourced + verified (no fabricated) → pill AND drawer are unsourced (amber)', () => {
      renderBubble({
        unsourced_count: 1,
        flagged_spans: [{ text: 'claim beta', reason: 'heuristic' }],
        sources: [{ raw: 'src/ok.rs:1', kind: 'file', status: 'verified', detail: 'file exists' }],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('unsourced');
      fireEvent.click(pill);
      expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('unsourced');
    });

    it('verified only → pill AND drawer are verified (green)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'src/ok.rs:1', kind: 'file', status: 'verified', detail: 'file exists' }],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('verified');
      fireEvent.click(pill);
      expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('verified');
    });

    it('unsourced count > 0 but fabricated_count 0 with NO verified source → amber (heuristic-only)', () => {
      renderBubble({
        unsourced_count: 5,
        flagged_spans: [{ text: 'claim gamma', reason: 'heuristic' }],
        sources: [],
        fabricated_count: 0,
      });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('unsourced');
    });
  });

  // ── 2. Counts in the pill label ──────────────────────────────────────────
  describe('pill label counts + i18n keys', () => {
    it('fabricated pill shows fabricated_count (not the source-array length)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        // 3 bad sources but the backend-supplied count is the source of truth
        sources: [
          { raw: 'a.rs:1', kind: 'file', status: 'not_found', detail: 'no' },
          { raw: 'b.rs:1', kind: 'file', status: 'not_found', detail: 'no' },
          { raw: 'c.rs:1', kind: 'file', status: 'not_found', detail: 'no' },
        ],
        fabricated_count: 7,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.textContent).toContain('7');
      expect(pill.textContent).toContain('disc.lintFabricated');
      expect(pill.textContent).not.toContain('disc.lintUnsourced');
      expect(pill.textContent).not.toContain('disc.lintVerified');
    });

    it('unsourced pill shows unsourced_count + disc.lintUnsourced', () => {
      renderBubble({
        unsourced_count: 9,
        flagged_spans: [{ text: 'x', reason: 'y' }],
        sources: [],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.textContent).toContain('9');
      expect(pill.textContent).toContain('disc.lintUnsourced');
      expect(pill.textContent).not.toContain('disc.lintFabricated');
    });

    it('verified pill shows verifiedCount (count of verified sources, not array length)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [
          { raw: 'a.rs:1', kind: 'file', status: 'verified', detail: 'ok' },
          { raw: 'b.rs:1', kind: 'file', status: 'verified', detail: 'ok' },
          { raw: 'c.rs:1', kind: 'file', status: 'verified', detail: 'ok' },
          // an unchecked one must NOT inflate the verified count
          { raw: 'https://x', kind: 'url', status: 'unchecked', detail: 'not checked' },
        ],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.textContent).toContain('3');
      expect(pill.textContent).toContain('disc.lintVerified');
    });
  });

  // ── 3. Drawer contents per severity ──────────────────────────────────────
  describe('drawer grouping per severity', () => {
    it('fabricated drawer lists every bad status under lintSourcesTitle, hides verified + unchecked', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [
          { raw: 'bad-notfound.rs:1', kind: 'file', status: 'not_found', detail: 'missing' },
          { raw: 'bad-oob.rs:1', kind: 'file', status: 'out_of_bounds', detail: 'beyond length' },
          { raw: 'bad-empty.rs', kind: 'file', status: 'empty_ref', detail: 'empty ref' },
          { raw: 'bad-outside.rs', kind: 'file', status: 'outside_project', detail: 'outside' },
          { raw: 'bad-rejected.rs', kind: 'file', status: 'rejected', detail: 'rejected' },
          { raw: 'good.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'https://unchecked', kind: 'url', status: 'unchecked', detail: 'not checked' },
        ],
        fabricated_count: 5,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const detail = screen.getByTestId('lint-detail');
      expect(detail.textContent).toContain('disc.lintSourcesTitle');
      // all five bad ones listed
      expect(detail.textContent).toContain('bad-notfound.rs:1');
      expect(detail.textContent).toContain('bad-oob.rs:1');
      expect(detail.textContent).toContain('bad-empty.rs');
      expect(detail.textContent).toContain('bad-outside.rs');
      expect(detail.textContent).toContain('bad-rejected.rs');
      // 0.8.8 (honesty / Option B): even on a red report the drawer shows
      // EVERYTHING — the verified source (its group) AND the uncheckable URL
      // (the "unverifiable" group). Say what's good, what's bad, what can't be
      // tested. Nothing hidden.
      expect(detail.textContent).toContain('good.rs:1');
      expect(screen.getByTestId('lint-verified-group')).not.toBeNull();
      expect(detail.textContent).toContain('https://unchecked');
      expect(screen.getByTestId('lint-unverifiable-group')).not.toBeNull();
    });

    it('verified drawer lists verified sources under lintVerifiedTitle, hides unchecked', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [
          { raw: 'src/foo.rs:14', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'src/bar.rs:1-9', kind: 'file', status: 'verified', detail: 'within bounds' },
          { raw: 'https://unchecked', kind: 'url', status: 'unchecked', detail: 'not checked' },
        ],
        fabricated_count: 0,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const group = screen.getByTestId('lint-verified-group');
      expect(group.textContent).toContain('disc.lintVerifiedTitle');
      expect(group.textContent).toContain('src/foo.rs:14');
      expect(group.textContent).toContain('src/bar.rs:1-9');
      expect(group.textContent).not.toContain('https://unchecked');
    });

    it('flagged spans listed under lintUnsourcedTitle on an amber drawer', () => {
      renderBubble({
        unsourced_count: 2,
        flagged_spans: [
          { text: 'span one text', reason: 'r1' },
          { text: 'span two text', reason: 'r2' },
        ],
        sources: [],
        fabricated_count: 0,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const detail = screen.getByTestId('lint-detail');
      expect(detail.textContent).toContain('disc.lintUnsourcedTitle');
      expect(detail.textContent).toContain('span one text');
      expect(detail.textContent).toContain('span two text');
    });

    it('caveat is always present in the drawer (fabricated)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'ghost.rs:1', kind: 'file', status: 'not_found', detail: 'no' }],
        fabricated_count: 1,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      expect(screen.getByTestId('lint-detail').textContent).toContain('disc.lintCaveat');
    });

    it('caveat is always present in the drawer (verified)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'ok.rs:1', kind: 'file', status: 'verified', detail: 'exists' }],
        fabricated_count: 0,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      expect(screen.getByTestId('lint-detail').textContent).toContain('disc.lintCaveat');
    });
  });

  // ── 4. unchecked-only → neutral pill (Option B) ──────────────────────────
  describe('unchecked-only → neutral pill', () => {
    it('only unchecked sources (url/user/inferred) → NEUTRAL "unverifiable" pill', () => {
      // Option B: surfaced honestly (not hidden, not green).
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [
          { raw: 'https://example.com', kind: 'url', status: 'unchecked', detail: 'url' },
          { raw: 'user:test', kind: 'user', status: 'unchecked', detail: 'user-declared' },
          { raw: 'inferred-claim', kind: 'inferred', status: 'unchecked', detail: 'inferred' },
        ],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('unchecked');
      expect(pill.textContent).toContain('3'); // all three listed
    });

    it('unchecked sources do NOT suppress an amber pill when a claim is flagged', () => {
      renderBubble({
        unsourced_count: 1,
        flagged_spans: [{ text: 'flagged claim', reason: 'heuristic' }],
        sources: [{ raw: 'https://x', kind: 'url', status: 'unchecked', detail: 'url' }],
        fabricated_count: 0,
      });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('unsourced');
    });
  });

  // ── 5. mixed verified + unsourced — verified group NOT shown on amber ─────
  describe('mixed amber report (verified source + flagged claim)', () => {
    it('amber drawer shows the flagged span but does NOT list the verified source', () => {
      renderBubble({
        unsourced_count: 1,
        flagged_spans: [{ text: 'unsourced claim here', reason: 'heuristic' }],
        sources: [{ raw: 'src/verified.rs:1', kind: 'file', status: 'verified', detail: 'exists' }],
        fabricated_count: 0,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const detail = screen.getByTestId('lint-detail');
      // flagged span IS shown
      expect(detail.textContent).toContain('unsourced claim here');
      // 0.8.8 (honesty): the verified source is now ALSO listed on an amber
      // report — "say what's good AND what's missing". It used to be hidden.
      expect(screen.getByTestId('lint-verified-group')).not.toBeNull();
      expect(detail.textContent).toContain('src/verified.rs:1');
      // no bad-sources group (the only source verified)
      expect(detail.textContent).not.toContain('disc.lintSourcesTitle');
    });
  });

  // ── 6. each bad status maps to the red (fabricated) pill ──────────────────
  describe('each bad status counts toward the red pill', () => {
    const badStatuses = ['not_found', 'out_of_bounds', 'empty_ref', 'outside_project', 'rejected'] as const;
    badStatuses.forEach(status => {
      it(`status "${status}" → fabricated (red) pill + listed in drawer`, () => {
        renderBubble({
          unsourced_count: 0,
          flagged_spans: [],
          sources: [{ raw: `bad-${status}.rs:1`, kind: 'file', status, detail: `${status} detail` }],
          fabricated_count: 1,
        });
        const pill = screen.getByTestId('lint-pill');
        expect(pill.getAttribute('data-severity')).toBe('fabricated');
        fireEvent.click(pill);
        const detail = screen.getByTestId('lint-detail');
        expect(detail.textContent).toContain(`bad-${status}.rs:1`);
      });
    });
  });

  // ── 7. toggle lifecycle + non-Agent + null report ───────────────────────
  describe('toggle lifecycle & guards', () => {
    it('toggles the drawer open → close → open across three clicks', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'ok.rs:1', kind: 'file', status: 'verified', detail: 'exists' }],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(screen.queryByTestId('lint-detail')).toBeNull();
      fireEvent.click(pill);
      expect(screen.getByTestId('lint-detail')).toBeTruthy();
      fireEvent.click(pill);
      expect(screen.queryByTestId('lint-detail')).toBeNull();
      fireEvent.click(pill);
      expect(screen.getByTestId('lint-detail')).toBeTruthy();
    });

    it('aria-expanded mirrors the drawer state', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'ok.rs:1', kind: 'file', status: 'verified', detail: 'exists' }],
        fabricated_count: 0,
      });
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('aria-expanded')).toBe('false');
      fireEvent.click(pill);
      expect(pill.getAttribute('aria-expanded')).toBe('true');
    });

    it('System message never shows a pill even with a fabricated report', () => {
      const sysMsg = {
        ...makeAgentMessage({
          unsourced_count: 0,
          flagged_spans: [],
          sources: [{ raw: 'x.rs:9', kind: 'file' as const, status: 'not_found' as const, detail: 'no' }],
          fabricated_count: 1,
        }),
        role: 'System' as const,
      };
      render(
        <I18nProvider>
          <MessageBubble {...baseProps} msg={sysMsg} />
        </I18nProvider>
      );
      expect(screen.queryByTestId('lint-pill')).toBeNull();
    });

    it('explicit null lint_report → no pill', () => {
      renderBubble(null);
      expect(screen.queryByTestId('lint-pill')).toBeNull();
    });
  });

  // ── 8. data-status colour hook (CSS keys off these) ──────────────────────
  describe('per-item data-status colour hook', () => {
    it('verified items carry data-status="verified"', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [{ raw: 'ok.rs:1', kind: 'file', status: 'verified', detail: 'exists' }],
        fabricated_count: 0,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const group = screen.getByTestId('lint-verified-group');
      const item = group.querySelector('.disc-lint-item');
      expect(item?.getAttribute('data-status')).toBe('verified');
    });

    it('bad items carry their own status code (e.g. out_of_bounds)', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        sources: [
          { raw: 'a.rs:1', kind: 'file', status: 'out_of_bounds', detail: 'beyond' },
          { raw: 'b.rs:1', kind: 'file', status: 'not_found', detail: 'missing' },
        ],
        fabricated_count: 2,
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const detail = screen.getByTestId('lint-detail');
      const statuses = Array.from(detail.querySelectorAll('.disc-lint-item'))
        .map(el => el.getAttribute('data-status'))
        .filter(Boolean);
      expect(statuses).toContain('out_of_bounds');
      expect(statuses).toContain('not_found');
    });
  });

  // 0.8.8 — niveau-1.5 soft tier: inline anchors that didn't resolve →
  // honest "unverified" (soft amber), NOT red "fabricated", NOT silent.
  describe('unverified (soft amber) tier', () => {
    const unverifiedReport = (): LintReport => ({
      unsourced_count: 0,
      flagged_spans: [],
      fabricated_count: 0,
      unverified_count: 1,
      sources: [
        { raw: 'src/ghost.rs:1', kind: 'file', status: 'unchecked', detail: "inline anchor (couldn't verify): file not found: src/ghost.rs" },
      ],
    });

    it('renders a soft-amber "unverified" pill when an inline anchor did not resolve', () => {
      renderBubble(unverifiedReport());
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('unverified');
      expect(pill.textContent).toContain('1');
      expect(pill.textContent).toContain('disc.lintUnverified');
    });

    it('unverified ranks BELOW fabricated and unsourced (red/amber win)', () => {
      renderBubble({ ...unverifiedReport(), fabricated_count: 1 });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('fabricated');
      renderBubble({ ...unverifiedReport(), unsourced_count: 1 });
      // second render: query the latest pill — both mounted; assert at least one unsourced
      expect(screen.getAllByTestId('lint-pill').some(p => p.getAttribute('data-severity') === 'unsourced')).toBe(true);
    });

    it('unverified ranks ABOVE verified (a mixed reply flags the soft signal)', () => {
      renderBubble({
        ...unverifiedReport(),
        sources: [
          { raw: 'src/foo.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'src/ghost.rs:1', kind: 'file', status: 'unchecked', detail: "inline anchor (couldn't verify): not found" },
        ],
      });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('unverified');
    });

    it('drawer lists the unverified anchor in its own group + the verified one too', () => {
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        fabricated_count: 0,
        unverified_count: 1,
        sources: [
          { raw: 'src/foo.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'src/ghost.rs:1', kind: 'file', status: 'unchecked', detail: "inline anchor (couldn't verify): not found" },
        ],
      });
      fireEvent.click(screen.getByTestId('lint-pill'));
      const unv = screen.getByTestId('lint-unverified-group');
      expect(unv.textContent).toContain('src/ghost.rs:1');
      expect(unv.textContent).toContain('disc.lintUnverifiedTitle');
      // verified group still shown on a mixed report
      expect(screen.getByTestId('lint-verified-group').textContent).toContain('src/foo.rs:1');
      // drawer severity echoes the pill
      expect(screen.getByTestId('lint-detail').getAttribute('data-severity')).toBe('unverified');
    });
  });

  // 0.8.8 Option B — neutral "unverifiable" tier: cited but uncheckable
  // (URL / user-confirmed / inferred). Surfaced, never hidden.
  describe('unverifiable (neutral) tier — Option B', () => {
    const uncheckedReport = (): LintReport => ({
      unsourced_count: 0,
      flagged_spans: [],
      fabricated_count: 0,
      unverified_count: 0,
      sources: [
        { raw: 'url: https://example.com', kind: 'url', status: 'unchecked', detail: 'URL/API — not network-checked (SSRF-safe)' },
      ],
    });

    it('renders a neutral "unverifiable" pill when only uncheckable sources are cited', () => {
      renderBubble(uncheckedReport());
      const pill = screen.getByTestId('lint-pill');
      expect(pill.getAttribute('data-severity')).toBe('unchecked');
      expect(pill.textContent).toContain('1');
      expect(pill.textContent).toContain('disc.lintUnverifiable');
    });

    it('drawer lists unverifiable sources under their own group', () => {
      renderBubble(uncheckedReport());
      fireEvent.click(screen.getByTestId('lint-pill'));
      const group = screen.getByTestId('lint-unverifiable-group');
      expect(group.textContent).toContain('https://example.com');
      expect(group.textContent).toContain('disc.lintUnverifiableTitle');
    });

    it('unverifiable ranks BELOW verified (green wins, URL still listed in drawer)', () => {
      renderBubble({
        ...uncheckedReport(),
        sources: [
          { raw: 'src/foo.rs:1', kind: 'file', status: 'verified', detail: 'file exists' },
          { raw: 'url: https://example.com', kind: 'url', status: 'unchecked', detail: 'URL/API — not network-checked' },
        ],
      });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('verified');
      fireEvent.click(screen.getByTestId('lint-pill'));
      // both groups present: the URL is still surfaced honestly
      expect(screen.getByTestId('lint-verified-group').textContent).toContain('src/foo.rs:1');
      expect(screen.getByTestId('lint-unverifiable-group').textContent).toContain('https://example.com');
    });

    it('an inline "couldn\'t verify" source does NOT land in the unverifiable group', () => {
      // It's the unverified (soft amber) tier, not the neutral one.
      renderBubble({
        unsourced_count: 0,
        flagged_spans: [],
        fabricated_count: 0,
        unverified_count: 1,
        sources: [
          { raw: 'src/ghost.rs:1', kind: 'file', status: 'unchecked', detail: "inline anchor (couldn't verify): not found" },
        ],
      });
      expect(screen.getByTestId('lint-pill').getAttribute('data-severity')).toBe('unverified');
      fireEvent.click(screen.getByTestId('lint-pill'));
      expect(screen.queryByTestId('lint-unverifiable-group')).toBeNull();
      expect(screen.getByTestId('lint-unverified-group')).not.toBeNull();
    });
  });
});
