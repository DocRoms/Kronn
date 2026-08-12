// KT-251 — a salvaged fragment must not read as a real reply.
//
// A user reported "j'ai trois réponses 😱" on one question: two of them were the
// truncated starts of an answer whose agent was killed mid-sentence, then
// retried. Nothing distinguished them, so they looked like several agents
// replying.
//
// Folded, NOT hidden: the fragment is real history and may hold reasoning the
// retry never repeated. So the tests pin both — collapsed by default, and
// reachable in one click.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { MessageBubble } from '../MessageBubble';
import { I18nProvider } from '../../lib/I18nContext';
import { ThemeProvider } from '../../lib/ThemeContext';
import type { DiscussionMessage } from '../../types/generated';

const FRAGMENT_BODY = 'I was explaining the cache when I was cut';

function message(over: Partial<DiscussionMessage> = {}): DiscussionMessage {
  return {
    id: 'm-1',
    role: 'Agent',
    channel: 'Main',
    content: FRAGMENT_BODY,
    agent_type: 'ClaudeCode',
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    cost_usd: null,
    author_pseudo: null,
    author_avatar_email: null,
    source_msg_id: null,
    duration_ms: null,
    target_agent: null,
    reply_to_message_id: null,
    ...over,
  } as DiscussionMessage;
}

function renderBubble(msg: DiscussionMessage) {
  return render(
    <ThemeProvider>
      <I18nProvider>
        <MessageBubble
          msg={msg}
          idx={0}
          isLastUser={false}
          isLastAgent={false}
          isEditing={false}
          isCopied={false}
          isTtsActive={false}
          ttsState="idle"
          isExpandedSummary={false}
          prevUserTs={null}
          defaultAgent="ClaudeCode"
          summaryCache={null}
          language="fr"
          sending={false}
          editingText=""
          hasFullAccess
          // Identity translator: the assertions target the KEY, not the French
          // wording, so a reworded label does not break the behaviour test.
          t={(key: string) => key}
          onCopy={vi.fn()}
          onTts={vi.fn()}
          onEditStart={vi.fn()}
          onEditCancel={vi.fn()}
          onEditSubmit={vi.fn()}
          onEditTextChange={vi.fn()}
          onRetry={vi.fn()}
          onExpandSummary={vi.fn()}
          onNavigate={vi.fn()}
        />
      </I18nProvider>
    </ThemeProvider>,
  );
}

describe('MessageBubble — interrupted fragment', () => {
  it('folds a fragment instead of showing it as a reply', () => {
    renderBubble(message({ recovered_partial: true }));
    expect(screen.queryByText(new RegExp(FRAGMENT_BODY))).toBeNull();
    expect(screen.getByText(/interruptedFragment/)).toBeTruthy();
  });

  it('reveals the fragment in one click, because it is real history', () => {
    // Hiding it outright would lose reasoning the retry never repeated.
    renderBubble(message({ recovered_partial: true }));
    fireEvent.click(screen.getByText(/interruptedFragment/));
    expect(screen.getByText(new RegExp(FRAGMENT_BODY))).toBeTruthy();
  });

  it('leaves an ordinary reply untouched', () => {
    // The fold must be the exception. Applying it to every agent message would
    // bury the answers.
    renderBubble(message({ recovered_partial: false }));
    expect(screen.getByText(new RegExp(FRAGMENT_BODY))).toBeTruthy();
    expect(screen.queryByText(/interruptedFragment/)).toBeNull();
  });

  it('treats a missing flag as an ordinary reply', () => {
    // Older rows have no flag at all; defaulting to "fragment" would fold the
    // entire history of a discussion.
    renderBubble(message());
    expect(screen.getByText(new RegExp(FRAGMENT_BODY))).toBeTruthy();
  });
});
