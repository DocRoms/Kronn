-- 0.9.2-G — Honest presence: persist the facts the UI needs to tell
-- `listening` / `dormant` / `offline` apart instead of blunt fresh/idle/away.
--
-- `next_poll_at`  — when the session's NEXT long-poll is contractually due
--   (server-computed from PollBackoffPolicy at each wait). Lets the UI show
--   "revient dans Xs" for a dormant peer, and lets the server call a peer that
--   blew past its own pacing deadline `offline` instead of a lingering TTL
--   "connecté" — the exact dishonesty this slice removes.
-- `last_write_at` — timestamp of the last SUCCESSFUL append by this session.
-- `write_state`   — tri-state write-liveness. Never inferred `false` from mere
--   silence: `unknown` until a write is observed, `ok` after a successful
--   append, `failed` only when the bridge reports an authentic write error.
--   Decoupled from read-liveness (a peer can still be reading while its append
--   path is broken — a case seen live during 0.9.2 dogfooding).
ALTER TABLE discussion_sessions ADD COLUMN next_poll_at DATETIME;
ALTER TABLE discussion_sessions ADD COLUMN last_write_at DATETIME;
ALTER TABLE discussion_sessions ADD COLUMN write_state TEXT NOT NULL DEFAULT 'unknown'
    CHECK (write_state IN ('ok', 'failed', 'unknown'));
