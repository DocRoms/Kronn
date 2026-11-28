/**
 * KT-1902 — inventory of what each message variant of the discussion page
 * offers. Written against the code before the Message component existed and
 * kept after it: every action and indicator listed here must survive. Only
 * where an action moved (its order, the « … » menu) was a query adapted.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    config: {
      getUiLanguage: vi.fn().mockResolvedValue('fr'),
      getServerConfig: vi.fn().mockResolvedValue({ pseudo: null, avatar_email: null }),
      getAgentAccess: vi.fn().mockResolvedValue(null),
    },
  };
});

import { MessageBubble, type MessageBubbleProps } from '../MessageBubble';
import { WorkflowStepActivityBubble } from '../WorkflowStepActivityBubble';
import { parseModelErrorEvent } from '../../lib/modelErrorEvent';
import { DELETED_MESSAGE_MARKER } from '../../lib/messageContent';
import type { ActiveWorkflowStep, DiscussionMessage, LintReport } from '../../types/generated';

function message(overrides: Partial<DiscussionMessage> = {}): DiscussionMessage {
  return {
    id: 'msg-inventory',
    role: 'Agent',
    channel: 'main',
    content: 'An answer.',
    agent_type: 'ClaudeCode',
    timestamp: '2026-11-20T10:30:00.000Z',
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    model: null,
    author_pseudo: null,
    author_avatar_email: null,
    lint_report: null,
    ...overrides,
  };
}

const handlers = () => ({
  onCopy: vi.fn(),
  onTts: vi.fn(),
  onEditStart: vi.fn(),
  onEditCancel: vi.fn(),
  onEditSubmit: vi.fn(),
  onEditTextChange: vi.fn(),
  onRetry: vi.fn(),
  onRetryAgentDispatch: vi.fn(),
  onExpandSummary: vi.fn(),
  onNavigate: vi.fn(),
  onReply: vi.fn(),
  onReplyNavigate: vi.fn(),
  onDelete: vi.fn(),
});

function renderMessage(msg: DiscussionMessage, props: Partial<MessageBubbleProps> = {}) {
  const fns = handlers();
  const base: Omit<MessageBubbleProps, 'msg'> = {
    idx: 1,
    isLastUser: false,
    isLastAgent: false,
    isEditing: false,
    isCopied: false,
    isTtsActive: false,
    ttsState: 'idle',
    isExpandedSummary: false,
    prevUserTs: null,
    defaultAgent: 'ClaudeCode',
    summaryCache: null,
    language: 'fr',
    sending: false,
    editingText: '',
    hasFullAccess: false,
    discussionId: 'disc-inventory',
    projectId: null,
    t: (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key),
    ...fns,
  };
  const view = render(
    <I18nProvider>
      <MessageBubble {...base} {...props} msg={msg} />
    </I18nProvider>,
  );
  const bubble = view.container.querySelector('.disc-msg-bubble') as HTMLElement;
  return { ...view, bubble, fns };
}

const lint = (overrides: Partial<LintReport> = {}): LintReport => ({
  unsourced_count: 0,
  fabricated_count: 0,
  unverified_count: 0,
  sources: [
    { raw: '[src: file: a.rs:1]', kind: 'file', status: 'verified', detail: 'ok' },
  ],
  flagged_spans: [],
  invalid_cards: [],
  ...overrides,
} as LintReport);

describe('KT-1902 inventory — user message', () => {
  it('names its human author, where it went, its id and time, and offers Reply, Copy, Edit and Delete', () => {
    const { bubble, fns } = renderMessage(
      message({ role: 'User', agent_type: null, author_pseudo: 'Alex', content: 'Please look' }),
      { isLastUser: true },
    );
    expect(within(bubble).getByText('Alex')).toBeInTheDocument();
    expect(within(bubble).getByText('· humain')).toBeInTheDocument();
    expect(within(bubble).getByTestId('message-routing-receipt')).toBeInTheDocument();
    expect(within(bubble).getByRole('button', { name: /disc\.idPillTooltip/ })).toBeInTheDocument();
    expect(bubble.querySelector('.disc-msg-time')).not.toBeNull();
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.reply' }));
    expect(fns.onReply).toHaveBeenCalledTimes(1);
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.editResend' }));
    expect(fns.onEditStart).toHaveBeenCalledWith('msg-inventory', 'Please look');
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.deleteMessage' }));
    expect(fns.onDelete).toHaveBeenCalledTimes(1);
    expect(within(bubble).queryByRole('button', { name: 'disc.retryResponse' })).toBeNull();
  });

  it('offers Edit on the last user message only, and never while a reply is running', () => {
    const notLast = renderMessage(message({ role: 'User', agent_type: null }));
    expect(within(notLast.bubble).queryByRole('button', { name: 'disc.editResend' })).toBeNull();
    notLast.unmount();
    const sending = renderMessage(message({ role: 'User', agent_type: null }), { isLastUser: true, sending: true });
    expect(within(sending.bubble).queryByRole('button', { name: 'disc.editResend' })).toBeNull();
  });

  it('in edit mode shows the textarea, Cancel and Resend, and hides Reply and Delete', () => {
    const { bubble, fns } = renderMessage(
      message({ role: 'User', agent_type: null }),
      { isLastUser: true, isEditing: true, editingText: 'new text' },
    );
    expect(within(bubble).getByRole('textbox', { name: 'disc.editResend' })).toHaveValue('new text');
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.cancel' }));
    expect(fns.onEditCancel).toHaveBeenCalled();
    fireEvent.click(within(bubble).getByRole('button', { name: /disc\.resend/ }));
    expect(fns.onEditSubmit).toHaveBeenCalled();
    expect(within(bubble).queryByRole('button', { name: 'disc.reply' })).toBeNull();
    expect(within(bubble).queryByRole('button', { name: 'disc.deleteMessage' })).toBeNull();
  });

  it('shows the in-reply-to header and the attachment placeholder', () => {
    const target = message({ id: 'target-1', role: 'Agent' });
    const { bubble, fns } = renderMessage(
      message({ role: 'User', agent_type: null, reply_to_message_id: 'target-1' }),
      { replyTarget: target, pendingAttachment: true },
    );
    fireEvent.click(within(bubble).getByRole('button', { name: /disc\.inReplyTo/ }));
    expect(fns.onReplyNavigate).toHaveBeenCalledWith('target-1');
    expect(within(bubble).getByText(/disc\.attachmentDownloading/)).toBeInTheDocument();
  });
});

describe('KT-1902 inventory — agent message', () => {
  it('native agent: identity, model, tier, and Reply, Copy, TTS, Retry and Delete', () => {
    const { bubble, fns } = renderMessage(
      message({ model: 'claude-opus', model_tier: 'reasoning', tokens_used: 1234, auth_mode: 'override' }),
      { isLastAgent: true, prevUserTs: '2026-11-20T10:29:00.000Z', hasFullAccess: true },
    );
    expect(within(bubble).getByTestId('agent-identity-model')).toHaveTextContent('claude-opus');
    expect(bubble.querySelector('.disc-model-tier-badge')).toHaveTextContent('disc.tier.reasoning');
    expect(bubble.querySelector('.disc-full-access-badge')).toHaveTextContent('config.fullAccessBadge');
    expect(bubble.querySelector('.disc-msg-token-count')).toHaveTextContent('tok');
    expect(bubble.querySelector('.disc-msg-auth-mode')).toHaveTextContent('API key');
    expect(bubble.querySelector('.disc-msg-duration')).toHaveTextContent('1m');
    expect(bubble.querySelector('.disc-msg-time')).not.toBeNull();
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.reply' }));
    expect(fns.onReply).toHaveBeenCalled();
    fireEvent.click(within(bubble).getAllByRole('button', { name: /disc\.copy/ })[0]);
    expect(fns.onCopy).toHaveBeenCalledWith('msg-inventory', 'An answer.');
    fireEvent.click(bubble.querySelector('.disc-tts-btn')!);
    expect(fns.onTts).toHaveBeenCalledWith('msg-inventory', 'An answer.', 'fr');
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.retryResponse' }));
    expect(fns.onRetry).toHaveBeenCalled();
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.deleteMessage' }));
    expect(fns.onDelete).toHaveBeenCalled();
    expect(within(bubble).queryByRole('button', { name: 'disc.editResend' })).toBeNull();
  });

  it('a CLI-joined agent says CLI and its ordinal, and whether its session continued', () => {
    const { bubble } = renderMessage(message({
      source_msg_id: 'cli-1',
      author_cli_ordinal: 2,
      session_continuity: { status: 'resumed', reason: null },
    } as Partial<DiscussionMessage>));
    expect(within(bubble).getByText(/CLI 2/)).toBeInTheDocument();
    expect(within(bubble).getByTestId('disc-msg-session-continuity')).toHaveAttribute('data-status', 'resumed');
  });

  it('an HTTP agent on a named connection writes its alias', () => {
    const { bubble } = renderMessage(message({ agent_type: 'Custom' }), {
      defaultAgent: 'Custom',
      defaultAgentAlias: '@openrouter',
    });
    expect(within(bubble).getByText('@openrouter')).toBeInTheDocument();
  });

  it('a federated reply names its peer', () => {
    const { bubble } = renderMessage(message({ author_pseudo: 'peer-one' }));
    expect(within(bubble).getByText('· peer-one')).toBeInTheDocument();
  });

  it('a workflow-step message names its workflow and step, and writes the step model (KT-2009)', () => {
    const { bubble } = renderMessage(message({ model: null }), {
      workflowStep: {
        run_id: 'run-12345678',
        workflow_name: 'Nightly',
        step_name: 'Review',
        model: 'gpt-step-model',
      } as MessageBubbleProps['workflowStep'],
    });
    expect(within(bubble).getByTestId('workflow-step-message-provenance')).toHaveTextContent('Nightly › Review');
    expect(within(bubble).getByTestId('agent-identity-model')).toHaveTextContent('gpt-step-model');
  });

  it('the message own model wins over the step model', () => {
    const { bubble } = renderMessage(message({ model: 'own-model' }), {
      workflowStep: {
        run_id: 'run-1', workflow_name: 'W', step_name: 'S', model: 'step-model',
      } as MessageBubbleProps['workflowStep'],
    });
    expect(within(bubble).getByTestId('agent-identity-model')).toHaveTextContent('own-model');
  });

  it('shows the « Répondu par » backlink and opens the reply', () => {
    const reply = message({ id: 'reply-1', role: 'User', agent_type: null, author_pseudo: 'Sam', content: 'thanks' });
    const { bubble, fns } = renderMessage(message(), { replies: [reply] });
    const link = within(bubble).getByRole('button', { name: /disc\.repliedBy/ });
    expect(link).toHaveClass('disc-reply-backlink');
    fireEvent.click(link);
    expect(fns.onReplyNavigate).toHaveBeenCalledWith('reply-1');
  });

  it('shows the lint pills and their detail drawer', () => {
    const { bubble } = renderMessage(message({
      lint_report: lint({
        unsourced_count: 1,
        flagged_spans: [{ text: 'a claim', reason: 'cue' }],
      } as Partial<LintReport>),
    }));
    const pill = within(bubble).getByTestId('lint-pill');
    expect(pill).toHaveAttribute('data-severity', 'unsourced');
    expect(within(bubble).getByTestId('lint-pill-verified')).toBeInTheDocument();
    fireEvent.click(pill);
    expect(screen.getByTestId('lint-detail')).toBeInTheDocument();
  });

  // Adapted by KT-1902: listening moved into the footer actions, an icon
  // whose state is its (now translated) accessible name, not a written label.
  it('shows the TTS states', () => {
    const playing = renderMessage(message(), { isTtsActive: true, ttsState: 'playing' });
    expect(playing.bubble.querySelector('.disc-tts-btn')).toHaveAccessibleName('disc.tts.pause');
    playing.unmount();
    const paused = renderMessage(message(), { isTtsActive: true, ttsState: 'paused' });
    expect(paused.bubble.querySelector('.disc-tts-btn')).toHaveAccessibleName('disc.tts.resume');
  });

  it('hides Retry while a reply is running and on a non-last agent message', () => {
    const sending = renderMessage(message(), { isLastAgent: true, sending: true });
    expect(within(sending.bubble).queryByRole('button', { name: 'disc.retryResponse' })).toBeNull();
    sending.unmount();
    const notLast = renderMessage(message());
    expect(within(notLast.bubble).queryByRole('button', { name: 'disc.retryResponse' })).toBeNull();
  });

  it('a salvaged fragment folds behind its own toggle', () => {
    const { bubble } = renderMessage(message({ recovered_partial: true } as Partial<DiscussionMessage>));
    expect(within(bubble).getByRole('button', { name: /disc\.interruptedFragment/ })).toHaveAttribute('aria-expanded', 'false');
  });
});

describe('KT-1902 inventory — system, error, orchestrator and deleted messages', () => {
  it('a system error names itself and the attempted model, offers Delete and no Reply', () => {
    const { bubble } = renderMessage(message({
      role: 'System', content: 'Agent error: boom', agent_type: 'Codex', model: 'gpt-x',
    }));
    expect(bubble).toHaveAttribute('data-variant', 'error');
    expect(within(bubble).getByTestId('disc-msg-attempted-model')).toHaveTextContent('gpt-x');
    expect(within(bubble).getByRole('button', { name: 'disc.deleteMessage' })).toBeInTheDocument();
    expect(within(bubble).queryByRole('button', { name: 'disc.reply' })).toBeNull();
  });

  it('a model error retries the failed dispatch of that agent', () => {
    const content = `[kronn:agent-error]\n${JSON.stringify({
      kind: 'model_error', status: 404, summary: 'Model missing', detail: 'raw', tier: 'default',
      retry_dispatch_id: 'dispatch-7', retried: false,
    })}`;
    expect(parseModelErrorEvent(content)).not.toBeNull();
    const { bubble, fns } = renderMessage(message({ role: 'System', content, agent_type: 'LiteLlm' }));
    fireEvent.click(within(bubble).getByTestId('retry-agent-dispatch'));
    expect(fns.onRetryAgentDispatch).toHaveBeenCalledWith('dispatch-7', 'LiteLlm');
  });

  it('a cached summary offers its expand toggle', () => {
    const { bubble, fns } = renderMessage(
      message({ role: 'System', content: 'summary cached (12 messages)', agent_type: null }),
      { summaryCache: 'the summary' },
    );
    fireEvent.click(within(bubble).getByRole('button', { name: 'disc.viewSummary' }));
    expect(fns.onExpandSummary).toHaveBeenCalledWith('msg-inventory');
  });

  it('an orchestrator notice offers its details toggle', () => {
    const { bubble } = renderMessage(message({
      role: 'User', agent_type: null, author_pseudo: 'Orchestrateur', content: '## Status\nAll good',
    }));
    expect(within(bubble).getByRole('button', { name: 'disc.orchestratorShowDetails' })).toHaveAttribute('aria-expanded', 'false');
  });

  it('a deleted message leaves a tombstone with its id', () => {
    const { container } = renderMessage(message({ content: DELETED_MESSAGE_MARKER }));
    expect(within(container).getByText('disc.deletedMessage')).toBeInTheDocument();
    expect(within(container).getByRole('button', { name: /disc\.idPillTooltip/ })).toBeInTheDocument();
  });
});

describe('KT-1902 inventory — workflow step at work', () => {
  it('names the step agent, the workflow and step, and says it is running', () => {
    const step = {
      run_id: 'run-abcdef12',
      step_key: 'review',
      workflow_name: 'Nightly',
      step_name: 'Review',
      agent_type: 'Codex',
      progress_run_id: null,
      preparing: false,
    } as unknown as ActiveWorkflowStep;
    render(
      <I18nProvider>
        <WorkflowStepActivityBubble step={step} discussionId="disc-inventory" />
      </I18nProvider>,
    );
    expect(screen.getByTestId('workflow-step-agent-identity')).toBeInTheDocument();
    expect(screen.getByTestId('workflow-step-identity')).toHaveTextContent('Nightly');
    expect(screen.getByTestId('workflow-step-waiting')).toBeInTheDocument();
  });
});
