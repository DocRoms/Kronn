import { describe, it, expect } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage, LintReport } from '../../types/generated';

/// KT-609 — Romuald, reading the room: « on a pas moyen de rendre ça joli dans
/// la conversation ? pas un truc que les personnes ne comprendront pas ? »
///
/// The markers are already verified server-side and already counted by a
/// footer pill. What was left in the prose was the notation itself.

function agentMessage(content: string, lint: LintReport | null = null): DiscussionMessage {
  return {
    id: 'msg-src',
    role: 'Agent',
    channel: 'main',
    content,
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

const renderMessage = (content: string, lint: LintReport | null = null) => render(
  <I18nProvider>
    <MessageBubble {...baseProps} msg={agentMessage(content, lint)} />
  </I18nProvider>,
);

describe('MessageBubble — inline source citations', () => {
  it('reads a file citation as a chip and keeps the sentence around it', () => {
    renderMessage('La cause est là [src: file: frontend/src/lib/api.ts:1014] et nulle part ailleurs.');

    const chip = screen.getByTestId('source-citation-chip');
    expect(chip).toHaveTextContent('api.ts:1014');
    expect(chip).toHaveAttribute('data-kind', 'file');
    // KT-609 — the kind is the one word the reader has to understand, so it is
    // translated: `code-comment` says nothing to someone who has not read the
    // anti-hallucination spec.
    expect(chip).toHaveTextContent('fichier');
    // A citation is evidence: the exact reference has to stay reachable.
    expect(chip).toHaveAttribute('title', '[src: file: frontend/src/lib/api.ts:1014]');
    expect(screen.getByText(/et nulle part ailleurs/)).toBeInTheDocument();
  });

  /// Three markers end to end is what turned a sentence into notation.
  it('reads a run of citations one chip at a time', () => {
    renderMessage('Preuves [src: file: a/b.rs:1] [src: commit: 0593f12a91bae2ece513cd] fin.');
    const chips = screen.getAllByTestId('source-citation-chip');
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveTextContent('b.rs:1');
    expect(chips[1]).toHaveTextContent('0593f12');
  });

  /// The backend already opened the file and checked the lines. A chip that
  /// claimed every citation was fine would be worse than the raw marker.
  it('carries the verdict the backend reached, and admits when there is none', () => {
    renderMessage(
      'Vérifiée [src: file: a.rs:1] et inventée [src: file: nope.rs:9].',
      {
        unsourced_count: 0,
        flagged_spans: [],
        fabricated_count: 1,
        sources: [
          { raw: 'file: a.rs:1', kind: 'file', status: 'verified', detail: 'ok' },
          { raw: 'file: nope.rs:9', kind: 'file', status: 'not_found', detail: 'introuvable' },
        ],
      } as unknown as LintReport,
    );

    const [verified, suspect] = screen.getAllByTestId('source-citation-chip');
    expect(verified).toHaveAttribute('data-state', 'verified');
    expect(suspect).toHaveAttribute('data-state', 'suspect');
    // And the reason the backend gave travels with it.
    expect(suspect.getAttribute('title')).toContain('introuvable');
  });

  it('falls back to the raw kind when nobody wrote a label for it', () => {
    renderMessage('Bizarre [src: futur-type: quelque chose].');
    // Not a translation key printed at the reader — the kind itself.
    expect(screen.getByTestId('source-citation-chip')).toHaveTextContent('futur-type');
  });

  it('says nothing about a citation nobody checked', () => {
    renderMessage('Sans rapport [src: url: https://example.com/x].');
    expect(screen.getByTestId('source-citation-chip')).toHaveAttribute('data-state', 'unknown');
  });

  it('renders every URL in consecutive autolinked citation markers', () => {
    renderMessage('Preuves [src: url: https://example.com/one] [src: url: https://example.org/two] fin.');
    const chips = screen.getAllByTestId('source-citation-chip');
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveAttribute('title', '[src: url: https://example.com/one]');
    expect(chips[1]).toHaveAttribute('title', '[src: url: https://example.org/two]');
    expect(screen.getByText(/fin\./)).toBeInTheDocument();
  });

  /// Turning ordinary prose into a chip by accident would be worse than
  /// leaving one marker unread, so the pattern stays narrow.
  it('leaves a message with no citation exactly as it was', () => {
    renderMessage('Un texte qui parle de src: file: sans crochets du tout.');
    expect(screen.queryByTestId('source-citation-chip')).toBeNull();
    expect(screen.getByText(/sans crochets du tout/)).toBeInTheDocument();
  });
});

/// KT-1184 — a file that exists only in an unmerged lane worktree used to show
/// the same red "not found" as a file that exists nowhere.
describe('MessageBubble — citation origin (checkout, lane, not found)', () => {
  const threeStates = {
    unsourced_count: 0,
    flagged_spans: [],
    fabricated_count: 1,
    unverified_count: 0,
    sources: [
      {
        raw: 'file: src/main.rs:2', kind: 'file', status: 'verified',
        detail: 'lines 2-2 within 5 lines', origin: { kind: 'checkout' },
      },
      {
        raw: 'file: frontend/src/lib/templateMerge.ts:4', kind: 'file', status: 'verified',
        detail: 'lines 4-4 within 10 lines — found in worktree 0151-kt1181 (not in the checkout)',
        origin: { kind: 'worktree', name: '0151-kt1181' },
      },
      {
        raw: 'file: src/ghost.rs:1', kind: 'file', status: 'not_found',
        detail: "file not found: src/ghost.rs — not in the checkout nor in the project's 3 active worktree(s)",
      },
    ],
  } as unknown as LintReport;

  const text = 'Ici [src: file: src/main.rs:2], là [src: file: frontend/src/lib/templateMerge.ts:4] '
    + 'et [src: file: src/ghost.rs:1].';

  it('gives each of the three states its own chip and tooltip', () => {
    renderMessage(text, threeStates);
    const [checkout, lane, missing] = screen.getAllByTestId('source-citation-chip');

    expect(checkout).toHaveAttribute('data-state', 'verified');
    expect(checkout.getAttribute('title')).toBe(
      '[src: file: src/main.rs:2]\nVérifié dans le checkout principal\nlines 2-2 within 5 lines',
    );

    expect(lane).toHaveAttribute('data-state', 'lane');
    expect(lane.getAttribute('title')).toContain(
      'Présent dans le worktree 0151-kt1181, pas encore intégré au checkout',
    );
    expect(lane).toHaveAttribute(
      'aria-label',
      'fichier templateMerge.ts:4 — Présent dans le worktree 0151-kt1181, pas encore intégré au checkout',
    );
    expect(screen.getByTestId('source-citation-lane')).toHaveTextContent('lane 0151-kt1181');

    expect(missing).toHaveAttribute('data-state', 'suspect');
    expect(missing.getAttribute('title')).toContain(
      'Introuvable : ni dans le checkout, ni dans un worktree actif du projet',
    );
    // Only the lane chip carries a lane tag.
    expect(screen.getAllByTestId('source-citation-lane')).toHaveLength(1);
  });

  it('reads a stored row without an origin as the checkout', () => {
    renderMessage('Ancien [src: file: a.rs:1].', {
      unsourced_count: 0,
      flagged_spans: [],
      fabricated_count: 0,
      sources: [{ raw: 'file: a.rs:1', kind: 'file', status: 'verified', detail: 'ok' }],
    } as unknown as LintReport);
    const chip = screen.getByTestId('source-citation-chip');
    expect(chip).toHaveAttribute('data-state', 'verified');
    expect(chip.getAttribute('title')).toContain('Vérifié dans le checkout principal');
    expect(screen.queryByTestId('source-citation-lane')).toBeNull();
  });

  it('counts lane files apart in the footer and lists them with their worktree', () => {
    renderMessage(text, threeStates);
    // Red headline (one file exists nowhere), then the good news split in two.
    expect(screen.getByTestId('lint-pill')).toHaveAttribute('data-severity', 'fabricated');
    expect(screen.getByTestId('lint-pill-verified')).toHaveTextContent('1');
    const lanePill = screen.getByTestId('lint-pill-lane');
    expect(lanePill).toHaveAttribute('data-severity', 'lane');
    // The footer takes the bubble's `t` prop, an identity here: keys show.
    expect(lanePill).toHaveTextContent('1 disc.lintLane');

    fireEvent.click(lanePill);
    const laneGroup = screen.getByTestId('lint-lane-group');
    expect(laneGroup).toHaveTextContent('disc.lintLaneTitle');
    expect(laneGroup).toHaveTextContent('disc.srcOrigin.laneTag');
    expect(laneGroup).toHaveTextContent('found in worktree 0151-kt1181');
    expect(laneGroup).toHaveTextContent('templateMerge.ts:4');
    expect(screen.getByTestId('lint-verified-group')).not.toHaveTextContent('templateMerge');
  });

  it('shows a neutral lane headline when lane files are the only signal', () => {
    renderMessage('Lot [src: file: frontend/src/lib/templateMerge.ts:4].', {
      ...threeStates,
      fabricated_count: 0,
      sources: [threeStates.sources[1]],
    } as unknown as LintReport);
    const pill = screen.getByTestId('lint-pill');
    expect(pill).toHaveAttribute('data-severity', 'lane');
    expect(pill).toHaveAttribute('title', 'disc.lintPillHintLane');
    expect(screen.queryByTestId('lint-pill-verified')).toBeNull();
    expect(screen.queryByTestId('lint-pill-lane')).toBeNull();
  });
});
