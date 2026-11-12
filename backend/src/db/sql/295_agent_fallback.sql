-- KT-1518 — agent fallback chain.
--
-- `agent_fallback_cooldowns`: an agent whose provider refused a turn (rate
-- limit, quota or credits spent, outage, not authenticated) is left alone until
-- the reset the provider announced, or a short re-try window. New turns go
-- straight to its fallback meanwhile and return to it by themselves: an expired
-- row is simply not read, so there is nothing to clean up.
CREATE TABLE IF NOT EXISTS agent_fallback_cooldowns (
    agent     TEXT PRIMARY KEY,
    until     TEXT NOT NULL,
    reason    TEXT NOT NULL,
    noted_at  TEXT NOT NULL
);

-- `dispatch_fallback_pins`: a discussion turn that continued on a fallback.
-- The dispatch reruns on `to_agent` with the tier it already had (a model is
-- the link's own, never the failing agent's); `hops` bounds how many times one
-- turn may move on.
CREATE TABLE IF NOT EXISTS dispatch_fallback_pins (
    dispatch_job_id TEXT PRIMARY KEY
                      REFERENCES agent_dispatch_jobs(id) ON DELETE CASCADE,
    from_agent      TEXT NOT NULL,
    to_agent        TEXT NOT NULL,
    model           TEXT,
    tier            TEXT NOT NULL,
    hops            INTEGER NOT NULL DEFAULT 1,
    reason          TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

-- `discussion_fallback_overrides`: a discussion's own fallback chain. A row
-- replaces the default chain of the agent the turn starts on; an empty list
-- (`[]`) is "no fallback in this discussion". No row: the default chain applies.
CREATE TABLE IF NOT EXISTS discussion_fallback_overrides (
    discussion_id TEXT PRIMARY KEY REFERENCES discussions(id) ON DELETE CASCADE,
    chain_json    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

-- `execution_fallback_origins`: the agent a task worker STARTED on. A worker
-- that moved to a fallback is reassigned, so the chain it keeps walking is the
-- one of this agent, not of the link it is on.
CREATE TABLE IF NOT EXISTS execution_fallback_origins (
    execution_id TEXT PRIMARY KEY,
    origin_agent TEXT NOT NULL
);
