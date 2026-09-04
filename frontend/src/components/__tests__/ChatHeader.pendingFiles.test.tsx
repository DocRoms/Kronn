// Badge that nudges the user to commit uncommitted files when the agent
// finished a run in Isolated mode without a commit. Guards against silent
// regression of the UX fix added alongside the disc_prompts worktree notice.

import { describe, it, expect, vi, afterEach } from 'vitest';
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
  pendingFilesCount: number,
  disc: Discussion = makeDiscussion(),
  pendingProposalItemCount = 0,
) {
  return render(
    <ChatHeader
      discussion={disc}
      projects={[]}
      agents={[]}
      showGitPanel={false}
      isMobile={false}
      sending={false}
      pendingFilesCount={pendingFilesCount}
      pendingProposalCount={pendingProposalItemCount > 0 ? 1 : 0}
      pendingProposalItemCount={pendingProposalItemCount}
      onRequestTestMode={noop}
      onToggleGitPanel={noop}
      onToggleSettingsPanel={noop}
      onTogglePlanPanel={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={noop}
      onAgentSwitch={noop}
      toast={vi.fn()}
      t={t}
    />
  );
}

afterEach(() => {
  // The rail persists its folded state; without this, a spec inherits the
  // rail left open by the previous one.
  localStorage.removeItem('kronn:panelRailExpanded');
});

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

  // KT-581 — the panels moved into a rail that starts folded, so a test
  // looking for one has to open it first. What each test guards is unchanged:
  // the badge, its cap, and the tooltip that names the count.
  // Idempotent on purpose: the rail remembers whether it was open, so a blind
  // click would CLOSE it in every test after the first. Asserting on the state
  // rather than clearing storage also matches what a returning reader sees.
  const openRail = () => {
    const toggle = screen.getByTestId('panel-rail-toggle');
    if (toggle.getAttribute('aria-expanded') !== 'true') fireEvent.click(toggle);
  };

  it('keeps the Code action visible when pendingFilesCount is 0', () => {
    renderHeader(0);
    openRail();
    expect(document.querySelector('.disc-panel-rail-badge')).toBeNull();
    expect(screen.getByRole('button', { name: /git\.filesBtn/ })).toBeInTheDocument();
  });

  it('shows the count inside the badge when pendingFilesCount > 0', () => {
    renderHeader(3);
    openRail();
    const badge = document.querySelector('[data-panel="git"] .disc-panel-rail-badge');
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe('3');
  });

  it('caps the displayed count at 9+ to avoid overflow', () => {
    renderHeader(27);
    openRail();
    expect(
      document.querySelector('[data-panel="git"] .disc-panel-rail-badge')!.textContent,
    ).toBe('9+');
  });

  it('uses the pending-files tooltip (with count) instead of the default label', () => {
    renderHeader(5);
    openRail();
    // The count belongs in the name now: the rail renders its label as text,
    // so a reader sees "5 files pending" instead of a bare "Code" whose
    // tooltip they must hover to understand.
    const btn = screen.getByRole('button', { name: /5 files pending/ });
    expect(btn.getAttribute('title')).toBe('5 files pending');
  });

  it('remains available when the discussion has no direct project', () => {
    renderHeader(5, makeDiscussion({ project_id: null }));
    openRail();
    expect(screen.getByRole('button', { name: /5 files pending/ })).toBeInTheDocument();
    expect(
      document.querySelector('[data-panel="git"] .disc-panel-rail-badge')?.textContent,
    ).toBe('5');
  });

  it('shows pending planning items inside the plan button', () => {
    renderHeader(0, makeDiscussion(), 3);
    openRail();
    expect(
      document.querySelector('[data-panel="plan"] .disc-panel-rail-badge')?.textContent,
    ).toBe('3');
  });
});
