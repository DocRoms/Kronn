/**
 * 0.8.3 — End-of-validation "View Tech Debts" CTA regression suite.
 *
 * When an audit-validation discussion completes, the agent emits the
 * `KRONN:VALIDATION_COMPLETE` marker in its last message. The marker
 * is stripped from the rendered text (so the user doesn't see it) but
 * the MessageBubble inspects the raw content and surfaces a CTA
 * button that jumps the user to the project card with the docs/
 * tech-debt section pre-expanded.
 *
 * These tests pin:
 *   - CTA visible only when the message contains the marker AND we
 *     have a project id (orphan discussion → no jump target)
 *   - Click stores the `kronn:postValidation:<id>` sessionStorage flag
 *     AND navigates to `/projects/<id>`
 *   - Stripped marker doesn't leak into the rendered text
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router';
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

function makeAgentMessage(content: string): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'Agent',
    content,
    agent_type: 'ClaudeCode',
    timestamp: new Date().toISOString(),
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
  };
}

const baseProps = {
  idx: 0,
  isLastUser: false,
  isLastAgent: true,
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
  // identity translator so tests assert on i18n keys verbatim
  t: (key: string) => key,
};

function renderWithRouter(ui: React.ReactElement, initialPath = '/') {
  const router = createMemoryRouter(
    [{ path: '*', element: ui }],
    { initialEntries: [initialPath] },
  );
  const result = render(<RouterProvider router={router} />);
  return { router, ...result };
}

describe('MessageBubble — KRONN:VALIDATION_COMPLETE CTA', () => {
  it('renders the CTA when message has the marker AND projectId is provided', () => {
    renderWithRouter(
      <I18nProvider>
        <MessageBubble
          {...baseProps}
          msg={makeAgentMessage('Audit finished.\n\nKRONN:VALIDATION_COMPLETE')}
          projectId="proj-xyz"
        />
      </I18nProvider>
    );
    // The i18n key is used verbatim (identity translator).
    expect(screen.getByText('audit.viewTechDebtsAfterValidation')).toBeInTheDocument();
  });

  it('hides the CTA when projectId is null (orphan discussion)', () => {
    // A discussion that isn't bound to a project has nowhere to send
    // the user — surfacing a CTA would dead-end. Verify it stays hidden.
    renderWithRouter(
      <I18nProvider>
        <MessageBubble
          {...baseProps}
          msg={makeAgentMessage('Done.\n\nKRONN:VALIDATION_COMPLETE')}
          projectId={null}
        />
      </I18nProvider>
    );
    expect(screen.queryByText('audit.viewTechDebtsAfterValidation')).toBeNull();
  });

  it('hides the CTA when the marker is absent (normal message)', () => {
    renderWithRouter(
      <I18nProvider>
        <MessageBubble
          {...baseProps}
          msg={makeAgentMessage('Hey, just a normal answer with no marker.')}
          projectId="proj-xyz"
        />
      </I18nProvider>
    );
    expect(screen.queryByText('audit.viewTechDebtsAfterValidation')).toBeNull();
  });

  it('clicking the CTA sets sessionStorage deeplink + navigates to the project', () => {
    // 0.8.3 (#314) — the CTA must do TWO things atomically:
    //   - write `sessionStorage[kronn:postValidation:<id>]` so the
    //     ProjectCard auto-opens docs/tech-debt on mount
    //   - navigate to /projects/<id> via the router
    sessionStorage.clear();
    const { router } = renderWithRouter(
      <I18nProvider>
        <MessageBubble
          {...baseProps}
          msg={makeAgentMessage('Validated.\n\nKRONN:VALIDATION_COMPLETE')}
          projectId="proj-xyz"
        />
      </I18nProvider>
    );
    fireEvent.click(screen.getByText('audit.viewTechDebtsAfterValidation'));
    expect(sessionStorage.getItem('kronn:postValidation:proj-xyz')).toBe('docs/tech-debt');
    expect(router.state.location.pathname).toBe('/projects/proj-xyz');
  });

  it('stripped marker does not leak into the rendered text', () => {
    // The marker is hidden in MarkdownContent — only the CTA reveals
    // that a validation just finished. Asserts the regex strip is
    // still working for VALIDATION_COMPLETE alongside its siblings.
    renderWithRouter(
      <I18nProvider>
        <MessageBubble
          {...baseProps}
          msg={makeAgentMessage('All good.\n\nKRONN:VALIDATION_COMPLETE')}
          projectId="proj-xyz"
        />
      </I18nProvider>
    );
    // The raw marker must not survive in the rendered DOM text.
    expect(document.body.textContent ?? '').not.toContain('KRONN:VALIDATION_COMPLETE');
  });
});
