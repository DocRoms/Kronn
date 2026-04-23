// Unit tests for ActiveRunsPopover — the nav fly-out listing in-flight
// workflow runs with inline Stop buttons.
//
// Scope: render active runs only, stop triggers cancelRun with correct ids,
// disabled state after cancel, Esc closes, footer navigates, empty state.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowSummary } from '../../../types/generated';

const { cancelRunMock } = vi.hoisted(() => ({
  cancelRunMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  workflows: { cancelRun: cancelRunMock as never },
}));

// i18n: echo the key (+ first arg) so assertions can match on stable strings.
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args[0]}` : key,
  }),
}));

import { ActiveRunsPopover } from '../ActiveRunsPopover';

const runningWf = (over: Partial<WorkflowSummary> & { runId?: string }): WorkflowSummary => ({
  id: over.id ?? 'wf-1',
  name: over.name ?? 'WorkflowAlpha',
  project_id: 'proj-1',
  project_name: over.project_name ?? 'ProjectA',
  trigger_type: 'manual',
  step_count: 2,
  enabled: true,
  last_run: {
    id: over.runId ?? 'run-1',
    status: 'Running',
    started_at: new Date(Date.now() - 30_000).toISOString(),
    finished_at: null,
    tokens_used: 0,
  },
  created_at: new Date().toISOString(),
});

const idleWf = (id: string): WorkflowSummary => ({
  id,
  name: `IdleWf-${id}`,
  project_id: null,
  project_name: null,
  trigger_type: 'manual',
  step_count: 1,
  enabled: true,
  last_run: {
    id: `run-${id}`,
    status: 'Success',
    started_at: new Date().toISOString(),
    finished_at: new Date().toISOString(),
    tokens_used: 100,
  },
  created_at: new Date().toISOString(),
});

describe('ActiveRunsPopover', () => {
  beforeEach(() => {
    cancelRunMock.mockReset();
    cancelRunMock.mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 });
  });

  it('lists only workflows whose last_run is Running or Pending', () => {
    const wfs = [
      runningWf({ id: 'a', name: 'Alpha', runId: 'run-a' }),
      idleWf('b'),
      runningWf({ id: 'c', name: 'Charlie', runId: 'run-c' }),
    ];
    render(
      <ActiveRunsPopover
        workflows={wfs}
        onClose={() => {}}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={() => {}}
      />,
    );
    expect(screen.getByText('Alpha')).toBeInTheDocument();
    expect(screen.getByText('Charlie')).toBeInTheDocument();
    expect(screen.queryByText(/IdleWf/)).not.toBeInTheDocument();
  });

  it('shows empty-state copy when no runs are active', () => {
    render(
      <ActiveRunsPopover
        workflows={[idleWf('x'), idleWf('y')]}
        onClose={() => {}}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={() => {}}
      />,
    );
    expect(screen.getByText('wf.activeRunsEmpty')).toBeInTheDocument();
  });

  it('clicking Stop calls cancelRun with the matching workflow and run ids', async () => {
    render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-42', runId: 'run-99' })]}
        onClose={() => {}}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={() => {}}
      />,
    );
    const stopBtn = screen.getByRole('button', { name: /wf.cancelRun/ });
    fireEvent.click(stopBtn);
    await waitFor(() => expect(cancelRunMock).toHaveBeenCalledTimes(1));
    expect(cancelRunMock).toHaveBeenCalledWith('wf-42', 'run-99');
  });

  it('disables the Stop button and swaps label after click, preventing double-trigger', async () => {
    cancelRunMock.mockImplementation(() => new Promise(() => { /* pending forever */ }));
    render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-1', runId: 'run-1' })]}
        onClose={() => {}}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={() => {}}
      />,
    );
    const stopBtn = screen.getByRole('button', { name: /wf.cancelRun/ });
    fireEvent.click(stopBtn);
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /wf.cancelling/ })).toBeDisabled();
    });
    // Re-click while disabled must not fire a second request.
    fireEvent.click(screen.getByRole('button', { name: /wf.cancelling/ }));
    expect(cancelRunMock).toHaveBeenCalledTimes(1);
  });

  it('stop click does not bubble up and trigger the row body navigation', async () => {
    const onNavigate = vi.fn();
    render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-1', runId: 'run-1' })]}
        onClose={() => {}}
        onNavigateToWorkflow={onNavigate}
        onViewAllWorkflows={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.cancelRun/ }));
    await waitFor(() => expect(cancelRunMock).toHaveBeenCalled());
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it('clicking the row body navigates to the workflow', () => {
    const onNavigate = vi.fn();
    render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-55', runId: 'run-55' })]}
        onClose={() => {}}
        onNavigateToWorkflow={onNavigate}
        onViewAllWorkflows={() => {}}
      />,
    );
    // The row body is the first button inside the item — distinguishable
    // because it contains the workflow name.
    const rowBtn = screen.getByRole('button', { name: /WorkflowAlpha/ });
    fireEvent.click(rowBtn);
    expect(onNavigate).toHaveBeenCalledWith('wf-55');
  });

  it('Escape key closes the popover', () => {
    const onClose = vi.fn();
    render(
      <ActiveRunsPopover
        workflows={[runningWf({})]}
        onClose={onClose}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={() => {}}
      />,
    );
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('footer "view all workflows" button calls onViewAllWorkflows', () => {
    const onViewAll = vi.fn();
    render(
      <ActiveRunsPopover
        workflows={[]}
        onClose={() => {}}
        onNavigateToWorkflow={() => {}}
        onViewAllWorkflows={onViewAll}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.viewAllWorkflows/ }));
    expect(onViewAll).toHaveBeenCalledTimes(1);
  });

  it('outside mousedown closes the popover', async () => {
    const onClose = vi.fn();
    render(
      <>
        <button type="button" data-testid="outside">outside</button>
        <ActiveRunsPopover
          workflows={[runningWf({})]}
          onClose={onClose}
          onNavigateToWorkflow={() => {}}
          onViewAllWorkflows={() => {}}
        />
      </>,
    );
    // The listener attaches on next tick to avoid closing on the opening click.
    await act(async () => { await new Promise(r => setTimeout(r, 10)); });
    fireEvent.mouseDown(screen.getByTestId('outside'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
