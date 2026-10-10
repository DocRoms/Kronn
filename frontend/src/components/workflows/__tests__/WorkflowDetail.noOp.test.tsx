// KT-1100 — runs that changed nothing: the toggle that shows them, the
// folded streak row, and the greyed compact row.
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

vi.mock('../../../lib/api', () => buildApiMock({
  workflows: { listBatchRunSummaries: vi.fn().mockResolvedValue([]) },
}));

import { WorkflowDetail } from '../WorkflowDetail';
import type { Workflow, WorkflowRun, WorkflowTrigger } from '../../../types/generated';

const workflow = {
  id: 'wf-1',
  name: 'État de la page',
  project_id: null,
  trigger: { type: 'Cron', schedule: '* * * * *' } as WorkflowTrigger,
  steps: [],
  actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null,
  concurrency_limit: null,
  enabled: true,
  created_at: '2026-10-01T00:00:00Z',
  updated_at: '2026-10-01T00:00:00Z',
} as unknown as Workflow;

const run = (id: string, minute: number, outcome?: WorkflowRun['outcome']): WorkflowRun => ({
  id,
  workflow_id: 'wf-1',
  status: 'Success',
  trigger_context: null,
  step_results: [],
  tokens_used: 0,
  workspace_path: null,
  started_at: `2026-10-09T08:${String(minute).padStart(2, '0')}:00Z`,
  finished_at: `2026-10-09T08:${String(minute).padStart(2, '0')}:30Z`,
  produced_branches: [],
  ...(outcome ? { outcome } : {}),
} as unknown as WorkflowRun);

type Props = React.ComponentProps<typeof WorkflowDetail>;

const renderDetail = (overrides: Partial<Props>) => render(
  <WorkflowDetail
    workflow={workflow}
    runs={[]}
    liveRun={null}
    onTrigger={vi.fn()}
    onRefresh={vi.fn()}
    onEdit={vi.fn()}
    onDeleteRun={vi.fn()}
    onDeleteAllRuns={vi.fn()}
    triggering={false}
    {...overrides}
  />,
);

afterEach(() => cleanup());

describe('WorkflowDetail — runs without changes', () => {
  it('offers to show the hidden runs and says so when nothing else is left', () => {
    const onToggleNoOpRuns = vi.fn();
    renderDetail({ runs: [], totalRuns: 0, noOpRunsHidden: 1440, onToggleNoOpRuns });
    expect(screen.getByText('wf.runs.noOp.onlyHidden')).toBeInTheDocument();
    const toggle = screen.getByRole('button', { name: 'wf.runs.noOp.show' });
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(toggle);
    expect(onToggleNoOpRuns).toHaveBeenCalledTimes(1);
  });

  it('has no toggle when no run is hidden and none is shown', () => {
    renderDetail({ runs: [run('a', 1)], totalRuns: 1, noOpRunsHidden: 0, onToggleNoOpRuns: vi.fn() });
    expect(screen.queryByRole('button', { name: 'wf.runs.noOp.show' })).toBeNull();
  });

  it('folds consecutive runs without changes into one row and opens it on click', () => {
    const runs = [
      run('changed-new', 9, 'changed'),
      run('noop-3', 8, 'no_op'),
      run('noop-2', 7, 'no_op'),
      run('noop-1', 6, 'no_op'),
      run('changed-old', 5, 'changed'),
      run('noop-alone', 4, 'no_op'),
    ];
    const { container } = renderDetail({ runs, totalRuns: runs.length, showNoOpRuns: true, onToggleNoOpRuns: vi.fn() });
    expect(screen.getByRole('button', { name: 'wf.runs.noOp.hide' })).toHaveAttribute('aria-pressed', 'true');

    const folds = screen.getAllByTestId('wf-run-noop-fold');
    expect(folds).toHaveLength(1);
    const rows = () => Array.from(container.querySelectorAll('[data-testid="wf-run-item"]'))
      .map(row => row.getAttribute('data-run-id'));
    expect(rows()).toEqual(['changed-new', 'changed-old', 'noop-alone']);
    expect(container.querySelector('[data-run-id="noop-alone"] .wf-run-compact'))
      .toHaveAttribute('data-outcome', 'no_op');

    fireEvent.click(screen.getByRole('button', { name: /wf\.runs\.noOp\.fold/ }));
    expect(rows()).toEqual(['changed-new', 'noop-3', 'noop-2', 'noop-1', 'changed-old', 'noop-alone']);
  });
});
