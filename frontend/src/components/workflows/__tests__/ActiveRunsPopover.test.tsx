// Unit tests for ActiveRunsPopover — the nav fly-out listing in-flight
// workflow runs with inline Stop buttons.
//
// Scope: render active runs only, stop triggers cancelRun with correct ids,
// disabled state after cancel, Esc closes, footer navigates, empty state.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render as baseRender, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router';
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
  misconfigured_step_count: 0,
  enabled: true,
  pinned: false,
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
  misconfigured_step_count: 0,
  enabled: true,
  pinned: false,
  last_run: {
    id: `run-${id}`,
    status: 'Success',
    started_at: new Date().toISOString(),
    finished_at: new Date().toISOString(),
    tokens_used: 100,
  },
  created_at: new Date().toISOString(),
});

function render(ui: React.ReactElement, initialPath = '/') {
  const router = createMemoryRouter(
    [{ path: '*', element: ui }],
    { initialEntries: [initialPath] },
  );
  const result = baseRender(<RouterProvider router={router} />);
  return { router, ...result };
}

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
      />,
    );
    expect(screen.getByText('wf.activeRunsEmpty')).toBeInTheDocument();
  });

  it('clicking Stop calls cancelRun with the matching workflow and run ids', async () => {
    render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-42', runId: 'run-99' })]}
        onClose={() => {}}
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
    const { router } = render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-1', runId: 'run-1' })]}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.cancelRun/ }));
    await waitFor(() => expect(cancelRunMock).toHaveBeenCalled());
    expect(router.state.location.pathname).toBe('/');
  });

  it('clicking the row body navigates to the workflow', () => {
    const { router } = render(
      <ActiveRunsPopover
        workflows={[runningWf({ id: 'wf-55', runId: 'run-55' })]}
        onClose={() => {}}
      />,
    );
    // The row body is the first button inside the item — distinguishable
    // because it contains the workflow name.
    const rowBtn = screen.getByRole('button', { name: /WorkflowAlpha/ });
    fireEvent.click(rowBtn);
    expect(router.state.location.pathname).toBe('/workflows/wf-55');
  });

  it('Escape key closes the popover', () => {
    const onClose = vi.fn();
    render(
      <ActiveRunsPopover
        workflows={[runningWf({})]}
        onClose={onClose}
      />,
    );
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('footer "view all workflows" button navigates to workflows', () => {
    const { router } = render(
      <ActiveRunsPopover
        workflows={[]}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /wf.viewAllWorkflows/ }));
    expect(router.state.location.pathname).toBe('/workflows');
  });

  it('outside mousedown closes the popover', async () => {
    const onClose = vi.fn();
    render(
      <>
        <button type="button" data-testid="outside">outside</button>
        <ActiveRunsPopover
          workflows={[runningWf({})]}
          onClose={onClose}
        />
      </>,
    );
    // The listener attaches on next tick to avoid closing on the opening click.
    await act(async () => { await new Promise(r => setTimeout(r, 10)); });
    fireEvent.mouseDown(screen.getByTestId('outside'));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
