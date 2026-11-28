/**
 * KT-1902 — the Message component: actions in one order (absent, never
 * moved), the « … » menu under the first breakpoint, the fold, the named code
 * copy, and the 400 px footer. The variants the page renders are checked at
 * the end, with the guard that no bubble is written by hand any more.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { act, render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { cssText } from '../../test/cssBlocks';

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

import { MessageActions } from '../Message';
import {
  MessageCompactContext, MessageFoldContext, MESSAGE_FOLD_THRESHOLD_PX, type MessageActionSet,
} from '../messageKinds';
import { MessageBubble, MarkdownContent, type MessageBubbleProps } from '../MessageBubble';
import { CodeBlock } from '../CodeBlock';
import {
  DebateLaunchingMessage, DebateStreamMessage, PendingAgentReplyMessage, QueuedMessages, StreamingAgentReplyMessage,
} from '../MessageVariants';
import type { DiscussionMessage } from '../../types/generated';
import type { QueuedMessage } from '../../hooks/useMessageQueue';

const NO_REPLY_IDS = { claimed: new Set<string>(), durable: new Set<string>() };
const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = join(HERE, '..', '..');

function message(overrides: Partial<DiscussionMessage> = {}): DiscussionMessage {
  return {
    id: 'msg-1',
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
  discussionId: 'disc-1',
  projectId: null,
  t: (key: string) => key,
  onCopy: () => {},
  onTts: () => {},
  onEditStart: () => {},
  onEditCancel: () => {},
  onEditSubmit: () => {},
  onEditTextChange: () => {},
  onRetry: () => {},
  onExpandSummary: () => {},
  onNavigate: () => {},
  onReply: () => {},
  onDelete: () => {},
};

function renderBubble(msg: DiscussionMessage, props: Partial<MessageBubbleProps> = {}, compact = false) {
  return render(
    <I18nProvider>
      <MessageCompactContext.Provider value={compact}>
        <MessageBubble {...base} {...props} msg={msg} />
      </MessageCompactContext.Provider>
    </I18nProvider>,
  );
}

/** The footer actions of the first message, in the order they appear. */
function actionOrder(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('.kr-message-actions [data-action]'))
    .map(button => button.getAttribute('data-action') ?? '');
}

describe('KT-1902 — actions in a fixed order, absent never moved', () => {
  it('user message: Reply, Copy, Edit, Delete', () => {
    const { container } = renderBubble(message({ role: 'User', agent_type: null }), { isLastUser: true });
    expect(actionOrder(container)).toEqual(['reply', 'copy', 'edit', 'delete']);
  });

  it('a user message that cannot be edited keeps the others in place', () => {
    const { container } = renderBubble(message({ role: 'User', agent_type: null }));
    expect(actionOrder(container)).toEqual(['reply', 'copy', 'delete']);
  });

  it('agent message: Reply, Copy, listen, Retry, Delete', () => {
    const { container } = renderBubble(message(), { isLastAgent: true });
    expect(actionOrder(container)).toEqual(['reply', 'copy', 'listen', 'retry', 'delete']);
  });

  it('an agent message while a reply runs drops Retry only', () => {
    const { container } = renderBubble(message(), { isLastAgent: true, sending: true });
    expect(actionOrder(container)).toEqual(['reply', 'copy', 'listen', 'delete']);
  });

  it('system message: Copy, Delete (no Reply)', () => {
    const { container } = renderBubble(message({ role: 'System', content: 'Agent error: x' }));
    expect(actionOrder(container)).toEqual(['copy', 'delete']);
  });

  it('queued message: Copy, Retry, Delete', () => {
    const queued: QueuedMessage[] = [{
      id: 'q-1', content: 'later', status: 'failed', attempts: 1, createdAt: '2026-11-20T10:00:00Z',
    }];
    const { container } = render(
      <I18nProvider>
        <QueuedMessages messages={queued} onCopy={() => {}} onRetry={() => {}} onRemove={() => {}} onClearAll={() => {}} />
      </I18nProvider>,
    );
    expect(actionOrder(container)).toEqual(['copy', 'retry', 'delete']);
  });

  it('the order holds whatever order the actions are declared in', () => {
    const noop = () => {};
    const actions: MessageActionSet = {
      delete: { label: 'Delete', icon: null, onSelect: noop },
      edit: { label: 'Edit', icon: null, onSelect: noop },
      copy: { label: 'Copy', icon: null, onSelect: noop },
      reply: { label: 'Reply', icon: null, onSelect: noop },
    };
    const { container } = render(<MessageActions actions={actions} compact={false} />);
    expect(actionOrder(container)).toEqual(['reply', 'copy', 'edit', 'delete']);
  });

  it('the header writes the time, and the footer no longer repeats Copy in the header', () => {
    const { container } = renderBubble(message());
    expect(container.querySelector('.disc-msg-header-actions time.disc-msg-time')).not.toBeNull();
    expect(container.querySelectorAll('button.disc-copy-btn')).toHaveLength(1);
  });

  it('status chips sit with the facts on the left, never between actions', () => {
    const { container } = renderBubble(message({ model_tier: 'reasoning' }), { hasFullAccess: true });
    const info = container.querySelector('.disc-msg-footer-info')!;
    expect(info.querySelector('.disc-model-tier-badge')).not.toBeNull();
    expect(info.querySelector('.disc-full-access-badge')).not.toBeNull();
    expect(container.querySelector('.kr-message-actions .disc-model-tier-badge')).toBeNull();
  });
});

describe('KT-1902 — under the first breakpoint the actions go into the « … » menu', () => {
  it('shows one named trigger, and the menu lists the actions in the same order', async () => {
    renderBubble(message({ role: 'User', agent_type: null }), { isLastUser: true }, true);
    expect(screen.queryByRole('button', { name: 'disc.reply' })).toBeNull();
    const trigger = screen.getByRole('button', { name: 'Actions du message' });
    expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
    fireEvent.click(trigger);
    const menu = await screen.findByRole('menu');
    expect(within(menu).getAllByRole('menuitem').map(item => item.textContent))
      .toEqual(['disc.reply', 'disc.copyMessage', 'disc.editResend', 'disc.deleteMessage']);
  });

  it('closes on Escape and gives focus back to its trigger', async () => {
    renderBubble(message(), {}, true);
    const trigger = screen.getByRole('button', { name: 'Actions du message' });
    fireEvent.click(trigger);
    const menu = await screen.findByRole('menu');
    fireEvent.keyDown(menu, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
    expect(trigger).toHaveFocus();
  });

  it('a menu entry runs its action', async () => {
    const onReply = vi.fn();
    renderBubble(message(), { onReply }, true);
    fireEvent.click(screen.getByRole('button', { name: 'Actions du message' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'disc.reply' }));
    expect(onReply).toHaveBeenCalledTimes(1);
  });
});

describe('KT-1902 — a long message folds', () => {
  let tall = MESSAGE_FOLD_THRESHOLD_PX + 200;
  beforeEach(() => {
    tall = MESSAGE_FOLD_THRESHOLD_PX + 200;
    vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(() => tall);
  });
  afterEach(() => vi.restoreAllMocks());

  const folded = (container: HTMLElement) =>
    container.querySelector('.kr-message-body')?.getAttribute('data-folded') === 'true';

  it('folds above the threshold behind a « Show more » button with aria-expanded', () => {
    const { container } = renderBubble(message({ content: 'long '.repeat(400) }));
    expect(folded(container)).toBe(true);
    const toggle = screen.getByRole('button', { name: 'Afficher plus' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(folded(container)).toBe(false);
    expect(screen.getByRole('button', { name: 'Afficher moins' })).toHaveAttribute('aria-expanded', 'true');
  });

  it('does not fold a message under the threshold', () => {
    tall = MESSAGE_FOLD_THRESHOLD_PX - 10;
    const { container } = renderBubble(message());
    expect(folded(container)).toBe(false);
    expect(screen.queryByRole('button', { name: 'Afficher plus' })).toBeNull();
  });

  it('keeps the unfolded state while the thread stays open', () => {
    const store = new Set<string>();
    const view = (
      <I18nProvider>
        <MessageFoldContext.Provider value={store}>
          <MessageBubble {...base} msg={message()} />
        </MessageFoldContext.Provider>
      </I18nProvider>
    );
    const first = render(view);
    fireEvent.click(screen.getByRole('button', { name: 'Afficher plus' }));
    first.unmount();
    const again = render(view);
    expect(folded(again.container)).toBe(false);
    expect(screen.getByRole('button', { name: 'Afficher moins' })).toBeInTheDocument();
  });

  it('a search hit or an anchor jump unfolds the message', () => {
    const { container, rerender } = renderBubble(message());
    expect(folded(container)).toBe(true);
    rerender(
      <I18nProvider>
        <MessageBubble {...base} msg={message()} isSearchCurrent />
      </I18nProvider>,
    );
    expect(folded(container)).toBe(false);
  });

  it('never folds a message being edited or one carrying a card to decide on', () => {
    const editing = renderBubble(message({ role: 'User', agent_type: null }), { isLastUser: true, isEditing: true, editingText: 'x' });
    expect(folded(editing.container)).toBe(false);
    editing.unmount();
    const card = renderBubble(message({ content: '```kronn-question\n{}\n```' }));
    expect(folded(card.container)).toBe(false);
  });

  it('a streaming reply never folds while it streams', () => {
    const { container } = render(
      <I18nProvider>
        <StreamingAgentReplyMessage
          replyId="r-1" discussionId="disc-1" replyIds={NO_REPLY_IDS} agent="ClaudeCode" triggerMessageId="u-1"
          elapsed={3} text={'long '.repeat(400)} logs={[]} showLogs={false} onToggleLogs={() => {}}
          stopping={false} onStop={() => {}} recovering={false} recoveryLabel={null}
          waitingUpstream={false} model={{ kind: 'providerDefault' }}
        />
      </I18nProvider>,
    );
    expect(container.querySelector('.kr-message-body')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Afficher plus' })).toBeNull();
  });
});

describe('KT-1902 — CodeBlock', () => {
  beforeEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
  });

  it('writes the language and names its Copy after it', async () => {
    render(
      <I18nProvider>
        <CodeBlock code={'const a = 1;'} language="ts" />
      </I18nProvider>,
    );
    expect(document.querySelector('.kr-code-block-language')).toHaveTextContent('TypeScript');
    const copy = await screen.findByRole('button', { name: /TypeScript/ });
    await act(async () => { fireEvent.click(copy); });
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('const a = 1;');
  });

  it('colours the code with the semantic code tokens (highlight.js classes)', () => {
    const { container } = render(<CodeBlock code={'const a = "x";'} language="javascript" />);
    expect(container.querySelector('.hljs-keyword')).toHaveTextContent('const');
    const css = cssText(join(HERE, '..', 'CodeBlock.css'));
    expect(css).toMatch(/\.hljs-keyword[^{]*\{\s*color:\s*var\(--kr-code-keyword\)/);
    expect(css).toMatch(/background:\s*var\(--kr-code-surface\)/);
  });

  it('keeps an unknown language as its label, and a fence without one has a plain named Copy', () => {
    const unknown = render(<CodeBlock code="x" language="cobol-ish" />);
    expect(unknown.container.querySelector('.kr-code-block-language')).toHaveTextContent('cobol-ish');
    unknown.unmount();
    render(<CodeBlock code="x" language={null} />);
    expect(document.querySelector('.kr-code-block-language')).toBeNull();
    expect(screen.getByRole('button', { name: 'disc.code.copy' })).toBeInTheDocument();
  });

  it('scrolls a long line inside the block, which a keyboard can reach', () => {
    const { container } = render(<MarkdownContent content={'```rust\nfn main() {}\n```'} />);
    expect(container.querySelector('pre.kr-code-block-pre')).toHaveAttribute('tabindex', '0');
    const css = cssText(join(HERE, '..', 'CodeBlock.css'));
    expect(css).toMatch(/\.kr-code-block-pre\s*\{[^}]*overflow-x:\s*auto/);
    expect(css).toMatch(/\.kr-code-block-pre\s*\{[^}]*white-space:\s*pre;/);
  });

  it('keeps the code and its language label on the 12 px floor (KT-1878 type scale)', () => {
    const css = cssText(join(HERE, '..', 'CodeBlock.css'));
    expect(css).toMatch(/\.kr-code-block-language\s*\{[^}]*font-size:\s*var\(--kr-type-sm\)/);
    expect(css).toMatch(/\.kr-code-block-pre code\s*\{[^}]*font-size:\s*var\(--kr-type-sm\)/);
    expect(css).not.toMatch(/--kr-fs-(2xs|xs)\b/);
    expect(css).toMatch(/\.kr-code-block-bar\s*\{[^}]*align-items:\s*center/);
  });
});

describe('KT-1902 — the footer at 400 px', () => {
  const css = cssText(join(HERE, '..', 'Message.css'));

  it('wraps as whole items: the footer and its facts wrap, no chip breaks its label', () => {
    expect(css).toMatch(/\.disc-msg-footer\s*\{[^}]*flex-wrap:\s*wrap/);
    expect(css).toMatch(/\.disc-msg-footer-info\s*\{[^}]*flex-wrap:\s*wrap/);
    expect(css).toMatch(/\.disc-msg-footer-info > \*\s*\{[^}]*white-space:\s*nowrap/);
    expect(css).toMatch(/\.kr-message-actions\s*\{[^}]*flex:\s*0 0 auto/);
  });

  it('every action is an icon with an accessible name, no written label to break', () => {
    const { container } = renderBubble(
      message({ lint_report: null }),
      { isLastAgent: true, replies: [message({ id: 'r-2', role: 'User', agent_type: null, author_pseudo: 'Sam' })] },
    );
    const actions = container.querySelectorAll<HTMLButtonElement>('.kr-message-actions button');
    expect(actions.length).toBeGreaterThan(0);
    actions.forEach(button => {
      expect(button.textContent?.trim()).toBe('');
      expect(button.getAttribute('aria-label')).toBeTruthy();
    });
    // The « Répondu par … » link is a fact on the left, kept on one line.
    expect(container.querySelector('.disc-msg-footer-info .disc-reply-backlink')).not.toBeNull();
  });

  it('the header identity shrinks and wraps whole; the #id pill and the time never shrink', () => {
    // The row: the identity side may shrink, the actions side (#id, time) may not.
    expect(css).toMatch(/\.disc-msg-header-main > \.disc-msg-agent-label\s*\{[^}]*min-width:\s*0/);
    expect(css).toMatch(/\.disc-msg-header-main \.disc-msg-agent-identity-line\s*\{[^}]*flex-wrap:\s*wrap[^}]*min-width:\s*0[^}]*max-width:\s*100%/);
    expect(css).toMatch(/\.disc-msg-header-main \.disc-msg-agent-identity-line > \*\s*\{[^}]*min-width:\s*0[^}]*max-width:\s*100%/);
    expect(css).toMatch(/\.disc-msg-header-main \.ui-agent-identity\s*\{[^}]*flex-wrap:\s*wrap[^}]*min-width:\s*0/);
    // The model id is the piece that truncates, with an ellipsis.
    expect(css).toMatch(/\.disc-msg-header-main \.ui-agent-identity-model\s*\{[^}]*flex:\s*1 1 8ch[^}]*min-width:\s*0[^}]*overflow:\s*hidden[^}]*text-overflow:\s*ellipsis[^}]*white-space:\s*nowrap/);
    const page = cssText(join(HERE, '..', '..', 'pages', 'DiscussionsPage.css'));
    expect(page).toMatch(/\.disc-msg-header-main\s*\{[^}]*min-width:\s*0/);
    expect(page).toMatch(/\.disc-msg-header-actions\s*\{[^}]*flex:\s*0 0 auto/);

    const model = 'nvidia/nemotron-3-ultra-550b-a55b';
    const { container } = renderBubble(message({ model, model_tier: 'reasoning' }));
    const line = container.querySelector<HTMLElement>('.disc-msg-header-main .disc-msg-agent-identity-line')!;
    expect(line).not.toBeNull();
    // The full name stays reachable when the model is cut.
    expect(line.title).toContain(model);
    expect(line.querySelector('.ui-agent-identity-model')).toHaveTextContent(model);
    expect(container.querySelector('.disc-msg-header-actions time.disc-msg-time')).not.toBeNull();
  });

  it('under the breakpoint the footer holds the facts and one « … » trigger', () => {
    const { container } = renderBubble(message(), { isLastAgent: true }, true);
    expect(container.querySelectorAll('.kr-message-actions button')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Actions du message' })).toBeInTheDocument();
  });
});

describe('KT-1902 — the page variants are Message, no bubble is written by hand', () => {
  it('pending, refused, streaming, debate and queued replies render as Message variants', () => {
    const queued: QueuedMessage[] = [{ id: 'q-1', content: 'later', status: 'queued', attempts: 0, createdAt: '2026-11-20T10:00:00Z' }];
    const { container } = render(
      <I18nProvider>
        <PendingAgentReplyMessage
          replyId="p-1" discussionId="disc-1" replyIds={NO_REPLY_IDS} agent="Codex" triggerMessageId="u-1"
          status="Pending" stopping={false} onStop={() => {}} model={{ kind: 'providerDefault' }}
        />
        <PendingAgentReplyMessage
          replyId="p-2" discussionId="disc-1" replyIds={NO_REPLY_IDS} agent="Ollama" triggerMessageId="u-1"
          status="Failed" lastError="no model" stopping={false} onStop={() => {}} model={{ kind: 'providerDefault' }}
        />
        <DebateLaunchingMessage systemMessages={['round 1']} />
        <DebateStreamMessage agent="Codex" agentType="Codex" round={1} text="" done />
        <QueuedMessages messages={queued} onCopy={() => {}} onRetry={() => {}} onRemove={() => {}} onClearAll={() => {}} />
      </I18nProvider>,
    );
    const variants = Array.from(container.querySelectorAll('.disc-msg-row'))
      .map(row => row.getAttribute('data-message-variant'));
    expect(variants).toEqual(['streaming', 'error', 'streaming', 'streaming', 'queued']);
    expect(screen.getByTestId('pending-agent-Codex')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Arrêter uniquement/ })).toBeInTheDocument();
    expect(screen.getByTestId('failed-agent-Ollama')).toHaveTextContent('no model');
    expect(screen.getByTestId('queued-messages')).toHaveAttribute('role', 'group');
    expect(screen.getByTestId('queued-messages')).toHaveAccessibleName(/.+/);
  });

  it('DiscussionsPage and the bubble components write no bubble markup of their own', () => {
    for (const file of ['pages/DiscussionsPage.tsx', 'components/WorkflowStepActivityBubble.tsx',
      'components/InlineMediaJob.tsx', 'components/DiscussionNote.tsx', 'components/MessageVariants.tsx']) {
      const source = readFileSync(join(SRC, file), 'utf8');
      expect(source, file).not.toMatch(/className="disc-msg-(row|bubble)/);
      expect(source, file).not.toMatch(/disc-queued-bubble/);
    }
  });

  it('the bubble stylesheet no longer holds a duplicated block', () => {
    const css = cssText(join(SRC, 'pages', 'DiscussionsPage.css'));
    for (const selector of ['.disc-reply-header {', '.disc-reply-header code {', '.disc-tool-calls-group[data-search-current="true"] {']) {
      expect(css.split(selector).length - 1, selector).toBe(1);
    }
    expect(css).not.toMatch(/\.disc-queued-bubble\s*\{/);
  });
});
