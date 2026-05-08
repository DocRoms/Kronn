/**
 * Regression for the `[kronn-internal: …]` System-message badge.
 *
 * The backend persists tool-call traces (post-stream MCP capture for
 * Claude Code et al, slash-marker resolver for Vibe / Ollama) as
 * System messages with a fixed `[kronn-internal: <tool>(<args>) → <result>]`
 * shape. MessageBubble parses that shape and renders a compact
 * yellow-tinted badge instead of a raw system-message error block —
 * the user has to recognise tool activity at a glance.
 *
 * If a refactor breaks the parser regex or drops the special-case
 * variant, these tests fire so we don't silently fall back to the
 * default System-message styling.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock the boot config call so I18nProvider doesn't try to fetch.
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    config: { getUiLanguage: vi.fn().mockResolvedValue('fr') },
  };
});

import { MessageBubble } from '../MessageBubble';
import type { DiscussionMessage } from '../../types/generated';

function makeSysMessage(content: string): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'System',
    content,
    agent_type: null,
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
  };
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
  // Pass-through stub: identity for the key so tests can match label
  // text in the rendered DOM regardless of locale.
  t: (key: string) => key,
};

describe('MessageBubble — kronn-internal tool-call badge', () => {
  it('renders disc_meta call as a tool badge (no args)', () => {
    render(
      <I18nProvider>
        <MessageBubble {...noopProps} msg={makeSysMessage('[kronn-internal: disc_meta()]')} />
      </I18nProvider>
    );
    const badge = screen.getByTestId('kronn-tool-badge');
    expect(badge).toBeInTheDocument();
    // Tool name visible
    expect(badge.textContent).toContain('disc_meta');
    // 🔧 wrench icon present (mirrors the ChatHeader pill)
    expect(badge.textContent).toContain('🔧');
  });

  it('renders disc_get_message with idx argument', () => {
    render(
      <I18nProvider>
        <MessageBubble {...noopProps} msg={makeSysMessage('[kronn-internal: disc_get_message(4)]')} />
      </I18nProvider>
    );
    const badge = screen.getByTestId('kronn-tool-badge');
    expect(badge.textContent).toContain('disc_get_message');
    expect(badge.textContent).toContain('(4)');
  });

  it('renders disc_summarize with range', () => {
    render(
      <I18nProvider>
        <MessageBubble {...noopProps} msg={makeSysMessage('[kronn-internal: disc_summarize(0..10)]')} />
      </I18nProvider>
    );
    const badge = screen.getByTestId('kronn-tool-badge');
    expect(badge.textContent).toContain('disc_summarize');
    expect(badge.textContent).toContain('(0..10)');
  });

  it('exposes a collapsed result for slash-marker payloads (Vibe/Ollama)', () => {
    // Slash-marker resolutions include the full payload after `→ `.
    // It renders as a `<details>` element, collapsed by default so it
    // doesn't dominate the transcript.
    render(
      <I18nProvider>
        <MessageBubble
          {...noopProps}
          msg={makeSysMessage(
            '[kronn-internal: disc_get_message(4) → User message: "Le commit fautif est abc1234"]'
          )}
        />
      </I18nProvider>
    );
    const badge = screen.getByTestId('kronn-tool-badge');
    // Result is in the DOM (inside the <details>) but collapsed.
    expect(badge.textContent).toContain('abc1234');
    // The <details> wrapper is closed by default.
    const details = badge.querySelector('details');
    expect(details).not.toBeNull();
    expect(details?.hasAttribute('open')).toBe(false);
  });

  it('does NOT render the kronn badge for regular system messages', () => {
    // Sanity: error / summary system messages still go through the
    // default `disc-msg-agent-label` path, not the kronn-tool variant.
    render(
      <I18nProvider>
        <MessageBubble {...noopProps} msg={makeSysMessage('summary cached for last 10 messages')} />
      </I18nProvider>
    );
    expect(screen.queryByTestId('kronn-tool-badge')).toBeNull();
  });

  it('falls back gracefully on a malformed kronn-internal message', () => {
    // Defensive: backend bug or hand-edited content shouldn't crash the
    // bubble. The badge still renders (showing the raw text) so the
    // user knows something happened.
    render(
      <I18nProvider>
        <MessageBubble
          {...noopProps}
          msg={makeSysMessage('[kronn-internal: malformed_no_closing_bracket')}
        />
      </I18nProvider>
    );
    // Still considered a kronn-tool message by the prefix match — we
    // suppress the markdown render and show the raw fallback in the
    // badge label. Either is acceptable; we just want no crash.
    const maybeBadge = screen.queryByTestId('kronn-tool-badge');
    expect(maybeBadge).not.toBeNull();
  });
});
