import { describe, it, expect } from 'vitest';
import { liveStepWaitingKey } from '../WorkflowDetail';

// 0.8.8 — the live-run "step in progress" placeholder must not claim an agent
// is starting on a deterministic step (the reported bug: an ApiCall `fetch`
// step showed "L'agent démarre…"). Only Agent steps stream chunks.

const key = (type?: string) =>
  liveStepWaitingKey({ step_type: type ? ({ type } as never) : undefined });

describe('liveStepWaitingKey', () => {
  it('uses the agent-streaming copy for Agent steps', () => {
    expect(key('Agent')).toBe('wf.live.stepStreamingWaiting');
  });

  it('falls back to the agent copy when step_type is missing (back-compat)', () => {
    expect(key(undefined)).toBe('wf.live.stepStreamingWaiting');
  });

  it.each(['ApiCall', 'Exec', 'Gate', 'Notify', 'JsonData', 'BatchQuickPrompt', 'BatchApiCall', 'SubWorkflow'])(
    'uses the neutral no-stream copy for the deterministic %s step',
    (type) => {
      expect(key(type)).toBe('wf.live.stepRunningNoStream');
    },
  );
});
