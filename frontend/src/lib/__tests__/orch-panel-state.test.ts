import { beforeEach, describe, expect, it } from 'vitest';
import {
  readPlanOrchestrationState,
  writePlanOrchestrationState,
} from '../orch-panel-state';

describe('plan orchestration panel persistence', () => {
  beforeEach(() => sessionStorage.clear());

  it('restores the selected task and view mode after a reload', () => {
    writePlanOrchestrationState('disc-1', {
      selectedTaskId: 'task-323',
      viewMode: 'all',
    });
    expect(readPlanOrchestrationState('disc-1')).toEqual({
      selectedTaskId: 'task-323',
      viewMode: 'all',
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
});
