// Badge that nudges the user to commit uncommitted files when the agent
// finished a run in Isolated mode without a commit. Guards against silent
// regression of the UX fix added alongside the disc_prompts worktree notice.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { ChatHeader } from '../ChatHeader';
import type { Discussion } from '../../types/generated';

const noop = () => {};
// Positional-argument-aware t() so the tooltip substitution ({0}) renders the count.
const t = (key: string, ...args: (string | number)[]) => {
  if (key === 'git.pendingFilesTooltip') return `${args[0]} files pending`;
  return key;
};

function makeDiscussion(overrides: Partial<Discussion> = {}): Discussion {
  return {
    id: 'd-1',
    project_id: 'p-1',
    title: 'Test discussion',
    agent: 'ClaudeCode' as any,
    language: 'en',
    participants: ['ClaudeCode' as any],
    messages: [],
    message_count: 0, non_system_message_count: 0, tier: "default" as const, summary_strategy: "OnDemand" as const, introspection_call_count: 0,
    archived: false,
    pinned: false, pin_first_message: false,
    workspace_mode: 'Isolated',
    worktree_branch: 'kronn/test-branch',
    workspace_path: '/tmp/worktree',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    awaiting_agent: false,
    ...overrides,
  };
}

function renderHeader(
  _pendingFilesCount: number,
  disc: Discussion = makeDiscussion(),
  _pendingProposalItemCount = 0,
) {
  return render(
    <ChatHeader
      discussion={disc}
      projects={[]}
      agents={[]}
      isMobile={false}
      sending={false}
      onRequestTestMode={noop}
      onOpenPanels={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={noop}
      onAgentSwitch={noop}
      toast={vi.fn()}
      t={t}
    />
  );
}

describe('ChatHeader — pending files badge', () => {
  it('exposes stable grouped tour anchors and durable discussion help', () => {
    const { container } = renderHeader(0);

    expect(container.querySelector('[data-tour-id="disc-header-controls"]')).not.toBeNull();
    expect(container.querySelector('[data-tour-id="disc-identity-controls"]')).not.toBeNull();
    expect(container.querySelector('[data-tour-id="disc-output-controls"]')).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'contextHelp.discussion.title' }));
    expect(screen.getByText('contextHelp.discussion.mainAgent')).toBeInTheDocument();
    expect(screen.getByText('contextHelp.discussion.participants')).toBeInTheDocument();
    expect(screen.getByText('contextHelp.discussion.messages')).toBeInTheDocument();
    expect(screen.getByText('contextHelp.discussion.outputs')).toBeInTheDocument();
    expect(screen.getByText('contextHelp.discussion.mcp')).toBeInTheDocument();
  });

  it('shows copied feedback on the discussion ID pill while copying the full ID', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    const id = '12345678-90ab-cdef-1234-567890abcdef';
    const { container } = renderHeader(0, makeDiscussion({ id }));
    const pill = container.querySelector<HTMLButtonElement>('.disc-id-pill');

    fireEvent.click(pill!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(id));
    expect(pill?.getAttribute('data-copied')).toBe('true');
    expect(pill?.querySelector('svg')).not.toBeNull();
  });

  // KT-581 — the pending-file badge, its cap and its tooltip moved with the
  // panels themselves: they are asserted in DiscussionPanelSwitcher's spec.
  // What stays here is what the header still owns.
  it('keeps a single control for the panel column instead of one per panel', () => {
    renderHeader(0);
    expect(screen.getByTestId('panel-open-toggle')).toBeInTheDocument();
    // The six panel buttons are gone from the row; only search and this one
    // remain, plus the export and delete actions.
    expect(screen.queryByRole('button', { name: 'git.filesBtn' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'planning.openPlan' })).toBeNull();
  });

  it('leaves the panel control closed while nothing is open', () => {
    renderHeader(0);
    expect(screen.getByTestId('panel-open-toggle')).toHaveAttribute('aria-expanded', 'false');
  });
});
