-- 0.8.7 (2026-06-08) — one-time cleanup of abandoned discussion sessions.
--
-- The double-responder guard (`count_live_participants`) became
-- presence-sticky: ANY `status='active'` session suppresses Kronn's
-- auto-response, with no per-message staleness window (the previous 300s
-- window wrongly judged idle turn-based CLI peers as dead → double-reply).
--
-- The flip side: sessions that were abandoned long ago (an agent that
-- exited without `disc_leave`) still show `status='active'` and would now
-- pin the gate forever, silently suppressing Kronn on old discussions.
-- This backfill retires every session that hasn't heartbeated (or, for
-- pre-heartbeat rows, hasn't been touched since `joined_at`) in over 24h —
-- the same threshold `reap_abandoned_sessions` enforces at boot going
-- forward. Genuinely-present peers (last_seen within 24h) are untouched.
UPDATE discussion_sessions
   SET status = 'left',
       left_at = COALESCE(left_at, datetime('now'))
 WHERE status = 'active'
   AND COALESCE(last_seen, joined_at) < datetime('now', '-1 day');
