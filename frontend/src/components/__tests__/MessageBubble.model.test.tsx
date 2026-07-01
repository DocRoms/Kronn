/**
 * 0.8.10 — per-message concrete model display.
 *
 * A discussion can switch models mid-thread (per-step/QP override, or a changed
 * OllamaCard default), so each agent message shows the CONCRETE model it ran on
 * ("Ollama · qwen3:32b"), falling back to the tier when the model wasn't
 * recorded (legacy rows / provider-default runs with no explicit flag).
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage } from '../../types/generated';

function makeAgentMessage(over: Partial<DiscussionMessage>): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'Agent',
    content: 'Some answer.',
    agent_type: 'Ollama',
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    model: null,
    author_pseudo: null,
    author_avatar_email: null,
    lint_report: null,
    ...over,
  };
}

const baseProps = {
  idx: 0, isLastUser: false, isLastAgent: false, isEditing: false, isCopied: false,
  isTtsActive: false, ttsState: 'idle' as const, isExpandedSummary: false, prevUserTs: null,
  defaultAgent: 'Ollama' as const, summaryCache: null, language: 'fr', sending: false,
  editingText: '', hasFullAccess: false, onCopy: () => {}, onTts: () => {}, onEditStart: () => {},
  onEditCancel: () => {}, onEditSubmit: () => {}, onEditTextChange: () => {}, onRetry: () => {},
  onExpandSummary: () => {}, discussionId: 'disc-test', projectId: null, onNavigate: () => {},
  t: (key: string) => key,
};

function renderBubble(over: Partial<DiscussionMessage>) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} msg={makeAgentMessage(over)} />
    </I18nProvider>
  );
}

describe('MessageBubble — per-message model', () => {
  it('shows the concrete model when present', () => {
    renderBubble({ model: 'qwen3:32b', model_tier: null });
    expect(screen.getByText(/qwen3:32b/)).toBeTruthy();
  });

  it('prefers the concrete model over the tier', () => {
    renderBubble({ model: 'qwen3:8b', model_tier: 'reasoning' });
    // The header shows the model, not the raw tier word, as its primary label.
    expect(screen.getByText(/qwen3:8b/)).toBeTruthy();
  });

  it('falls back to the tier when no concrete model is recorded', () => {
    renderBubble({ model: null, model_tier: 'economy' });
    // No model → the tier is surfaced (economy label / badge).
    expect(screen.queryByText(/qwen3/)).toBeNull();
    expect(screen.getAllByText(/economy|disc\.tier\.economy/).length).toBeGreaterThan(0);
  });

  it('shows neither when both are absent (legacy row)', () => {
    renderBubble({ model: null, model_tier: null });
    expect(screen.queryByText(/qwen3/)).toBeNull();
  });
});
