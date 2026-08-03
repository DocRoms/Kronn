-- KT-157 — one-shot catch-up of user turns a joined CLI never saw.
--
-- In a room whose native discussion agent is active, an untargeted User turn
-- routes to the native principal and is withheld from joined CLIs by design.
-- When the user actually meant the CLI (no mention typed), that turn is lost
-- for it forever. `native_fallback` marks exactly those turns: untargeted
-- User turns dispatched to the native while at least one CLI session existed
-- in the room. The wait path replays them ONCE per durable session as
-- catch-up context.
ALTER TABLE messages ADD COLUMN native_fallback INTEGER NOT NULL DEFAULT 0;

-- Durable per-session catch-up cursor: the highest sort_order this session
-- has already been caught up to. Kept on the DURABLE session row (not the
-- bridge) so an MCP reload or re-join never replays the batch.
ALTER TABLE discussion_sessions ADD COLUMN user_catchup_cursor INTEGER NOT NULL DEFAULT 0;

-- Existing sessions must not replay the whole room history on their first
-- wait after this migration: start them at the room's current tip.
UPDATE discussion_sessions
   SET user_catchup_cursor = COALESCE(
        (SELECT MAX(m.sort_order) FROM messages m
          WHERE m.discussion_id = discussion_sessions.disc_id),
        0);
