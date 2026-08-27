// Presence freshness from `last_seen` (a participant's last activity
// heartbeat), aligned with the PollBackoffPolicy (stab-3): a long-polling
// agent in cold regime legitimately sleeps up to the policy's max_delay
// between waits — that is "en veille", NOT absent. Grey only beyond
// cap + 2 min margin, i.e. the agent actually missed its own pacing
// contract. The cap is read from the disc meta at runtime; the constant
// is only the fetch-failed fallback (mirrors PollBackoffPolicy::default
// max_delay_seconds).
export type Freshness = 'fresh' | 'idle' | 'away';
export type HonestPresenceState =
  | 'running'
  | 'listening'
  | 'dormant'
  | 'resume_expected'
  | 'stalled'
  | 'quota_exhausted'
  | 'offline';

const FALLBACK_MAX_DELAY_MS = 480_000;
export const AWAY_MARGIN_MS = 2 * 60_000;
export const DEFAULT_AWAY_AFTER_MS = FALLBACK_MAX_DELAY_MS + AWAY_MARGIN_MS;

export function freshnessOf(lastSeen: string | null | undefined, awayAfterMs: number): Freshness {
  if (!lastSeen) return 'away';
  const ageMs = Date.now() - new Date(lastSeen).getTime();
  if (Number.isNaN(ageMs) || ageMs >= awayAfterMs) return 'away';
  return ageMs < 2 * 60_000 ? 'fresh' : 'idle';
}

// Server-derived `activity` values that PROVE the session is alive right now:
// the backend applies expiry at read time, so a non-null activity is never
// stale. `listening`/`reading` = actively engaged; `waiting` = dormant during
// a pacing pause but committed to poll again (presence-gap fix). Any of them
// outranks a `last_seen` that legitimately aged past the freshness window
// during a long cold-regime sleep — otherwise a present-but-dormant agent
// flips to "away" and the human relaunches it needlessly.
const LIVE_ACTIVITIES = new Set(['listening', 'reading', 'waiting']);

// Effective presence: an unexpired activity means present (`fresh`),
// whatever `last_seen`'s age; only with no activity do we fall back to the
// heartbeat-based freshness. The textual activity label carries the
// listening-vs-waiting nuance separately.
export function presenceFromActivity(
  activity: string | null | undefined,
  lastSeen: string | null | undefined,
  awayAfterMs: number,
): Freshness {
  if (activity && LIVE_ACTIVITIES.has(activity)) return 'fresh';
  return freshnessOf(lastSeen, awayAfterMs);
}

const HONEST_PRESENCE_STATES = new Set<HonestPresenceState>([
  'running',
  'listening',
  'dormant',
  'resume_expected',
  'stalled',
  'quota_exhausted',
  'offline',
]);

/** Prefer the server-derived 0.9.2 state, with a conservative fallback for an
 * older backend. A heartbeat alone never claims that a read channel is open:
 * legacy fresh/idle sessions render dormant until an activity proves they are
 * listening. */
export function honestPresenceState(
  serverState: string | null | undefined,
  status: string,
  activity: string | null | undefined,
  lastSeen: string | null | undefined,
  awayAfterMs: number,
): HonestPresenceState {
  if (status === 'paused' || status === 'left') return 'offline';
  if (serverState && HONEST_PRESENCE_STATES.has(serverState as HonestPresenceState)) {
    return serverState as HonestPresenceState;
  }
  if (activity === 'listening' || activity === 'reading') return 'listening';
  if (activity === 'waiting') return 'dormant';
  return freshnessOf(lastSeen, awayAfterMs) === 'away' ? 'offline' : 'dormant';
}

export function freshnessForPresence(state: HonestPresenceState): Freshness {
  if (state === 'running' || state === 'listening') return 'fresh';
  return state === 'dormant' || state === 'resume_expected' ? 'idle' : 'away';
}

export function secondsUntil(nextPollAt: string | null | undefined): number | null {
  if (!nextPollAt) return null;
  const timestamp = new Date(nextPollAt).getTime();
  if (Number.isNaN(timestamp)) return null;
  return Math.max(0, Math.ceil((timestamp - Date.now()) / 1000));
}
