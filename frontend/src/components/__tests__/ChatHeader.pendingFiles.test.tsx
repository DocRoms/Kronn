// Badge that nudges the user to commit uncommitted files when the agent
// finished a run in Isolated mode without a commit. Guards against silent
// regression of the UX fix added alongside the disc_prompts worktree notice.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
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
      agents={[]}
      isMobile={false}
      sending={false}
      onOpenDetails={noop}
      onToggleSidebar={noop}
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

  // KT-581 — the pending-file badge, its cap and its tooltip moved with the
  // panels themselves: they are asserted in DiscussionPanelSwitcher's spec.
  // What stays here is what the header still owns.
  it('carries no panel buttons at all', () => {
    renderHeader(0);
    // Every one of them, including the control that opens the column, now
    // lives in the panel's own column: a button that opens a panel has to sit
    // against it, and here a row of counters came between the two.
    expect(screen.queryByTestId('panel-open-toggle')).toBeNull();
    expect(screen.queryByRole('button', { name: 'git.filesBtn' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'planning.openPlan' })).toBeNull();
  });

  it('carries no action either — export and delete moved with the panels', () => {
    renderHeader(0);
    // The header describes the discussion; it no longer performs anything on
    // it. Export and delete live in the panel column's strip; the in-room
    // search is gone, the discussion search covers the whole content.
    expect(screen.queryByRole('button', { name: 'disc.portability.export' })).toBeNull();
  });
});
