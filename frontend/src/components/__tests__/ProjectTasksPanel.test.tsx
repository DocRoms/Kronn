import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { planning } from '../../lib/api';
import { MAX_PLANNING_PAGES } from '../../lib/planningPages';
import type { PlanningTaskSummary } from '../../types/generated';
import { ProjectTasksPanel } from '../ProjectTasksPanel';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) =>
      args.length ? `${key} ${args.join(' ')}` : key,
  }),
}));

vi.mock('../../lib/api', () => ({
  planning: {
    list: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
  },
}));

const task: PlanningTaskSummary = {
  id: 'task-uuid',
  reference: 'KT-42',
  parent_id: null,
  parent_reference: null,
  parent_title: null,
  title: 'Ship planning',
  status: 'todo',
  priority: 'high',
  rank: 1024,
  completed_subtasks: 1,
  total_subtasks: 2,
  project_ids: ['project-1'],
  discussion_ids: ['discussion-1'],
  tags: [],
  blocker_count: 0,
  assignees: [],
  created_at: '2026-07-25T10:00:00Z',
  updated_at: '2026-07-25T10:00:00Z',
};

describe('ProjectTasksPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(planning.list).mockReset()
      .mockResolvedValue({ items: [task], next_cursor: null, total: 1, project_totals: {}, unassigned_total: 0 });
    vi.mocked(planning.create).mockReset().mockResolvedValue({} as never);
    vi.mocked(planning.update).mockReset().mockResolvedValue({} as never);
  });

  it('lists project tasks and updates their completion state', async () => {
    const onCountChange = vi.fn();
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={vi.fn()}
        onCountChange={onCountChange}
        toast={vi.fn()}
      />,
    );

    expect(await screen.findByText('Ship planning')).toBeInTheDocument();
    expect(planning.list).toHaveBeenCalledWith({ projectId: 'project-1', limit: 100 });
    expect(onCountChange).toHaveBeenCalledWith(1);

    fireEvent.click(screen.getByRole('button', { name: 'planning.markDone' }));
    await waitFor(() => {
      expect(planning.update).toHaveBeenCalledWith('task-uuid', { status: 'done' });
    });
  });

  it('reads every page and reports the backend total, not the size of a page', async () => {
    const onCountChange = vi.fn();
    const second = { ...task, id: 'task-2', reference: 'KT-43', title: 'Second page' };
    vi.mocked(planning.list).mockReset()
      .mockResolvedValueOnce({ items: [task], next_cursor: 100, total: 2, project_totals: {}, unassigned_total: 0 })
      .mockResolvedValueOnce({ items: [second], next_cursor: null, total: 2, project_totals: {}, unassigned_total: 0 });
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={vi.fn()}
        onCountChange={onCountChange}
        toast={vi.fn()}
      />,
    );

    expect(await screen.findByText('Second page')).toBeInTheDocument();
    expect(screen.getByText('Ship planning')).toBeInTheDocument();
    expect(planning.list).toHaveBeenNthCalledWith(2, { projectId: 'project-1', limit: 100, cursor: 100 });
    expect(onCountChange).toHaveBeenLastCalledWith(2);
  });

  it('says the list stopped when the page ceiling is reached, and still reports the total', async () => {
    const onCountChange = vi.fn();
    const total = MAX_PLANNING_PAGES * 100 + 7;
    vi.mocked(planning.list).mockReset().mockImplementation(async ({ cursor } = {}) => {
      const page = (cursor ?? 0) / 100;
      return {
        items: [{ ...task, id: `task-${page}`, reference: `KT-${page}`, title: `Task ${page}` }],
        next_cursor: (page + 1) * 100,
        total,
        project_totals: {},
        unassigned_total: 0,
      };
    });
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={vi.fn()}
        onCountChange={onCountChange}
        toast={vi.fn()}
      />,
    );

    expect(
      await screen.findByText(`projects.tasks.truncated ${MAX_PLANNING_PAGES} ${total}`),
    ).toBeInTheDocument();
    // No request goes past the ceiling, however many pages the total announces.
    const cursors = vi.mocked(planning.list).mock.calls.map(([filters]) => filters?.cursor ?? 0);
    expect(Math.max(...cursors)).toBe((MAX_PLANNING_PAGES - 1) * 100);
    expect(onCountChange).toHaveBeenLastCalledWith(total);
  });

  it('creates a task already linked to the project and opens global planning', async () => {
    const onOpenPlanning = vi.fn();
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={onOpenPlanning}
        toast={vi.fn()}
      />,
    );
    await screen.findByText('Ship planning');

    fireEvent.change(screen.getByRole('textbox', { name: 'planning.quickCreate' }), {
      target: { value: 'Review release' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'planning.newIdea' }));

    await waitFor(() => {
      expect(planning.create).toHaveBeenCalledWith({
        title: 'Review release',
        status: 'todo',
        priority: 'normal',
        project_ids: ['project-1'],
      });
    });

    fireEvent.click(screen.getByRole('button', { name: /projects.tasks.openPlanning/ }));
    expect(onOpenPlanning).toHaveBeenCalledWith();

    fireEvent.click(screen.getByRole('button', {
      name: 'projects.tasks.openTask KT-42',
    }));
    expect(onOpenPlanning).toHaveBeenLastCalledWith('task-uuid');
  });

  it('keeps the panel usable and reports a project-task load failure', async () => {
    const toast = vi.fn();
    vi.mocked(planning.list).mockReset().mockRejectedValue(new Error('offline'));

    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={vi.fn()}
        toast={toast}
      />,
    );

    await waitFor(() => {
      expect(toast).toHaveBeenCalledWith('projects.tasks.loadError', 'error');
    });
    expect(screen.getByText('projects.tasks.empty')).toBeInTheDocument();
  });

  it('opens a task in a new tab on Ctrl-click, without opening planning here', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const onOpenPlanning = vi.fn();
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={onOpenPlanning}
        toast={vi.fn()}
      />,
    );
    await screen.findByText('Ship planning');

    fireEvent.click(screen.getByRole('button', { name: 'projects.tasks.openTask KT-42' }), { ctrlKey: true });

    expect(open).toHaveBeenCalledWith(
      `${window.location.origin}/planning/task-uuid`, '_blank', 'noopener,noreferrer',
    );
    expect(onOpenPlanning).not.toHaveBeenCalled();
    open.mockRestore();
  });
  it('opens global planning in a new tab on Ctrl-click, without opening it here', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const onOpenPlanning = vi.fn();
    render(
      <ProjectTasksPanel
        projectId="project-1"
        onOpenPlanning={onOpenPlanning}
        toast={vi.fn()}
      />,
    );
    await screen.findByText('Ship planning');

    fireEvent.click(screen.getByRole('button', { name: /projects.tasks.openPlanning/ }), { ctrlKey: true });

    expect(open).toHaveBeenCalledWith(
      `${window.location.origin}/planning`, '_blank', 'noopener,noreferrer',
    );
    expect(onOpenPlanning).not.toHaveBeenCalled();
    open.mockRestore();
  });
});
