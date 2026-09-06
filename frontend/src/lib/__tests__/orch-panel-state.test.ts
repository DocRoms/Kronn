import { beforeEach, describe, expect, it } from 'vitest';
import {
  readPlanOrchestrationState,
  writePlanOrchestrationState,
} from '../orch-panel-state';

describe('plan orchestration panel persistence', () => {
  beforeEach(() => sessionStorage.clear());

  it.each(['focus', 'in_progress', 'all'] as const)('restores the selected task and %s view after a reload', viewMode => {
    writePlanOrchestrationState('disc-1', {
      selectedTaskId: 'task-323',
      viewMode,
    });
    expect(readPlanOrchestrationState('disc-1')).toEqual({
      selectedTaskId: 'task-323',
      viewMode,
    });
  });

  it('isolates rooms and survives malformed browser state', () => {
    sessionStorage.setItem('kronn:plan-orchestration:broken', '{nope');
    expect(readPlanOrchestrationState('broken')).toEqual({
      selectedTaskId: null,
      viewMode: 'focus',
    });
    expect(readPlanOrchestrationState('disc-2').selectedTaskId).toBeNull();
  });

  it('falls back to Focus for an unknown view', () => {
    sessionStorage.setItem('kronn:plan-orchestration:disc-1', JSON.stringify({ viewMode: 'unknown' }));
    expect(readPlanOrchestrationState('disc-1').viewMode).toBe('focus');
  });
});
