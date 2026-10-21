/**
 * KT-1401 — a `kronn-question` fence Kronn could not record is shown as an
 * invalid card with its reason, never as a card the human cannot answer.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
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
    channel: 'main',
    content: 'Voici ma question.',
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
  // Echo the arguments so the test sees the reason the notice interpolates.
  t: (key: string, ...args: unknown[]) => (args.length ? `${key}|${args.join('|')}` : key),
};

function renderBubble(lint: LintReport | null) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} msg={makeAgentMessage(lint)} />
    </I18nProvider>,
  );
}

const report = (invalid_cards?: LintReport['invalid_cards']): LintReport => ({
  unsourced_count: 0,
  flagged_spans: [],
  sources: [],
  fabricated_count: 0,
  unverified_count: 0,
  invalid_cards,
});

describe('MessageBubble — invalid arbitration card notice', () => {
  it('shows each invalid card with its key and reason, always visible', () => {
    renderBubble(report([
      { key: 'ew7697-front-tools-ticket', reason: 'the fence is not a valid question: unknown field `run_command`' },
      { reason: '`version` must be the number 1' },
    ]));
    const notices = screen.getAllByTestId('invalid-card-notice');
    expect(notices).toHaveLength(2);
    expect(notices[0].textContent).toContain('disc.invalidCard');
    expect(notices[0].textContent).toContain('ew7697-front-tools-ticket');
    expect(notices[0].textContent).toContain('unknown field `run_command`');
    expect(notices[1].textContent).toContain('`version` must be the number 1');
    expect(notices[0].getAttribute('role')).toBe('alert');
  });

  it('shows nothing when every fence became a card', () => {
    renderBubble(report([]));
    expect(screen.queryByTestId('invalid-card-notice')).toBeNull();
    renderBubble(report(undefined));
    expect(screen.queryByTestId('invalid-card-notice')).toBeNull();
    renderBubble(null);
    expect(screen.queryByTestId('invalid-card-notice')).toBeNull();
  });
});
