/**
 * KT-1108 — the live progress of the agent runs of each discussion, from the
 * `agent_run_progress` WebSocket frames and the snapshot read on open and on
 * reconnect. One entry per run; a frame never revives a run that already
 * stopped, an older frame never overwrites a newer one, and a reply shows its
 * latest attempt only.
 */
import { useSyncExternalStore } from 'react';
import type { AgentRunProgress, AgentType, WsMessage } from '../types/generated';

export type AgentRunProgressFrame = Extract<WsMessage, { type: 'agent_run_progress' }>;

export interface LiveRun {
  runId: string;
  dispatchId: string | null;
  triggerMessageId: string | null;
  agent: AgentType;
  seq: number;
  /** Server time the launch started: the latest attempt of a reply wins. */
  startedAt: number;
  progress: AgentRunProgress;
  /** Local time the frame arrived: durations grow from it, never from a local guess. */
  receivedAt: number;
  firstSeenAt: number;
  /** Kept only so no older attempt of its reply is ever shown again. */
  retired?: boolean;
}

/** discussion id → run id → run. */
export type LiveRuns = Record<string, Record<string, LiveRun>>;

/** How long a stopped run stays, so its bubble can say why it stopped. */
export const STOPPED_RUN_TTL_MS = 60_000;
const RUNS_PER_DISCUSSION = 32;

/** The reply a run is an attempt of: its dispatch, else its agent's turn. */
function attemptKey(run: Pick<LiveRun, 'dispatchId' | 'agent' | 'triggerMessageId'>): string {
  return run.dispatchId ?? `${run.agent}|${run.triggerMessageId ?? ''}`;
}

/**
 * The runs once `frame` is applied; `runs` itself when the frame is stale: an
 * older frame of a run, any frame after its stop, or a frame of an attempt
 * older than one already seen for the same reply. A newer attempt drops the
 * older ones for good.
 */
export function applyRunFrame(runs: LiveRuns, frame: AgentRunProgressFrame, now: number): LiveRuns {
  const discussion = runs[frame.discussion_id] ?? {};
  const known = discussion[frame.run_id];
  if (known && (known.retired || known.progress.stopped || frame.seq <= known.seq)) return runs;
  const incoming: LiveRun = {
    runId: frame.run_id,
    dispatchId: frame.dispatch_id,
    triggerMessageId: frame.trigger_message_id,
    agent: frame.agent_type,
    seq: frame.seq,
    startedAt: Date.parse(frame.started_at) || 0,
    progress: frame.progress,
    receivedAt: now,
    firstSeenAt: known?.firstSeenAt ?? now,
  };
  const key = attemptKey(incoming);
  const siblings = Object.values(discussion).filter(run => run.runId !== frame.run_id && attemptKey(run) === key);
  if (siblings.some(run => run.startedAt > incoming.startedAt)) return runs;
  const next: Record<string, LiveRun> = { ...discussion, [frame.run_id]: incoming };
  siblings
    .filter(run => run.startedAt < incoming.startedAt)
    .forEach(run => { delete next[run.runId]; });
  // Bounded: the oldest runs go first.
  const ids = Object.keys(next);
  if (ids.length > RUNS_PER_DISCUSSION) {
    ids
      .sort((a, b) => next[a].firstSeenAt - next[b].firstSeenAt)
      .slice(0, ids.length - RUNS_PER_DISCUSSION)
      .forEach(id => { delete next[id]; });
  }
  return { ...runs, [frame.discussion_id]: next };
}

/**
 * Retires the runs that stopped more than the TTL ago: hidden, but kept as
 * the latest attempt of their reply so no older attempt can come back.
 */
export function pruneStoppedRuns(runs: LiveRuns, now: number): LiveRuns {
  let changed = false;
  const next: LiveRuns = {};
  for (const [discussionId, discussion] of Object.entries(runs)) {
    const kept: Record<string, LiveRun> = {};
    for (const [runId, run] of Object.entries(discussion)) {
      if (!run.retired && run.progress.stopped && now - run.receivedAt > STOPPED_RUN_TTL_MS) {
        kept[runId] = { ...run, retired: true };
        changed = true;
      } else {
        kept[runId] = run;
      }
    }
    next[discussionId] = kept;
  }
  return changed ? next : runs;
}

/**
 * Applies a snapshot of `discussionId` read at `requestedAt`: a run still
 * live here but absent from it, and not heard of since, ended unseen.
 */
export function applyRunSnapshot(
  runs: LiveRuns,
  discussionId: string,
  frames: AgentRunProgressFrame[],
  requestedAt: number,
  now: number,
): LiveRuns {
  let next = frames.reduce((acc, frame) => applyRunFrame(acc, frame, now), runs);
  const listed = new Set(frames.map(frame => frame.run_id));
  const discussion = next[discussionId];
  if (!discussion) return next;
  const ended = Object.values(discussion).filter(run => (
    !run.retired && !run.progress.stopped && !listed.has(run.runId) && run.receivedAt < requestedAt
  ));
  if (ended.length === 0) return next;
  const updated = { ...discussion };
  ended.forEach(run => { updated[run.runId] = { ...run, retired: true }; });
  next = { ...next, [discussionId]: updated };
  return next;
}

/** The reply bubbles on screen: all of them, and those backed by a durable dispatch. */
export interface ReplyIds {
  claimed: ReadonlySet<string>;
  durable: ReadonlySet<string>;
}

/**
 * The run a reply bubble shows: the latest attempt that positively belongs to
 * it. A bubble backed by a dispatch shows that dispatch's runs only. Any other
 * bubble (an optimistic reply, a legacy stream) needs a run of its agent for
 * its very turn that no other bubble claims. Among those, the attempt started
 * last is the one shown, even when an older one's stop was never received.
 */
export function runForReply(
  runs: LiveRuns,
  discussionId: string,
  reply: { id: string; agent: AgentType; triggerMessageId: string },
  replyIds: ReplyIds,
): LiveRun | null {
  const discussion = runs[discussionId];
  if (!discussion) return null;
  const all = Object.values(discussion).filter(run => !run.retired);
  const candidates = replyIds.durable.has(reply.id)
    ? all.filter(run => run.dispatchId === reply.id)
    : all.filter(run => (
      run.agent === reply.agent
      && run.triggerMessageId === reply.triggerMessageId
      && (run.dispatchId === null || !replyIds.claimed.has(run.dispatchId))
    ));
  let latest: LiveRun | null = null;
  for (const run of candidates) {
    if (!latest || run.startedAt > latest.startedAt
      || (run.startedAt === latest.startedAt && run.firstSeenAt > latest.firstSeenAt)) {
      latest = run;
    }
  }
  return latest;
}

/** Elapsed run time at `now`, frozen once the run stopped. */
export function runElapsedMs(run: LiveRun, now: number): number {
  if (run.progress.stopped) return run.progress.elapsed_ms;
  return run.progress.elapsed_ms + Math.max(0, now - run.receivedAt);
}

/** Time spent in the current phase at `now`. */
export function phaseElapsedMs(run: LiveRun, now: number): number {
  if (run.progress.stopped) return run.progress.phase_ms;
  return run.progress.phase_ms + Math.max(0, now - run.receivedAt);
}

/**
 * Inactivity time left before Kronn stops the agent, or `null` when no delay
 * applies. It only restarts when a frame reports new activity.
 */
export function idleRemainingMs(run: LiveRun, now: number): number | null {
  const limit = run.progress.idle_limit_ms;
  if (run.progress.stopped || limit === null) return null;
  const silent = run.progress.silent_ms + Math.max(0, now - run.receivedAt);
  return Math.max(0, limit - silent);
}

/** Silence in progress at `now`. */
export function silentMs(run: LiveRun, now: number): number {
  if (run.progress.stopped) return run.progress.silent_ms;
  return run.progress.silent_ms + Math.max(0, now - run.receivedAt);
}

/** A startup step's length: tenths of a second while it is short. */
export function formatStepDuration(ms: number): string {
  return ms < 10_000 ? `${(Math.max(0, ms) / 1000).toFixed(1)} s` : formatDuration(ms);
}

/** `1 min 05 s`, `42 s`. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return `${total} s`;
  const minutes = Math.floor(total / 60);
  const seconds = String(total % 60).padStart(2, '0');
  if (minutes < 60) return `${minutes} min ${seconds} s`;
  return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, '0')} min`;
}

// ─── The page-wide store: frames land here without re-rendering the page ────

let store: LiveRuns = {};
const listeners = new Set<() => void>();

/** Applies one WebSocket frame; only the bubbles reading it re-render. */
export function receiveRunFrame(frame: AgentRunProgressFrame, now = Date.now()): void {
  const next = pruneStoppedRuns(applyRunFrame(store, frame, now), now);
  if (next === store) return;
  store = next;
  listeners.forEach(listener => listener());
}

/** Applies a snapshot of `discussionId` read at `requestedAt` (see `applyRunSnapshot`). */
export function receiveRunSnapshot(
  discussionId: string,
  frames: AgentRunProgressFrame[],
  requestedAt: number,
  now = Date.now(),
): void {
  const next = applyRunSnapshot(store, discussionId, frames, requestedAt, now);
  if (next === store) return;
  store = next;
  listeners.forEach(listener => listener());
}

/** Empties the store; for tests. */
export function resetLiveRuns(): void {
  store = {};
  listeners.forEach(listener => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

const readStore = () => store;

/** The live run a reply bubble shows, or `null` before its first frame. */
export function useLiveRun(
  discussionId: string | undefined,
  reply: { id: string; agent: AgentType; triggerMessageId: string },
  replyIds: ReplyIds,
): LiveRun | null {
  const runs = useSyncExternalStore(subscribe, readStore, readStore);
  return discussionId ? runForReply(runs, discussionId, reply, replyIds) : null;
}
