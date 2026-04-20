// Badge that nudges the user to commit uncommitted files when the agent
// finished a run in Isolated mode without a commit. Guards against silent
// regression of the UX fix added alongside the disc_prompts worktree notice.

import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
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
    message_count: 0,
    archived: false,
    pinned: false,
    workspace_mode: 'Isolated',
    worktree_branch: 'kronn/test-branch',
    workspace_path: '/tmp/worktree',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function renderHeader(pendingFilesCount: number, disc: Discussion = makeDiscussion()) {
  return render(
    <ChatHeader
      discussion={disc}
      projects={[]}
      agents={[]}
      availableSkills={[]}
      availableProfiles={[]}
      availableDirectives={[]}
      mcpConfigs={[]}
      mcpIncompatibilities={[]}
      showGitPanel={false}
      isMobile={false}
      sending={false}
      pendingFilesCount={pendingFilesCount}
      onRequestTestMode={noop}
      onToggleGitPanel={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={noop}
      onAgentSwitch={noop}
      contacts={[]}
      onShare={noop}
      toast={vi.fn()}
      t={t}
    />
  );
}

describe('ChatHeader — pending files badge', () => {
  it('shows no badge when pendingFilesCount is 0', () => {
    renderHeader(0);
    expect(document.querySelector('.disc-icon-btn-badge')).toBeNull();
  });

  it('shows the count inside the badge when pendingFilesCount > 0', () => {
    renderHeader(3);
    const badge = document.querySelector('.disc-icon-btn-badge');
    expect(badge).not.toBeNull();
    expect(badge!.textContent).toBe('3');
  });

  it('caps the displayed count at 9+ to avoid overflow', () => {
    renderHeader(27);
    expect(document.querySelector('.disc-icon-btn-badge')!.textContent).toBe('9+');
  });

  it('uses the pending-files tooltip (with count) instead of the default label', () => {
    renderHeader(5);
    // Select by aria-label (stable — identifies the git-panel icon among
    // multiple .disc-icon-btn elements in the header).
    const btn = screen.getByRole('button', { name: 'git.filesBtn' });
    expect(btn.getAttribute('title')).toBe('5 files pending');
  });

  it('falls back to the default label when there are no pending files', () => {
    renderHeader(0);
    const btn = screen.getByRole('button', { name: 'git.filesBtn' });
    expect(btn.getAttribute('title')).toBe('git.filesBtn');
  });

  it('is only rendered next to a project-scoped discussion', () => {
    // No project_id → no git button at all, so obviously no badge.
    renderHeader(5, makeDiscussion({ project_id: null }));
    expect(document.querySelector('.disc-icon-btn-badge')).toBeNull();
  });
});
