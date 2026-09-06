import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
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

  it('says nothing about a citation nobody checked', () => {
    renderMessage('Sans rapport [src: url: https://example.com/x].');
    expect(screen.getByTestId('source-citation-chip')).toHaveAttribute('data-state', 'unknown');
  });

  /// Turning ordinary prose into a chip by accident would be worse than
  /// leaving one marker unread, so the pattern stays narrow.
  it('leaves a message with no citation exactly as it was', () => {
    renderMessage('Un texte qui parle de src: file: sans crochets du tout.');
    expect(screen.queryByTestId('source-citation-chip')).toBeNull();
    expect(screen.getByText(/sans crochets du tout/)).toBeInTheDocument();
  });
});
