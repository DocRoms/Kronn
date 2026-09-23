import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun } from '../../../types/generated';

const { getRun } = vi.hoisted(() => ({ getRun: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({ workflows: { getRun } }));
vi.mock('../../../lib/I18nContext', () => ({ useT: () => ({ t: (key: string) => key }) }));
vi.mock('../RunDetail', () => ({ RunDetail: ({ run }: { run: WorkflowRun }) =>
  <div>{run.step_results.map(step => <p key={step.step_name}>{step.output}</p>)}</div> }));
import { LoadedRunDetail } from '../LoadedRunDetail';

const run = (id: string, output = '') => ({
  id, workflow_id: 'wf-1', status: 'Failed',
  step_results: [{ step_name: 'collect', status: 'Failed', step_kind: 'CollectApiData', output }],
} as WorkflowRun);

describe('loading omitted workflow outputs', () => {
  beforeEach(() => getRun.mockReset());
  afterEach(cleanup);

  it('loads the full failed run and forwards its cause to the panel', async () => {
    const cause = 'news (qa-deleted): QuickApi `qa-deleted` does not exist';
    getRun.mockResolvedValue(run('a', cause));
    render(<LoadedRunDetail run={run('a')} onDelete={() => {}} />);
    expect(await screen.findByText(cause)).toBeInTheDocument();
    expect(getRun).toHaveBeenCalledWith('wf-1', 'a');
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('does not fetch again when navigation already supplied full outputs', () => {
    render(<LoadedRunDetail run={run('a', 'complete cause')} onDelete={() => {}} />);
    expect(screen.getByText('complete cause')).toBeInTheDocument();
    expect(getRun).not.toHaveBeenCalled();
  });

  it('ignores a late response after a different run was selected', async () => {
    let finishFirst!: (value: WorkflowRun) => void;
    getRun.mockReturnValueOnce(new Promise(resolve => { finishFirst = resolve; }))
      .mockResolvedValueOnce(run('b', 'cause B'));
    const view = render(<LoadedRunDetail run={run('a')} onDelete={() => {}} />);
    view.rerender(<LoadedRunDetail run={run('b')} onDelete={() => {}} />);
    expect(await screen.findByText('cause B')).toBeInTheDocument();
    await act(async () => { finishFirst(run('a', 'stale cause A')); });
    expect(screen.queryByText('stale cause A')).toBeNull();
    expect(screen.getByText('cause B')).toBeInTheDocument();
  });

  it('shows a failed load and retries only the detail read', async () => {
    getRun.mockRejectedValueOnce(new Error('temporarily unavailable'))
      .mockResolvedValueOnce(run('a', 'recovered cause'));
    render(<LoadedRunDetail run={run('a')} onDelete={() => {}} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('temporarily unavailable');
    fireEvent.click(screen.getByRole('button', { name: 'wf.runDetailRetry' }));
    expect(await screen.findByText('recovered cause')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(getRun).toHaveBeenCalledTimes(2);
  });

  it('refreshes a changed run instead of retaining its earlier output', async () => {
    getRun.mockResolvedValueOnce(run('a', 'earlier cause')).mockResolvedValueOnce(run('a', 'latest cause'));
    const view = render(<LoadedRunDetail run={run('a')} onDelete={() => {}} />);
    await screen.findByText('earlier cause');
    view.rerender(<LoadedRunDetail run={run('a')} onDelete={() => {}} />);
    await waitFor(() => expect(screen.getByText('latest cause')).toBeInTheDocument());
    expect(screen.queryByText('earlier cause')).toBeNull();
  });
});
