import { describe, expect, it } from 'vitest';
import {
  applyRunFrame,
  applyRunSnapshot,
  idleRemainingMs,
  pruneStoppedRuns,
  runElapsedMs,
  runForReply,
  STOPPED_RUN_TTL_MS,
  type AgentRunProgressFrame,
  type LiveRuns,
} from '../agentRunProgress';

const frame = (
  overrides: Partial<Omit<AgentRunProgressFrame, 'progress'>> & { progress?: Partial<AgentRunProgressFrame['progress']> } = {},
): AgentRunProgressFrame => ({
  type: 'agent_run_progress',
  discussion_id: 'd1',
  dispatch_id: 'job-1',
  trigger_message_id: 'u1',
  agent_type: 'OpenCode',
  run_id: 'run-1',
  started_at: '2026-10-09T09:00:00Z',
  seq: 1,
  ...overrides,
  progress: {
    phase: 'waiting_model',
    phase_ms: 1_000,
    elapsed_ms: 2_000,
    timeline: [],
    mcp_servers: null,
    activity: [],
    tool_calls: 0,
    silent_ms: 1_000,
    idle_limit_ms: 60_000,
    stopped: null,
    ...overrides.progress,
  },
});

const reply = { id: 'job-1', agent: 'OpenCode' as const, triggerMessageId: 'u1' };

describe('applyRunFrame', () => {
  it('ignores an older frame and anything after the stop', () => {
    let runs: LiveRuns = applyRunFrame({}, frame({ seq: 2 }), 1_000);
    expect(applyRunFrame(runs, frame({ seq: 1, progress: { phase: 'initializing' } }), 1_100)).toBe(runs);
    runs = applyRunFrame(runs, frame({ seq: 3, progress: { stopped: 'idle' } }), 1_200);
    const after = applyRunFrame(runs, frame({ seq: 4, progress: { phase: 'responding' } }), 1_300);
    expect(after).toBe(runs);
    expect(after.d1['run-1'].progress.stopped).toBe('idle');
  });

  it('hides a stopped run once its announcement had time to be read', () => {
    const runs = applyRunFrame({}, frame({ progress: { stopped: 'finished' } }), 0);
    expect(pruneStoppedRuns(runs, STOPPED_RUN_TTL_MS - 1)).toBe(runs);
    const pruned = pruneStoppedRuns(runs, STOPPED_RUN_TTL_MS + 1);
    expect(runForReply(pruned, 'd1', reply, { claimed: new Set(['job-1']), durable: new Set(['job-1']) })).toBeNull();
  });
});

describe('runForReply', () => {
  it('matches a dispatch exactly and never another agent or a claimed dispatch', () => {
    const ids = { claimed: new Set(['job-1']), durable: new Set(['job-1']) };
    let runs = applyRunFrame({}, frame({ run_id: 'other', dispatch_id: 'job-2' }), 0);
    expect(runForReply(runs, 'd1', reply, ids)).toBeNull();
    runs = applyRunFrame(runs, frame({ run_id: 'claude', dispatch_id: null, agent_type: 'ClaudeCode' }), 0);
    expect(runForReply(runs, 'd1', reply, ids)).toBeNull();
    runs = applyRunFrame(runs, frame(), 0);
    expect(runForReply(runs, 'd1', reply, ids)?.runId).toBe('run-1');
    expect(runForReply(runs, 'd2', reply, ids)).toBeNull();
  });

  it('lets an optimistic bubble show its own unclaimed run, and a retry win over its stopped attempt', () => {
    let runs = applyRunFrame({}, frame({ run_id: 'first', progress: { stopped: 'failed' } }), 0);
    runs = applyRunFrame(runs, frame({ run_id: 'retry', started_at: '2026-10-09T09:00:05Z' }), 10);
    const optimistic = { ...reply, id: 'optimistic:u1:OpenCode' };
    expect(runForReply(runs, 'd1', optimistic, { claimed: new Set([optimistic.id]), durable: new Set() })?.runId)
      .toBe('retry');
    expect(runForReply(runs, 'd1', reply, { claimed: new Set(['job-1']), durable: new Set(['job-1']) })?.runId)
      .toBe('retry');
    // Once its dispatch has a bubble, an optimistic one no longer takes the run.
    expect(runForReply(runs, 'd1', optimistic, { claimed: new Set(['job-1', optimistic.id]), durable: new Set(['job-1']) }))
      .toBeNull();
  });
});

describe('attempts and turns', () => {
  const ids = { claimed: new Set(['job-1']), durable: new Set(['job-1']) };

  it('never brings an older attempt back when its stop was missed', () => {
    // Attempt A runs; its stop never arrives. Attempt B of the same dispatch
    // starts, then stops for inactivity: the bubble shows B's stop, not A.
    let runs = applyRunFrame({}, frame({ run_id: 'A', seq: 3 }), 0);
    runs = applyRunFrame(runs, frame({ run_id: 'B', started_at: '2026-10-09T09:01:00Z', seq: 1 }), 100);
    runs = applyRunFrame(runs, frame({
      run_id: 'B', started_at: '2026-10-09T09:01:00Z', seq: 2,
      progress: { stopped: 'idle', idle_limit_ms: null },
    }), 200);
    const shown = runForReply(runs, 'd1', reply, ids);
    expect(shown?.runId).toBe('B');
    expect(shown?.progress.stopped).toBe('idle');
    // A late frame of A changes nothing.
    runs = applyRunFrame(runs, frame({ run_id: 'A', seq: 4, progress: { phase: 'responding' } }), 300);
    expect(runForReply(runs, 'd1', reply, ids)?.runId).toBe('B');
  });

  it('keeps a newer live frame over a late snapshot of the same run', () => {
    let runs = applyRunFrame({}, frame({ seq: 5, progress: { phase: 'tool' } }), 1_000);
    // The snapshot was read before frame 5 left; it arrives after.
    runs = applyRunFrame(runs, frame({ seq: 4, progress: { phase: 'waiting_model' } }), 1_100);
    expect(runForReply(runs, 'd1', reply, ids)?.progress.phase).toBe('tool');
    expect(runForReply(runs, 'd1', reply, ids)?.seq).toBe(5);
    // Nor does a snapshot of an older attempt displace the current one.
    runs = applyRunFrame(runs, frame({ run_id: 'older', started_at: '2026-10-09T08:00:00Z', seq: 9 }), 1_200);
    expect(runForReply(runs, 'd1', reply, ids)?.runId).toBe('run-1');
  });

  it('never attaches a run without a proven turn to a new turn', () => {
    const optimistic = { id: 'optimistic:u2:OpenCode', agent: 'OpenCode' as const, triggerMessageId: 'u2' };
    const optimisticIds = { claimed: new Set([optimistic.id]), durable: new Set<string>() };
    let runs = applyRunFrame({}, frame({
      run_id: 'previous', dispatch_id: null, trigger_message_id: null,
      progress: { stopped: 'failed' },
    }), 0);
    runs = applyRunFrame(runs, frame({ run_id: 'u1-run', dispatch_id: null }), 0);
    expect(runForReply(runs, 'd1', optimistic, optimisticIds)).toBeNull();
  });
});

describe('durations', () => {
  it('grow from the frame, restart only with a new frame, and freeze at the stop', () => {
    const runs = applyRunFrame({}, frame(), 10_000);
    const run = runs.d1['run-1'];
    // 59 s left at receipt; 5 s of local ticking later, 54 s.
    expect(idleRemainingMs(run, 10_000)).toBe(59_000);
    expect(idleRemainingMs(run, 15_000)).toBe(54_000);
    expect(runElapsedMs(run, 15_000)).toBe(7_000);
    expect(idleRemainingMs(run, 200_000)).toBe(0);
    const stopped = applyRunFrame(runs, frame({ seq: 2, progress: { stopped: 'idle', elapsed_ms: 61_000 } }), 70_000).d1['run-1'];
    expect(runElapsedMs(stopped, 500_000)).toBe(61_000);
    expect(idleRemainingMs(stopped, 500_000)).toBeNull();
  });
});

describe('Codex review — retiring old attempts', () => {
  it('never revives A when the stopped current attempt B expires from the UI cache', () => {
    let runs = applyRunFrame({}, frame({ run_id: 'A', seq: 3 }), 0);
    runs = applyRunFrame(runs, frame({
      run_id: 'B', started_at: '2026-10-09T09:01:00Z', seq: 1,
    }), 100);
    runs = applyRunFrame(runs, frame({
      run_id: 'B', started_at: '2026-10-09T09:01:00Z', seq: 2,
      progress: { stopped: 'idle', idle_limit_ms: null },
    }), 200);
    const ids = { claimed: new Set(['job-1']), durable: new Set(['job-1']) };
    expect(runForReply(runs, 'd1', reply, ids)?.runId).toBe('B');
    runs = pruneStoppedRuns(runs, 200 + STOPPED_RUN_TTL_MS + 1);
    // Either B's terminal state or no run is fine, but A can never become current again.
    expect(runForReply(runs, 'd1', reply, ids)?.runId).not.toBe('A');
    // Nor through a late frame of A.
    runs = applyRunFrame(runs, frame({ run_id: 'A', seq: 9 }), 70_000);
    expect(runForReply(runs, 'd1', reply, ids)?.runId).not.toBe('A');
  });

  it('retires a run that ended unseen when a snapshot no longer lists it', () => {
    const ids = { claimed: new Set(['job-1']), durable: new Set(['job-1']) };
    let runs = applyRunFrame({}, frame({ run_id: 'gone' }), 0);
    // A run first heard of after the snapshot was requested is kept.
    runs = applyRunFrame(runs, frame({ run_id: 'other', dispatch_id: 'job-2', started_at: '2026-10-09T09:02:00Z' }), 500);
    runs = applyRunSnapshot(runs, 'd1', [], 100, 600);
    expect(runForReply(runs, 'd1', reply, ids)).toBeNull();
    const second = { id: 'job-2', agent: 'OpenCode' as const, triggerMessageId: 'u1' };
    expect(runForReply(runs, 'd1', second, { claimed: new Set(['job-2']), durable: new Set(['job-2']) })?.runId)
      .toBe('other');
  });
});
