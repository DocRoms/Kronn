/** The menu badges count from the bell's own read. */
import { describe, expect, it } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { publishActivity, runningWorkflowRuns, useSharedActivity } from '../activityShared';
import type { ActivityOverview, ActivityRow } from '../../types/generated';

const row = (id: string, type: string, compartment: string): ActivityRow => ({
  id, type, compartment, state: 'running', source: 'test', title: id,
  started_at: '2026-11-10T11:56:00Z', stalled: false, actions: [],
} as ActivityRow);
const overview = (rows: ActivityRow[]): ActivityOverview => ({
  rows, stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [],
});

describe('runningWorkflowRuns', () => {
  it('counts the workflow runs in progress of the overview, nothing else', () => {
    const rows = [
      row('w1', 'workflow_run', 'running'), row('w2', 'workflow_run', 'running'),
      row('w3', 'workflow_run', 'needs_decision'), row('a1', 'agent_turn', 'running'), row('k1', 'audit', 'running'),
    ];
    // Four workflows may say "Running" (a child run among them): the overview has the last word.
    expect(runningWorkflowRuns(overview(rows), 4)).toBe(2);
    expect(runningWorkflowRuns(overview([]), 4)).toBe(0);
  });

  it('stands on the fallback until an overview is read', () => {
    expect(runningWorkflowRuns(null, 4)).toBe(4);
  });
});

describe('useSharedActivity', () => {
  it('gives every reader what the bell published last', () => {
    publishActivity(null);
    const { result } = renderHook(() => useSharedActivity());
    expect(result.current).toBeNull();
    const first = overview([row('w1', 'workflow_run', 'running')]);
    act(() => publishActivity(first));
    expect(result.current).toBe(first);
  });
});
