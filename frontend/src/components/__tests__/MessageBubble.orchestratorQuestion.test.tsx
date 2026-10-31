/**
 * KT-1433 — a decision card Kronn's orchestrator posts (the Docker elevation)
 * is shown at once and finds its durable question. Folded behind "details"
 * and rendered without its message id, it read "Arbitrage introuvable".
 */
import { afterEach, describe, it, expect, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import type { DiscussionMessage, DiscussionQuestion } from '../../types/generated';

const MESSAGE_ID = 'orch-elevation:docker:exec-1:0';

const { questionsMock } = vi.hoisted(() => ({ questionsMock: vi.fn() }));
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<Record<string, object>>('../../lib/api');
  return {
    ...real,
    config: { getUiLanguage: vi.fn().mockResolvedValue('fr') },
    discussions: { ...real.discussions, questions: questionsMock, answerQuestion: vi.fn() },
  };
});
vi.mock('../../lib/importantMessages', async () => {
  const real = await vi.importActual<object>('../../lib/importantMessages');
  return {
    ...real,
    refreshImportantMessages: vi.fn(),
    useImportantMessages: () => ({ items: [], totalAll: 0, loaded: true }),
  };
});

import { MessageBubble } from '../MessageBubble';
import { resetDiscussionQuestions } from '../../lib/discussionQuestions';

const question: DiscussionQuestion = {
  id: `question:${MESSAGE_ID}:0`, discussion_id: 'disc-test', source_message_id: MESSAGE_ID,
  fence_index: 0, key: 'kronn-elevation-docker', question: 'Autoriser Docker pour les validations ?',
  context: null, td_errors: null,
  options: [
    { id: 'deny', label: 'Refuser', description: null },
    { id: 'discussion', label: 'Autoriser pour cette discussion', description: null },
    { id: 'project', label: 'Autoriser pour le projet', description: null },
  ],
  items: [], multiple: false, recommended_option_ids: [], task_ref: null,
  requester_workflow_step: null, resume: null, state: 'pending', answer: null,
  created_at: '2026-10-23T00:00:00Z', updated_at: '2026-10-23T00:00:00Z',
};

function orchestratorMessage(content: string, id = MESSAGE_ID): DiscussionMessage {
  return {
    id, role: 'User', channel: 'main', content, agent_type: null,
    timestamp: new Date().toISOString(), tokens_used: 0, auth_mode: null, model_tier: null,
    author_pseudo: 'Orchestrateur', author_avatar_email: null, kind: 'orchestration',
  } as DiscussionMessage;
}

const noopProps = {
  idx: 0, isLastUser: false, isLastAgent: false, isEditing: false, isCopied: false,
  isTtsActive: false, ttsState: 'idle', isExpandedSummary: false, prevUserTs: null,
  defaultAgent: 'ClaudeCode' as const, summaryCache: null, language: 'fr', sending: false,
  editingText: '', hasFullAccess: false, onCopy: () => {}, onTts: () => {},
  onEditStart: () => {}, onEditCancel: () => {}, onEditSubmit: () => {},
  onEditTextChange: () => {}, onRetry: () => {}, onExpandSummary: () => {},
  onNavigate: () => {}, discussionId: 'disc-test', t: (key: string) => key,
};

afterEach(() => { cleanup(); resetDiscussionQuestions(); });

describe('orchestrator decision cards (KT-1433)', () => {
  it('shows the elevation card expanded, bound to its question', async () => {
    questionsMock.mockResolvedValue({ questions: [question], pending_count: 1 });
    const content = [
      '**Élévation demandée — Docker**',
      '',
      '```kronn-question',
      JSON.stringify({ version: 1, key: 'kronn-elevation-docker', question: question.question, options: question.options }),
      '```',
    ].join('\n');
    render(<I18nProvider><MessageBubble {...noopProps} msg={orchestratorMessage(content)} /></I18nProvider>);

    expect(await screen.findByTestId(`disc-question-${question.id}`)).toBeVisible();
    expect(screen.queryByText('disc.question.missing')).toBeNull();
    expect(screen.queryByRole('button', { name: 'disc.orchestratorShowDetails' })).toBeNull();
  });

  // KT-1492 — every elevation card keeps refuse / this conversation / this
  // project and names the global trust setting, with a link to Settings › Trust levels.
  it('keeps the three answers of an elevation card and links the global trust setting', async () => {
    questionsMock.mockResolvedValue({ questions: [question], pending_count: 1 });
    const content = [
      '**Élévation demandée — Docker**',
      '',
      '```kronn-question',
      JSON.stringify({ version: 1, key: 'kronn-elevation-docker', question: question.question, options: question.options }),
      '```',
    ].join('\n');
    render(<I18nProvider><MessageBubble {...noopProps} msg={orchestratorMessage(content)} /></I18nProvider>);

    const card = await screen.findByTestId(`disc-question-${question.id}`);
    for (const option of question.options) {
      expect(card).toHaveTextContent(option.label);
    }
    const hint = await screen.findByTestId('disc-question-trust-hint');
    expect(card).toContainElement(hint);
    expect(hint).toHaveTextContent(/Niveaux de confiance/);
    const link = screen.getByTestId('disc-question-trust-link');
    expect(link.getAttribute('href')).toBe('/config#settings-trust');
  });

  // A plain click stays in the app (no document reload); the modified clicks
  // and the address stay the browser's.
  it('opens the trust settings in this tab through the app navigation on a plain click', async () => {
    questionsMock.mockResolvedValue({ questions: [question], pending_count: 1 });
    const onNavigate = vi.fn();
    const content = [
      '```kronn-question',
      JSON.stringify({ version: 1, key: 'kronn-elevation-docker', question: question.question, options: question.options }),
      '```',
    ].join('\n');
    render(<I18nProvider><MessageBubble {...noopProps} onNavigate={onNavigate} msg={orchestratorMessage(content)} /></I18nProvider>);

    const link = await screen.findByTestId('disc-question-trust-link');
    expect(link.getAttribute('href')).toBe('/config#settings-trust');
    const plain = fireEvent.click(link);
    expect(plain, 'the browser does not follow the link').toBe(false);
    expect(onNavigate).toHaveBeenCalledWith('settings', { scrollTo: 'settings-trust' });

    onNavigate.mockClear();
    const modified = fireEvent.click(link, { ctrlKey: true });
    expect(modified, 'a Ctrl-click is the browser\'s own').toBe(true);
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it('adds the same line to an elevation card an agent posts, and to no other card', async () => {
    const agentCard: DiscussionQuestion = { ...question, key: 'kronn-elevation-code-write', source_message_id: 'agent-msg-1', id: 'question:agent-msg-1:0' };
    const plain: DiscussionQuestion = { ...question, key: 'which-db', source_message_id: 'agent-msg-2', id: 'question:agent-msg-2:0' };
    questionsMock.mockResolvedValue({ questions: [agentCard, plain], pending_count: 2 });
    const fence = (key: string) => [
      '```kronn-question',
      JSON.stringify({ version: 1, key, question: 'Q ?', options: question.options }),
      '```',
    ].join('\n');
    const { rerender } = render(<I18nProvider><MessageBubble {...noopProps} msg={orchestratorMessage(fence(agentCard.key), 'agent-msg-1')} /></I18nProvider>);
    expect(await screen.findByTestId('disc-question-trust-hint')).toBeInTheDocument();

    rerender(<I18nProvider><MessageBubble {...noopProps} msg={orchestratorMessage(fence(plain.key), 'agent-msg-2')} /></I18nProvider>);
    await screen.findByTestId(`disc-question-${plain.id}`);
    expect(screen.queryByTestId('disc-question-trust-hint')).toBeNull();
  });

  it('keeps an ordinary orchestrator notice folded behind its details', () => {
    questionsMock.mockResolvedValue({ questions: [], pending_count: 0 });
    render(
      <I18nProvider>
        <MessageBubble {...noopProps} msg={orchestratorMessage('**Revue demandée — KT-1**\n\nLe worker a livré.', 'orch-review-request:x:0')} />
      </I18nProvider>,
    );
    expect(screen.getByRole('button', { name: 'disc.orchestratorShowDetails' })).toBeInTheDocument();
  });
});
