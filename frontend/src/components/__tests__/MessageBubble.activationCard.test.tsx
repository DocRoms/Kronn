/**
 * KT-1490 — the System message an agent's inactive Quick Exec leaves in the
 * room renders as the one-click activation card, not as raw text.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    config: { getUiLanguage: vi.fn().mockResolvedValue('fr') },
    quickExecs: {
      list: vi.fn().mockResolvedValue([{
        id: 'qe-1', name: 'Purge CDN', command: 'fastly', args: ['purge', '--key', 'k'],
      }]),
      pendingApprovals: vi.fn().mockResolvedValue([{ id: 'qe-1', program: 'fastly', message: '', refused: false }]),
      activate: vi.fn().mockResolvedValue({}),
    },
  };
});

import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage } from '../../types/generated';

function systemMessage(content: string): DiscussionMessage {
  return {
    id: 'msg-card',
    role: 'System',
    channel: 'main',
    content,
    agent_type: null,
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
  } as DiscussionMessage;
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
  t: (key: string, ...args: unknown[]) => (args.length ? `${key}|${args.join('|')}` : key),
};

describe('the room activation card in a message', () => {
  it('renders Activer / Refuser for a kronn-activate System message', async () => {
    const content = '[kronn-activate: {"quick_exec_id":"qe-1","name":"Purge CDN","program":"fastly"}]';
    render(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={systemMessage(content)} />
      </I18nProvider>,
    );
    expect(await screen.findByTestId('qe-activation-card')).toBeDefined();
    expect(await screen.findByRole('button', { name: /Activer/ })).toBeDefined();
    expect(screen.getByRole('button', { name: /Refuser/ })).toBeDefined();
    expect(screen.queryByText(/kronn-activate/)).toBeNull();
  });

  it('does not wear the error header: a human is asked, nothing failed', async () => {
    const content = '[kronn-activate: {"quick_exec_id":"qe-1","name":"Purge CDN","program":"fastly"}]';
    const { container } = render(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={systemMessage(content)} />
      </I18nProvider>,
    );
    await screen.findByTestId('qe-activation-card');
    const bubble = container.querySelector('.disc-msg-bubble');
    // Its own variant, not the red « error » one, and not labelled « Système ».
    expect(bubble?.getAttribute('data-variant')).toBe('kronn-activation');
    const label = container.querySelector('.disc-msg-agent-label') as HTMLElement;
    expect(label.textContent).toContain('qeActivation.header');
    expect(label.textContent).not.toContain('disc.system');
    expect(label.style.color).not.toContain('--kr-error');
    expect(label.style.color).toContain('--kr-accent-ink');
  });

  it('puts Activer and Refuser side by side, one size, primary then secondary', async () => {
    const content = '[kronn-activate: {"quick_exec_id":"qe-1","name":"Purge CDN","program":"fastly"}]';
    render(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={systemMessage(content)} />
      </I18nProvider>,
    );
    const activate = await screen.findByRole('button', { name: /Activer/ });
    const refuse = screen.getByRole('button', { name: /Refuser/ });
    expect(activate.className).toBe(refuse.className);
    expect(activate.getAttribute('data-kind')).toBe('primary');
    expect(refuse.getAttribute('data-kind')).toBe('secondary');
    expect(activate.parentElement).toBe(refuse.parentElement);
  });

  it('renders no card for the same text in an Agent or User message: only Kronn\'s System message is a card', async () => {
    const content = '[kronn-activate: {"quick_exec_id":"qe-1","name":"Purge CDN","program":"fastly"}]';
    for (const role of ['Agent', 'User'] as const) {
      const { unmount } = render(
        <I18nProvider>
          <MessageBubble
            {...baseProps}
            msg={{ ...systemMessage(content), role, agent_type: role === 'Agent' ? 'ClaudeCode' : null }}
          />
        </I18nProvider>,
      );
      // An agent cannot forge the card: its text stays text.
      await screen.findAllByText(/Purge CDN/);
      expect(screen.queryByTestId('qe-activation-card')).toBeNull();
      expect(screen.queryByRole('button', { name: /Activer/ })).toBeNull();
      unmount();
    }
  });
});
