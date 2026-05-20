-- 0.8.6 phase 2 — Discussion-first refactor : decouple disc lifecycle from agent session.
--
-- Today a `discussions` row is tightly bound to ONE agent session via
-- `source_agent` + `source_session_id` (added migration 054). That works
-- for 1 disc ↔ 1 CLI, but blocks the multi-agent collab use case where
-- N CLIs participate in the same disc (Claude + Codex + Gemini for the
-- same RGPD audit, etc.).
--
-- Design — see `project_cross_agent_collab_demo.md` in memory :
--   - `discussion_sessions` = the participants table. One row per
--     (disc, agent_type, session_id) tuple. A disc with 0 sessions
--     is allowed (= user-only brainstorm phase or just-created topic
--     awaiting an invite).
--   - `role` distinguishes the disc creator ('owner') from later
--     joiners ('peer'). The owner CAN leave too — the disc survives.
--   - `status` tracks live participation : 'active' (currently bound
--     to a running CLI), 'paused' (bound but the CLI is idle / user
--     intervened), 'left' (the session left the disc, history
--     preserved for the participants header).
--   - `discussion_invite_tokens` is the short-lived ([+ Inviter])
--     bridge that lets a host-launched CLI join a disc without the
--     env-var injection that Kronn-launched agents get for free.
--     `token_hash` only — the plain token only ever lives in the
--     response of `POST /api/discussions/:id/invite-peer` so a leaked
--     DB row can't be used to join.
--
-- Backfill : every existing `discussions` row with `source_agent` set
-- becomes one 'owner' row with status='active' (joined_at = the disc's
-- own created_at). Discs created before migration 054 (no source_agent)
-- get NO seed row — they're either historical Kronn-launched without
-- the metadata, or empty-shell topics. Either way the new code paths
-- behave correctly without a seed row.

CREATE TABLE discussion_sessions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    disc_id             TEXT    NOT NULL,
    agent_type          TEXT    NOT NULL,
    -- Session id from the CLI's transcript export, or a Kronn-minted
    -- UUID for sessions launched from the UI. NULL is permitted only
    -- briefly during invite acceptance (between token validation and
    -- the agent's first message) so the unique index below is partial.
    session_id          TEXT,
    role                TEXT    NOT NULL CHECK (role IN ('owner', 'peer')),
    status              TEXT    NOT NULL CHECK (status IN ('active', 'paused', 'left')),
    joined_at           DATETIME NOT NULL,
    left_at             DATETIME,
    FOREIGN KEY (disc_id) REFERENCES discussions(id) ON DELETE CASCADE
);

-- Fast lookup : "list participants for this disc" (header rendering,
-- broadcast fan-out, /invite UX).
CREATE INDEX idx_disc_sessions_disc ON discussion_sessions(disc_id, status);

-- Fast lookup : "find the disc this CLI session is currently bound to"
-- (used by the bridge when it gets a tool call from an agent and
-- needs to verify the caller is still an active participant).
-- Partial: only enforce uniqueness for active/paused — a session can
-- be 'left' multiple times historically (rejoin/leave cycles).
CREATE UNIQUE INDEX idx_disc_sessions_session_active
    ON discussion_sessions(agent_type, session_id)
    WHERE session_id IS NOT NULL AND status != 'left';

-- Single-use, TTL'd invite tokens.
CREATE TABLE discussion_invite_tokens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Hash, not plaintext. Plain token returned ONLY in the invite
    -- response and never persisted in any log or table.
    token_hash  TEXT    NOT NULL UNIQUE,
    disc_id     TEXT    NOT NULL,
    created_at  DATETIME NOT NULL,
    expires_at  DATETIME NOT NULL,
    -- NULL = pending. Set on first successful `disc_join`. Used-tokens
    -- aren't deleted so we can answer "who joined via which invite"
    -- in the audit panel later.
    used_at     DATETIME,
    used_by_session_id INTEGER,
    FOREIGN KEY (disc_id) REFERENCES discussions(id) ON DELETE CASCADE,
    FOREIGN KEY (used_by_session_id) REFERENCES discussion_sessions(id) ON DELETE SET NULL
);
CREATE INDEX idx_invite_tokens_disc ON discussion_invite_tokens(disc_id);

-- Backfill from `discussions.source_agent` / `source_session_id` (set
-- by migration 054 + every Kronn-launched session). One 'owner' row
-- per existing disc with metadata.
INSERT INTO discussion_sessions (disc_id, agent_type, session_id, role, status, joined_at)
SELECT id, source_agent, source_session_id, 'owner', 'active', created_at
FROM discussions
WHERE source_agent IS NOT NULL AND source_session_id IS NOT NULL;
