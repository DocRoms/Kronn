-- KT-562 — remember how far a resumed conversation has already been told.
--
-- Resuming a session lets the next turn send only what the agent has not seen.
-- "Only the new message" is wrong in a room: between two turns of the SAME
-- agent, the human or another agent may have written, and sending just the
-- last message would drop that silently — worse than the full-history replay
-- it replaces, because the loss is invisible.
--
-- So the session row remembers the id of the last message this agent was
-- shown, and the next turn sends everything after it. An id rather than a
-- position on purpose: if that message is no longer in the history — edited
-- away, pruned, a discussion rebuilt — it simply is not found, and the caller
-- falls back to the full prompt. A numeric cursor would keep pointing at
-- something, and quietly send the wrong slice.
--
-- NULL means "resumed conversation of unknown extent", read the same way.
-- Rows written before this migration read that way, so no backfill is needed.
ALTER TABLE acp_runtime_sessions
    ADD COLUMN last_seen_message_id TEXT;
