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
    expect(detail.textContent).not.toContain('real.rs:1');
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

  it('green pill suppressed when all sources are only `unchecked` (no real verification)', () => {
    // Reply citing only urls / users / inferred → none mechanically verified.
    // We refuse to award a green chip for citations Kronn never validated.
    renderBubble({
      unsourced_count: 0,
      flagged_spans: [],
      sources: [
        { raw: 'https://example.com/doc', kind: 'url', status: 'unchecked', detail: 'URL — not network-checked' },
        { raw: 'user:rom:2026-05-28: disc-1', kind: 'user', status: 'unchecked', detail: 'user-declared' },
      ],
      fabricated_count: 0,
    });
    expect(screen.queryByTestId('lint-pill')).toBeNull();
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
});
