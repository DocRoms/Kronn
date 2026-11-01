/**
 * KT-1344 — the discussion header's third row is gone: the id, the project,
 * the worktree state and the test-mode CTA live in the side panel's settings
 * tab. These assertions moved here from the header's own specs; what they
 * guard did not change, only where it is rendered.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());

import { discussions as discussionsApi } from '../../lib/api';
import { DiscussionDetailsSection } from '../DiscussionDetailsSection';
import type { Discussion, Project } from '../../types/generated';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

function makeDiscussion(overrides: Partial<Discussion> = {}): Discussion {
  return {
    id: 'disc-details-0000-1111',
    project_id: null,
    title: 'Details',
    agent: 'ClaudeCode',
    language: 'fr',
    participants: ['ClaudeCode'],
    messages: [],
    message_count: 0,
    non_system_message_count: 0,
    tier: 'default',
    summary_strategy: 'OnDemand',
    introspection_call_count: 0,
    archived: false,
    pinned: false,
    pin_first_message: false,
    workspace_mode: 'Direct',
    workspace_path: null,
    created_at: '2026-07-24T00:00:00Z',
    updated_at: '2026-07-24T00:00:00Z',
    awaiting_agent: false,
    ...overrides,
  };
}

function renderSection(
  discussion: Discussion = makeDiscussion(),
  extra: { projects?: Project[]; onRequestTestMode?: () => void } = {},
) {
  return render(
    <DiscussionDetailsSection
      discussion={discussion}
      projects={extra.projects ?? []}
      onRequestTestMode={extra.onRequestTestMode ?? vi.fn()}
      onDiscussionUpdated={vi.fn()}
      toast={vi.fn()}
      t={t}
    />,
  );
}

describe('DiscussionDetailsSection', () => {
  it('does not carry the discussion id: it sits next to the title in the header (KT-1530)', () => {
    const { container } = renderSection();
    expect(container.querySelector('.disc-id-pill')).toBeNull();
    expect(container.querySelector('[data-tour-id="discussion-id-pill"]')).toBeNull();
  });

  it('names the project, or the general scope when there is none', () => {
    const project = { id: 'project-1', name: 'Kronn' } as Project;
    const { container, rerender } = renderSection(
      makeDiscussion({ project_id: 'project-1' }),
      { projects: [project] },
    );
    expect(container.querySelector('.disc-chat-context-project')).toHaveTextContent('Kronn');

    rerender(
      <DiscussionDetailsSection
        discussion={makeDiscussion()}
        projects={[project]}
        onRequestTestMode={vi.fn()}
        onDiscussionUpdated={vi.fn()}
        toast={vi.fn()}
        t={t}
      />,
    );
    expect(container.querySelector('.disc-chat-context-project')).toHaveTextContent('disc.general');
  });

  it('shows declared joined-CLI worktrees', async () => {
    vi.mocked(discussionsApi.workspaces).mockResolvedValue([{
      id: 'workspace-1',
      disc_id: 'disc-details-0000-1111',
      session_pk: 42,
      session_agent_type: 'Codex',
      task_id: 'task-140',
      task_reference: 'KT-140',
      project_id: 'project-1',
      workspace_path: '/tmp/kronn-kt140',
      canonical_path: '/tmp/kronn-kt140',
      branch: 'feature/kt140',
      head_sha: 'abc123',
      ownership: 'external',
      state: 'attached',
      parent_discussion_id: null,
      base_sha: null,
      task_execution_id: null,
      created_at: '2026-01-01',
      updated_at: '2026-01-01',
    }]);

    renderSection();

    expect(await screen.findByText('feature/kt140')).toBeInTheDocument();
    expect(screen.getByText('KT-140')).toBeInTheDocument();
    expect(screen.getByText('Codex')).toBeInTheDocument();
  });

  it('offers the test-mode CTA only on an active isolated worktree', () => {
    const onRequestTestMode = vi.fn();
    const isolated = makeDiscussion({
      workspace_mode: 'Isolated',
      worktree_branch: 'kronn/disc/x',
      workspace_path: '/tmp/wt',
    });
    const { unmount } = renderSection(isolated, { onRequestTestMode });
    fireEvent.click(screen.getByText('testMode.cta'));
    expect(onRequestTestMode).toHaveBeenCalledTimes(1);
    unmount();

    renderSection(makeDiscussion());
    expect(screen.queryByText('testMode.cta')).toBeNull();
  });
});
