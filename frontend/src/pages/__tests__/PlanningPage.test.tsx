import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  get: vi.fn(),
  update: vi.fn(),
  removeBlocker: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  planning: {
    ...mocks,
    linkDiscussion: vi.fn(),
    addBlocker: vi.fn(),
    removeBlocker: mocks.removeBlocker,
    discussionPlan: vi.fn(),
    changes: vi.fn(),
  },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));

import { PlanningPage } from '../PlanningPage';
import type { PlanningTaskDetail, PlanningTaskSummary } from '../../types/generated';

function summary(overrides: Partial<PlanningTaskSummary> = {}): PlanningTaskSummary {
  return {
    id: 'task-1',
    reference: 'KT-1',
    parent_id: null,
    parent_reference: null,
    parent_title: null,
    title: 'Upgrade PHP',
    status: 'todo',
    priority: 'high',
    rank: 1024,
    completed_subtasks: 0,
    total_subtasks: 0,
    project_ids: [],
    discussion_ids: [],
    tags: ['platform'],
    blocker_count: 0,
    created_at: '2026-07-25T00:00:00Z',
    updated_at: '2026-07-25T00:00:00Z',
    ...overrides,
  };
}

function detail(task: PlanningTaskSummary = summary()): PlanningTaskDetail {
  return {
    ...task,
    subtasks: [],
    description: 'Move the runtime forward.',
    blocked_reason: null,
    definition_of_done: [],
    links: [],
    blockers: [],
    blocking: [],
    events: [],
  };
}

describe('PlanningPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    localStorage.clear();
    vi.stubGlobal('confirm', () => true);
    mocks.list.mockResolvedValue({
      items: [
        summary(),
        summary({
          id: 'task-done',
          reference: 'KT-2',
          title: 'Old task',
          status: 'done',
          priority: 'low',
        }),
      ],
      next_cursor: null,
    });
    mocks.create.mockResolvedValue(detail(summary({
      id: 'task-new',
      reference: 'KT-3',
      title: 'New idea',
      status: 'idea',
      priority: 'critical',
    })));
    mocks.get.mockResolvedValue(detail());
    mocks.update.mockImplementation(async (_id, patch) => detail({ ...summary(), ...patch }));
    mocks.removeBlocker.mockResolvedValue(detail());
  });

  it('renders tasks in the shared collection sidebar', async () => {
    const { container } = render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    expect(await screen.findByText('Upgrade PHP')).toBeInTheDocument();
    expect(container.querySelector('.collection-shell-sidebar')).not.toBeNull();
    expect(screen.getByText('Old task')).toBeInTheDocument();
  });

  it('uses the shared title, favorites, collapse, and bulk archive flow', async () => {
    render(<PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    await screen.findByText('Upgrade PHP');
    expect(screen.getByText('planning.title')).toHaveTextContent('planning.title · 2');
    fireEvent.click(screen.getByRole('button', { name: /collection\.favorites · Upgrade PHP/ }));
    expect(localStorage.getItem('kronn:collection-favorites:planning')).toContain('task-1');
    fireEvent.click(screen.getByRole('button', { name: 'collection.favorites' }));
    expect(screen.getByText('Upgrade PHP')).toBeInTheDocument();
    expect(screen.queryByText('Old task')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'collection.favorites' }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.moreActions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'collection.selectMultiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Upgrade PHP.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Old task.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.archiveSelected' }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledTimes(2));
    expect(mocks.update).toHaveBeenCalledWith('task-1', { status: 'archived' });
    expect(mocks.update).toHaveBeenCalledWith('task-done', { status: 'archived' });
    fireEvent.click(screen.getByRole('button', { name: 'collection.closeCollection' }));
    expect(screen.queryByRole('complementary', { name: 'planning.title' })).toBeNull();
    expect(screen.getByRole('button', { name: 'collection.openCollection' })).toHaveClass('collection-shell-sidebar-rail');
  });

  it('keeps Planning task rows in the shared keyboard and active-row contract', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    const first = (await screen.findAllByRole('button', { name: /Upgrade PHP/ }))
      .find(button => button.classList.contains('collection-shell-row-button'))!;
    const second = screen.getAllByRole('button', { name: /Old task/ })
      .find(button => button.classList.contains('collection-shell-row-button'))!;
    first.focus();
    fireEvent.keyDown(first, { key: 'ArrowDown' });
    expect(second).toHaveFocus();

    fireEvent.click(second);
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-done'));
    expect(second).toHaveAttribute('aria-current', 'true');
  });

  it('opens task creation from the compact header action and removes the inline form', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await screen.findByText('Upgrade PHP');
    expect(screen.queryByPlaceholderText('planning.newIdea')).toBeNull();

    const trigger = screen.getByRole('button', { name: 'planning.quickCreate' });
    expect(trigger).toHaveClass('collection-shell-icon', 'collection-shell-primary-action');
    fireEvent.click(trigger);

    const dialog = screen.getByRole('dialog', { name: 'planning.createTitle' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    const input = screen.getByRole('textbox', { name: 'planning.taskTitle' });
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: 'New idea' } });
    fireEvent.change(screen.getByRole('combobox', { name: 'planning.priorityField' }), {
      target: { value: 'critical' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'planning.createAction' }));

    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'New idea',
      priority: 'critical',
      status: 'idea',
    }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(trigger).toHaveFocus();
  });

  it('closes task creation with Escape or the backdrop and restores header focus', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await screen.findByText('Upgrade PHP');
    const trigger = screen.getByRole('button', { name: 'planning.quickCreate' });

    fireEvent.click(trigger);
    const input = screen.getByRole('textbox', { name: 'planning.taskTitle' });
    fireEvent.change(input, { target: { value: 'Preserved draft' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();

    fireEvent.click(trigger);
    expect(screen.getByRole('textbox', { name: 'planning.taskTitle' })).toHaveValue('Preserved draft');
    const backdrop = screen.getByRole('dialog').parentElement;
    expect(backdrop).toHaveClass('planning-create-modal-backdrop');
    fireEvent.mouseDown(backdrop!);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it('labels filter controls for assistive technology', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await screen.findByText('Upgrade PHP');

    const search = screen.getByRole('textbox', { name: 'planning.search' });
    fireEvent.keyDown(window, { key: '/' });
    expect(search).toHaveFocus();
    fireEvent.change(search, { target: { value: 'Upgrade' } });
    const searchHeader = search.closest<HTMLElement>('.collection-shell-header')!;
    fireEvent.click(within(searchHeader).getByRole('button', { name: 'collection.clearFilters' }));
    expect(search).toHaveValue('');

    const filterButton = within(searchHeader).getByRole('button', { name: 'planning.filters' });
    expect(filterButton).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(filterButton.querySelector('span')).toBeNull();
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(filterButton).not.toHaveAttribute('aria-controls');
    expect(screen.queryByRole('combobox', { name: 'planning.allStatuses' })).toBeNull();

    fireEvent.click(filterButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'true');
    expect(filterButton).toHaveAttribute('aria-controls', 'planning-search-options');
    expect(screen.getByRole('combobox', { name: 'planning.allStatuses' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'planning.allProjects' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'planning.allLinks' })).toBeInTheDocument();

    fireEvent.change(screen.getByRole('combobox', { name: 'planning.allStatuses' }), { target: { value: 'todo' } });
    expect(filterButton).toHaveAttribute('data-active', 'true');
    expect(filterButton.querySelector('strong')).toBeNull();
    await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'todo' })));
    fireEvent.click(filterButton);
    expect(screen.queryByRole('combobox', { name: 'planning.allStatuses' })).toBeNull();
  });

  it('opens the full detail only after a task is selected', async () => {
    const { container } = render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByText('Upgrade PHP'));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByDisplayValue('Move the runtime forward.')).toBeInTheDocument();
    const panel = screen.getByRole('complementary', { name: 'planning.taskActions' });
    expect(panel.querySelector(':scope > header')).not.toBeNull();
    expect(panel.querySelector('.planning-detail-form')).not.toBeNull();
    expect(container.querySelector('.collection-shell-detail > .planning-detail')).toBe(panel);
  });

  it('opens a directly linked task detail on mount', async () => {
    render(
      <PlanningPage
        initialSelectedTaskId="task-1"
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByDisplayValue('Move the runtime forward.')).toBeInTheDocument();
  });

  it('keeps a valid initial selection that is outside the first loaded task page', async () => {
    const outsideFirstPage = summary({
      id: 'task-outside-first-page',
      reference: 'KT-101',
      title: 'Outside first page',
    });
    mocks.list.mockResolvedValueOnce({ items: [summary()], next_cursor: null });
    mocks.get.mockResolvedValueOnce(detail(outsideFirstPage));
    render(
      <PlanningPage
        initialSelectedTaskId={outsideFirstPage.id}
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await screen.findByText('Upgrade PHP');
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith(outsideFirstPage.id));
    expect(await screen.findByDisplayValue('Move the runtime forward.')).toBeInTheDocument();
    expect(screen.getByRole('complementary', { name: 'planning.taskActions' })).toBeInTheDocument();
  });

  it('clears a directly linked selection when its detail cannot be loaded', async () => {
    const toast = vi.fn();
    mocks.get.mockRejectedValueOnce(new Error('Task no longer exists'));
    render(
      <PlanningPage
        initialSelectedTaskId="missing-task"
        projects={[]}
        discussions={[]}
        toast={toast}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('missing-task'));
    expect(await screen.findByText('planning.selectHint')).toBeInTheDocument();
    expect(screen.queryByRole('complementary', { name: 'planning.taskActions' })).toBeNull();
    expect(toast).toHaveBeenCalled();
  });

  it('opens a linked discussion with the exact workspace selected for its Git panel', async () => {
    const linked = detail();
    linked.workspaces = [{
      id: 'workspace-2',
      disc_id: 'disc-2',
      branch: 'feature/kt-140',
      state: 'attached',
      ownership: 'external',
      session_agent_type: 'Codex',
    }];
    mocks.get.mockResolvedValue(linked);
    const onNavigateDiscussion = vi.fn();
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={onNavigateDiscussion}
      />,
    );

    fireEvent.click(await screen.findByText('Upgrade PHP'));
    fireEvent.click(await screen.findByText('feature/kt-140'));

    expect(onNavigateDiscussion).toHaveBeenCalledWith('disc-2');
    expect(JSON.parse(
      sessionStorage.getItem('kronn:discussion-workspace-target') ?? '{}',
    )).toEqual({
      discussionId: 'disc-2',
      workspaceId: 'workspace-2',
    });
  });

  it('creates a subtask from the detail panel with inherited priority and project links', async () => {
    const parent = detail({
      ...summary(),
      project_ids: ['project-1'],
    });
    mocks.get.mockResolvedValue(parent);
    mocks.create.mockResolvedValue(detail(summary({
      id: 'child-1',
      reference: 'KT-2',
      parent_id: 'task-1',
      parent_reference: 'KT-1',
      parent_title: 'Upgrade PHP',
      title: 'Update CI image',
    })));
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByText('Upgrade PHP'));
    const input = await screen.findByPlaceholderText('planning.addSubtask');
    fireEvent.change(input, { target: { value: 'Update CI image' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'Update CI image',
      status: 'todo',
      priority: 'high',
      parent_id: 'task-1',
      project_ids: ['project-1'],
    }));
  });

  it('offers to unblock a task when every blocker is done or archived', async () => {
    const blocked = detail(summary({ status: 'blocked', blocker_count: 0 }));
    blocked.blocked_reason = 'Waiting for the old migration.';
    blocked.blockers = [
      summary({
        id: 'blocker-archived',
        reference: 'KT-9',
        title: 'Legacy migration',
        status: 'archived',
      }),
    ];
    mocks.get.mockResolvedValue(blocked);

    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    fireEvent.click(await screen.findByText('Upgrade PHP'));
    fireEvent.click(await screen.findByText('planning.unblock'));

    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', {
      status: 'todo',
      blocked_reason: null,
    }));
  });

  it('removes one blocker without changing the task status', async () => {
    const blocked = detail(summary({ status: 'blocked', blocker_count: 1 }));
    blocked.blockers = [summary({
      id: 'blocker-9',
      reference: 'KT-9',
      title: 'Dependency',
      status: 'todo',
    })];
    mocks.get.mockResolvedValue(blocked);
    mocks.removeBlocker.mockResolvedValue(detail(summary({
      status: 'blocked',
      blocker_count: 0,
    })));

    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    fireEvent.click(await screen.findByText('Upgrade PHP'));
    fireEvent.click(await screen.findByRole('button', {
      name: 'planning.removeBlocker:KT-9',
    }));

    await waitFor(() => expect(mocks.removeBlocker).toHaveBeenCalledWith(
      'task-1',
      'blocker-9',
    ));
    expect(mocks.update).not.toHaveBeenCalled();
  });
});
