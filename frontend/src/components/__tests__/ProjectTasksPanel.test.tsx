import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { planning } from '../../lib/api';
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
  created_at: '2026-07-25T10:00:00Z',
  updated_at: '2026-07-25T10:00:00Z',
};

describe('ProjectTasksPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(planning.list).mockReset()
      .mockResolvedValue({ items: [task], next_cursor: null });
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
});
