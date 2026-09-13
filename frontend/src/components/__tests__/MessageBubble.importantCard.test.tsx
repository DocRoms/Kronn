/**
 * KT-619 — a card published by a HUMAN renders on the human's own message.
 *
 * Found by the isolated E2E, not by reading: the durable id was handed to the
 * message body only for `Agent` messages. That was true while an agent was the
 * only thing that could publish a durable fence. Since the human composer can,
 * the card sat in the database, the counter counted it, and the bubble directly
 * underneath said it had been refused — the two halves of the same screen
 * disagreeing about the same row.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { DiscussionMessage, ImportantMessage } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

const MESSAGE_ID = 'msg-human-publisher';

const card: ImportantMessage = {
  id: 'card-1',
  discussion_id: 'disc-test',
  message_id: MESSAGE_ID,
  category: 'decision',
  schema_version: 1,
  dedup_key: 'unit-human-card',
  title: 'Le socle de publication part en 0.13.0',
  highlight: 'La carte appartient au message qui la porte.',
  context: null,
  impact: 'Aucune install ne publie avant enrôlement.',
  action_required: { required: false, action: null, owner: null, due: null },
  references: {
    task_ref: null,
    dod_id: null,
    execution_id: null,
    agent: null,
    commit: null,
    artifact: null,
  },
  author_kind: 'human',
  author_label: 'Romu',
  source_kind: null,
  source_id: null,
  created_at: '2026-09-12T08:00:00Z',
  sort_order: 2,
} as ImportantMessage;

vi.mock('../../lib/importantMessages', async () => {
  const real = await vi.importActual<object>('../../lib/importantMessages');
  return {
    ...real,
    refreshImportantMessages: vi.fn(),
    useImportantMessages: () => ({ items: [card], totalAll: 1, loaded: true }),
  };
});

import { MessageBubble } from '../MessageBubble';

function fenceMessage(role: DiscussionMessage['role']): DiscussionMessage {
  const spec = {
    version: 1,
    category: 'decision',
    dedup_key: 'unit-human-card',
    title: card.title,
    highlight: card.highlight,
    impact: card.impact,
    action_required: { required: false },
  };
  return {
    id: MESSAGE_ID,
    role,
    channel: 'main',
    content: ['Point de passage.', '', '```kronn-important', JSON.stringify(spec), '```'].join('\n'),
    agent_type: null,
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: 'Romu',
    author_avatar_email: null,
  } as DiscussionMessage;
}

const noopProps = {
  idx: 0,
  isLastUser: false,
  isLastAgent: false,
  isEditing: false,
  isCopied: false,
  isTtsActive: false,
  ttsState: 'idle',
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
  onNavigate: () => {},
  discussionId: 'disc-test',
  t: (key: string) => key,
};

describe('MessageBubble — an important card on its own message', () => {
  for (const role of ['User', 'Agent'] as const) {
    it(`renders the recorded card on a ${role} message`, () => {
      render(
        <I18nProvider>
          <MessageBubble {...noopProps} msg={fenceMessage(role)} />
        </I18nProvider>,
      );

      expect(screen.getByText(card.highlight)).toBeInTheDocument();
      // The refusal notice is the other branch of the same component. Seeing it
      // here would mean the card exists and the message cannot find it.
      expect(screen.queryByText('disc.important.missingTitle')).toBeNull();
    });
  }
});
