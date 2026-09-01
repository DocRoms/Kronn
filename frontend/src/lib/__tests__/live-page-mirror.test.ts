import { describe, it, expect, vi, beforeEach } from 'vitest';

const bindings = vi.fn();
const listRuns = vi.fn();
const getRun = vi.fn();
vi.mock('../api', () => ({
  pages: { bindings: (...a: unknown[]) => bindings(...a) },
  workflows: {
    listRuns: (...a: unknown[]) => listRuns(...a),
    getRun: (...a: unknown[]) => getRun(...a),
  },
}));

const runToPipeline = vi.fn();
vi.mock('../live-page-pipeline', () => ({
  runToPipeline: (...a: unknown[]) => runToPipeline(...a),
}));

import { resolveBindingPipelines } from '../live-page-mirror';

const PHASE = [{ name: 'P', steps: [{ step: 'a' }] }];
const RUN_ACTIVE = { id: 'r1', status: 'WaitingApproval', started_at: '2026-01-02T00:00:00Z' };
const RUN_DONE = { id: 'r0', status: 'Success', started_at: '2026-01-01T00:00:00Z' };

beforeEach(() => {
  bindings.mockReset();
  listRuns.mockReset();
  getRun.mockReset();
  runToPipeline.mockReset();
  runToPipeline.mockReturnValue({ meta: {}, phases: [] });
  getRun.mockResolvedValue({ id: 'r1', step_results: [] });
});

describe('resolveBindingPipelines', () => {
  it('folds an array phase_map into a pipeline keyed by dataset and stays active while the run is non-terminal', async () => {
    bindings.mockResolvedValue([
      { dataset: 'pipeline', workflow_id: 'w1', run_selector: 'latest_active', phase_map: PHASE, meta_map: { ticket: 'trigger.k' } },
    ]);
    listRuns.mockResolvedValue([RUN_ACTIVE, RUN_DONE]);

    const res = await resolveBindingPipelines('page1');

    expect(res.active).toBe(true);
    expect(Object.keys(res.pipelines)).toEqual(['pipeline']);
    // latest_active picks the non-terminal run and folds it with the parsed maps.
    expect(getRun).toHaveBeenCalledWith('w1', 'r1');
    expect(runToPipeline).toHaveBeenCalledWith(expect.anything(), PHASE, { ticket: 'trigger.k' }, { dataset: 'pipeline' });
  });

  it('parses a stringified phase_map/meta_map (MCP double-encoded) before folding', async () => {
    bindings.mockResolvedValue([
      { dataset: 'pipeline', workflow_id: 'w1', run_selector: 'latest', phase_map: JSON.stringify(PHASE), meta_map: JSON.stringify({ ticket: 'trigger.k' }) },
    ]);
    listRuns.mockResolvedValue([RUN_DONE]);

    const res = await resolveBindingPipelines('page1');

    expect(Object.keys(res.pipelines)).toEqual(['pipeline']);
    expect(runToPipeline).toHaveBeenCalledWith(expect.anything(), PHASE, { ticket: 'trigger.k' }, { dataset: 'pipeline' });
    expect(res.active).toBe(false); // only a terminal run exists
  });

  it('skips a trigger-only binding whose phase_map is empty/null', async () => {
    bindings.mockResolvedValue([
      { dataset: 'ticket_trigger', workflow_id: 'w1', run_selector: 'latest', phase_map: null, meta_map: null },
    ]);

    const res = await resolveBindingPipelines('page1');

    expect(res.pipelines).toEqual({});
    expect(listRuns).not.toHaveBeenCalled();
  });
});
