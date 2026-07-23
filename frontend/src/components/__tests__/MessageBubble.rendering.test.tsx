/**
 * MessageBubble — broad rendering / branch coverage suite.
 *
 * The pre-existing MessageBubble specs each pin one feature (emoji,
 * kronn-tool badge, lint pill, seed toggle, validation CTA). This file
 * fills the gap on the *structural* render branches that were otherwise
 * only exercised transitively: role-based bubble variants, author
 * pseudo/avatar, the agent + footer copy buttons, TTS button states,
 * timestamp / token / auth-mode / duration / model-tier / full-access
 * footer chips, edit mode, the auth-error & partial-response CTAs, the
 * summary-cached system message + expand toggle, and the last-user /
 * last-agent edit & retry affordances.
 *
 * Plus the exported MarkdownContent helpers: CopyableBlock (pre + table
 * copy paths through extractText) and the doc-data / mermaid fence
 * branches that only fire when a discussionId is supplied.
 *
 * Convention: mirrors the kronnTool / lintPill specs — I18nProvider
 * wrapper, mocked boot-config call, identity translator `t`, a base
 * props bag spread onto every render. No real person names.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, within, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock the boot config call so I18nProvider doesn't try to fetch.
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { MessageBubble, MarkdownContent, type MessageBubbleProps } from '../MessageBubble';
import type { DiscussionMessage, MessageRole } from '../../types/generated';

function makeMessage(overrides: Partial<DiscussionMessage> = {}): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'Agent',
    content: 'Some answer.',
    agent_type: 'ClaudeCode',
    timestamp: '2026-05-29T10:30:00.000Z',
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
    lint_report: null,
    ...overrides,
  };
}

const baseProps: Omit<MessageBubbleProps, 'msg'> = {
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
  summaryCache: null as string | null,
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
  projectId: null,
  t: (key: string) => key,
};

function renderBubble(
  msg: DiscussionMessage,
  props: Partial<typeof baseProps> = {},
) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} {...props} msg={msg} />
    </I18nProvider>,
  );
}

describe('MessageBubble — role-based bubble variant', () => {
  it('tags a User message bubble with data-role="user"', () => {
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hi' }));
    const bubble = container.querySelector('.disc-msg-bubble');
    expect(bubble?.getAttribute('data-role')).toBe('user');
    expect(bubble?.hasAttribute('data-variant')).toBe(false);
  });

  it('tags an Agent message bubble with data-role="agent"', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    expect(container.querySelector('.disc-msg-bubble')?.getAttribute('data-role')).toBe('agent');
  });

  it('tags an error System message with data-variant="error"', () => {
    const { container } = renderBubble(makeMessage({ role: 'System', content: 'API exploded' }));
    const bubble = container.querySelector('.disc-msg-bubble');
    expect(bubble?.getAttribute('data-role')).toBe('system');
    expect(bubble?.getAttribute('data-variant')).toBe('error');
    expect(screen.getByText('disc.system')).toBeInTheDocument();
  });

  it('tags a summary-cached System message with data-variant="summary"', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'System', content: 'summary cached for last 10 messages' }),
    );
    expect(container.querySelector('.disc-msg-bubble')?.getAttribute('data-variant')).toBe('summary');
    expect(screen.getByText('disc.summaryCached')).toBeInTheDocument();
  });
});

describe('MessageBubble — author pseudo / avatar (User)', () => {
  it('renders gravatar <img> when author_avatar_email is set', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'hello', author_pseudo: 'PeerAlpha', author_avatar_email: 'peer@example.com' }),
    );
    const img = container.querySelector('img.disc-msg-author-avatar') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    expect(img?.getAttribute('src')).toContain('gravatar.com');
    // Name still rendered alongside the avatar.
    expect(screen.getByText('PeerAlpha')).toBeInTheDocument();
  });

  it('renders 2-letter uppercased initials when only author_pseudo is set', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'hello', author_pseudo: 'peerbravo' }),
    );
    expect(container.querySelector('img.disc-msg-author-avatar')).toBeNull();
    const initials = container.querySelector('.disc-msg-author-initials');
    expect(initials?.textContent).toBe('PE');
  });

  it('still renders a HUMAN attribution (anonyme) when neither pseudo nor avatar is present', () => {
    // F11: a user message must ALWAYS read as a human (vs an agent), even with
    // no pseudo (federated from a peer whose pseudo is unset) — it falls back to
    // "anonyme · humain" rather than rendering no attribution at all.
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hello' }));
    const author = container.querySelector('.disc-msg-author');
    expect(author).not.toBeNull();
    expect(author?.textContent).toContain('anonyme');
    expect(author?.textContent).toContain('humain');
  });
});

describe('MessageBubble — agent label + copy buttons', () => {
  it('renders the agent label with the agent type', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', agent_type: 'Codex' }));
    expect(container.querySelector('.disc-msg-agent-label')?.textContent).toContain('Codex');
  });

  it('falls back to defaultAgent when agent_type is null', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', agent_type: null }),
      { defaultAgent: 'Vibe' },
    );
    expect(container.querySelector('.disc-msg-agent-label')?.textContent).toContain('Vibe');
  });

  it('invokes onCopy(msgId, content) from the footer copy button', () => {
    const onCopy = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'copy me' }),
      { onCopy },
    );
    // Two copy buttons exist for an Agent msg (label + footer); clicking
    // either must forward the same (id, content) pair.
    const btns = container.querySelectorAll('button.disc-copy-btn');
    expect(btns.length).toBe(2);
    fireEvent.click(btns[0]);
    expect(onCopy).toHaveBeenCalledWith('msg-test', 'copy me');
  });

  it('reflects the copied state on the copy button', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { isCopied: true });
    const btn = container.querySelector('button.disc-copy-btn');
    expect(btn?.getAttribute('data-copied')).toBe('true');
    expect(btn?.textContent).toContain('disc.copied');
  });
});

describe('MessageBubble — TTS button states (Agent only)', () => {
  it('shows the default TTS label when not active', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    const tts = container.querySelector('button.disc-tts-btn');
    expect(tts?.textContent).toContain('TTS');
  });

  it('shows the loading state', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'loading' },
    );
    const tts = container.querySelector('button.disc-tts-btn');
    expect(tts?.textContent).toContain('TTS');
    expect(tts?.querySelector('svg')).not.toBeNull();
  });

  it('shows Pause while playing', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'playing' },
    );
    expect(container.querySelector('button.disc-tts-btn')?.textContent).toContain('Pause');
  });

  it('shows Reprendre while paused', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'paused' },
    );
    expect(container.querySelector('button.disc-tts-btn')?.textContent).toContain('Reprendre');
  });

  it('invokes onTts(msgId, content, language) on click', () => {
    const onTts = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'read aloud' }),
      { onTts, language: 'es' },
    );
    fireEvent.click(container.querySelector('button.disc-tts-btn')!);
    expect(onTts).toHaveBeenCalledWith('msg-test', 'read aloud', 'es');
  });

  it('does NOT render a TTS button for User messages', () => {
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hi' }));
    expect(container.querySelector('button.disc-tts-btn')).toBeNull();
  });
});

describe('MessageBubble — footer chips', () => {
  it('renders the formatted timestamp', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    const timeEl = container.querySelector('.disc-msg-time');
    // Locale-formatted HH:MM — exact value depends on TZ, so just assert
    // it looks like a clock value rather than empty.
    expect(timeEl?.textContent).toMatch(/\d{1,2}:\d{2}/);
  });

  it('renders the standard short ID pill in the header and copies the full ID', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    const id = '12345678-90ab-cdef-1234-567890abcdef';
    const { container } = renderBubble(makeMessage({ role: 'System', id }));

    const pill = container.querySelector<HTMLButtonElement>('.disc-message-id-pill');
    expect(pill).not.toBeNull();
    expect(pill?.textContent).toContain('#12345678');
    expect(pill?.classList.contains('disc-id-pill')).toBe(true);
    expect(pill?.closest('.disc-msg-header-row')).not.toBeNull();
    expect(pill?.closest('.disc-msg-footer')).toBeNull();

    fireEvent.click(pill!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(id));
    expect(pill?.getAttribute('data-copied')).toBe('true');
  });

  it('renders the token count only when tokens_used > 0', () => {
    const { container, rerender } = renderBubble(makeMessage({ role: 'Agent', tokens_used: 0 }));
    expect(container.querySelector('.disc-msg-token-count')).toBeNull();

    rerender(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={makeMessage({ role: 'Agent', tokens_used: 12345 })} />
      </I18nProvider>,
    );
    const tok = container.querySelector('.disc-msg-token-count');
    expect(tok?.textContent).toContain('tok');
    // Locale-grouped number (12,345 / 12 345 / 12.345 depending on locale).
    expect(tok?.textContent).toMatch(/12[\s,.]?345/);
  });

  it('renders the auth-mode chip as "API key" for override', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', auth_mode: 'override' }));
    const chip = container.querySelector('.disc-msg-auth-mode');
    expect(chip?.getAttribute('data-mode')).toBe('override');
    expect(chip?.textContent).toContain('API key');
  });

  it('renders the auth-mode chip as "auth locale" for any other mode', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', auth_mode: 'local' }));
    const chip = container.querySelector('.disc-msg-auth-mode');
    expect(chip?.getAttribute('data-mode')).toBe('local');
    expect(chip?.textContent).toContain('auth locale');
  });

  it('renders a duration chip for an Agent message with prevUserTs', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', timestamp: '2026-05-29T10:30:42.000Z' }),
      { prevUserTs: '2026-05-29T10:30:00.000Z' },
    );
    const dur = container.querySelector('.disc-msg-duration');
    expect(dur?.textContent).toContain('42s');
  });

  it('formats a sub-minute-and-over duration as Xm Ys', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', timestamp: '2026-05-29T10:31:30.000Z' }),
      { prevUserTs: '2026-05-29T10:30:00.000Z' },
    );
    expect(container.querySelector('.disc-msg-duration')?.textContent).toContain('1m 30s');
  });

  it('omits the duration chip when prevUserTs is null', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { prevUserTs: null });
    expect(container.querySelector('.disc-msg-duration')).toBeNull();
  });

  it('renders the economy model-tier badge', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', model_tier: 'economy' }));
    const badge = container.querySelector('.disc-model-tier-badge');
    expect(badge?.getAttribute('data-tier')).toBe('economy');
    expect(badge?.textContent).toContain('⚡');
    expect(badge?.textContent).toContain('disc.tier.economy');
  });

  it('renders a non-economy model-tier badge with the brain glyph', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', model_tier: 'premium' }));
    const badge = container.querySelector('.disc-model-tier-badge');
    expect(badge?.getAttribute('data-tier')).toBe('premium');
    expect(badge?.textContent).toContain('🧠');
  });

  it('renders the full-access badge when hasFullAccess is true', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { hasFullAccess: true });
    const badge = container.querySelector('.disc-full-access-badge');
    expect(badge?.textContent).toContain('config.fullAccessBadge');
  });
});

describe('MessageBubble — last-message affordances', () => {
  it('shows the edit pencil on the last user message', () => {
    const onEditStart = vi.fn();
    renderBubble(
      makeMessage({ role: 'User', content: 'oops typo' }),
      { isLastUser: true, isLastAgent: false },
    );
    const pencil = screen.getByLabelText('disc.editResend');
    fireEvent.click(pencil);
    expect(onEditStart).not.toHaveBeenCalled(); // (default noop in baseProps)
  });

  it('wires the edit pencil to onEditStart(id, content)', () => {
    const onEditStart = vi.fn();
    renderBubble(
      makeMessage({ role: 'User', content: 'oops typo' }),
      { isLastUser: true, onEditStart },
    );
    fireEvent.click(screen.getByLabelText('disc.editResend'));
    expect(onEditStart).toHaveBeenCalledWith('msg-test', 'oops typo');
  });

  it('shows the retry button on the last agent message and calls onRetry', () => {
    const onRetry = vi.fn();
    renderBubble(makeMessage({ role: 'Agent' }), { isLastAgent: true, onRetry });
    fireEvent.click(screen.getByLabelText('disc.retryResponse'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('hides edit/retry affordances while sending', () => {
    renderBubble(makeMessage({ role: 'User' }), { isLastUser: true, sending: true });
    expect(screen.queryByLabelText('disc.editResend')).toBeNull();
  });
});

describe('MessageBubble — edit mode', () => {
  it('renders the textarea bound to editingText', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'orig' }),
      { isEditing: true, editingText: 'edited text' },
    );
    const ta = container.querySelector('textarea.disc-edit-textarea') as HTMLTextAreaElement;
    expect(ta).not.toBeNull();
    expect(ta.value).toBe('edited text');
  });

  it('fires onEditTextChange while typing', () => {
    const onEditTextChange = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'x', onEditTextChange },
    );
    fireEvent.change(container.querySelector('textarea')!, { target: { value: 'new' } });
    expect(onEditTextChange).toHaveBeenCalledWith('new');
  });

  it('fires onEditCancel from the cancel button', () => {
    const onEditCancel = vi.fn();
    renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'x', onEditCancel },
    );
    fireEvent.click(screen.getByText('disc.cancel'));
    expect(onEditCancel).toHaveBeenCalledTimes(1);
  });

  it('disables the resend button when the edited text is blank', () => {
    renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: '   ' },
    );
    const resend = screen.getByText('disc.resend').closest('button') as HTMLButtonElement;
    expect(resend.disabled).toBe(true);
  });

  it('submits on Ctrl+Enter (not during IME composition)', () => {
    const onEditSubmit = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'go', onEditSubmit },
    );
    fireEvent.keyDown(container.querySelector('textarea')!, { key: 'Enter', ctrlKey: true });
    expect(onEditSubmit).toHaveBeenCalledTimes(1);
  });
});

describe('MessageBubble — inline CTAs', () => {
  it('renders the override-key CTA on an auth-error message and navigates to settings', () => {
    const onNavigate = vi.fn();
    renderBubble(
      makeMessage({ role: 'Agent', content: 'Error: invalid API key, please authenticate.' }),
      { onNavigate },
    );
    fireEvent.click(screen.getByText('disc.overrideKey'));
    expect(onNavigate).toHaveBeenCalledWith('settings');
  });

  it('renders the edit-timeout CTA on a partial-response message and scrolls to server config', () => {
    const onNavigate = vi.fn();
    renderBubble(
      makeMessage({ role: 'Agent', content: "Réponse partielle — l'agent a été interrompu." }),
      { onNavigate },
    );
    fireEvent.click(screen.getByText('disc.editTimeout'));
    expect(onNavigate).toHaveBeenCalledWith('settings', { scrollTo: 'settings-server' });
  });
});

describe('MessageBubble — summary-cached expand toggle', () => {
  it('shows the view-summary toggle when a summaryCache is present', () => {
    renderBubble(
      makeMessage({ role: 'System', content: 'summary cached for last 10 messages' }),
      { summaryCache: 'condensed history' },
    );
    expect(screen.getByText('disc.viewSummary')).toBeInTheDocument();
  });

  it('calls onExpandSummary when the toggle is clicked', () => {
    const onExpandSummary = vi.fn();
    renderBubble(
      makeMessage({ role: 'System', content: 'summary cached' }),
      { summaryCache: 'condensed history', onExpandSummary },
    );
    fireEvent.click(screen.getByText('disc.viewSummary'));
    expect(onExpandSummary).toHaveBeenCalledWith('msg-test');
  });

  it('renders the expanded summary body when isExpandedSummary is true', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'System', content: 'summary cached' }),
      { summaryCache: 'condensed history body', isExpandedSummary: true },
    );
    const expanded = container.querySelector('.disc-summary-expanded');
    expect(expanded?.textContent).toContain('condensed history body');
    expect(screen.getByText('disc.hideSummary')).toBeInTheDocument();
  });
});

// ─── MarkdownContent helpers (CopyableBlock + fence branches) ────────────────

describe('MarkdownContent — CopyableBlock copy paths', () => {
  beforeEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
  });

  it('copies a fenced code block via the overlay button', () => {
    const { container } = render(
      <MarkdownContent content={'```\nconst x = 1;\n```'} />,
    );
    const btn = container.querySelector('button.disc-copyable-block-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    fireEvent.click(btn);
    expect(navigator.clipboard.writeText).toHaveBeenCalled();
    const arg = (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(arg).toContain('const x = 1;');
  });

  it('copies a markdown table as tab/newline-joined text (extractText TABLE path)', () => {
    const md = '| A | B |\n| --- | --- |\n| 1 | 2 |';
    const { container } = render(<MarkdownContent content={md} />);
    // The table is wrapped in its own CopyableBlock; grab the button
    // inside the element that contains a <table>.
    const tableWrapper = container.querySelector('table')!.closest('.relative') as HTMLElement;
    const btn = within(tableWrapper).getByRole('button');
    fireEvent.click(btn);
    const arg = (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    // Header + cells flattened with tab separators and newline between rows.
    expect(arg).toContain('A\tB');
    expect(arg).toContain('1\t2');
  });
});

describe('MarkdownContent — fence branches (discussionId-gated)', () => {
  it('renders a normal code block through CopyableBlock when no discussionId', () => {
    const { container } = render(<MarkdownContent content={'```js\nfoo()\n```'} />);
    expect(container.querySelector('pre')).not.toBeNull();
    expect(container.querySelector('button.disc-copyable-block-btn')).not.toBeNull();
  });

  it('falls through to a code block for a malformed kronn-doc-data fence', () => {
    // Invalid JSON inside the fence → the try/catch falls through to the
    // standard CopyableBlock render rather than crashing.
    const md = '```kronn-doc-data\n{ not valid json\n```';
    const { container } = render(<MarkdownContent content={md} discussionId="disc-x" />);
    expect(container.querySelector('pre')).not.toBeNull();
  });

  it('falls through to a code block for an unknown kronn-doc-data format', () => {
    const md = '```kronn-doc-data\n{"format":"docx","rows":[]}\n```';
    const { container } = render(<MarkdownContent content={md} discussionId="disc-x" />);
    // Unknown format → no DocDataExport, plain code block instead.
    expect(container.querySelector('pre')).not.toBeNull();
  });
});

describe('MessageBubble — role exhaustiveness sanity', () => {
  it.each<[MessageRole, string]>([
    ['User', 'user'],
    ['Agent', 'agent'],
    ['System', 'system'],
  ])('role %s → data-role %s', (role, expected) => {
    const { container } = renderBubble(makeMessage({ role, content: 'x' }));
    expect(container.querySelector('.disc-msg-row')?.getAttribute('data-role')).toBe(expected);
  });
});
