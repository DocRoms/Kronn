-- 0.8.4 (#294) — Cross-agent memory MCP.
-- Lets CLI agents (Claude Code, Cursor, Codex, …) push their
-- conversation history into Kronn DB so the SAME discussion thread
-- can be picked up by a DIFFERENT agent later. The 10 new MCP tools
-- (disc_create / disc_append / disc_link / disc_unlink /
-- disc_find_by_session / disc_search / disc_load_other + the 3
-- existing ones) operate on the schema below.
--
-- Design — see `project_cross_agent_memory_0_8_4.md` in memory:
--   - 3 nullable columns on `discussions` carry the CURRENT source
--     binding (last link wins);
--   - `disc_source_history` is the append-only chain so we can
--     answer "this disc was first owned by ClaudeCode session X,
--     then Cursor session Y, then Codex session Z";
--   - `discussion_messages.source_msg_id` makes `disc_append`
--     idempotent — re-pushing the same exported transcript doesn't
--     duplicate messages.
--   - `discussions.diverged_at` flags discs where the Kronn UI was
--     edited after an import, so a later `disc_append` doesn't
--     silently overwrite user edits.

ALTER TABLE discussions ADD COLUMN source_agent TEXT;
ALTER TABLE discussions ADD COLUMN source_session_id TEXT;
ALTER TABLE discussions ADD COLUMN imported_at DATETIME;
ALTER TABLE discussions ADD COLUMN diverged_at DATETIME;

-- Append-only chain: every disc_link / disc_unlink writes a row here.
-- `unlinked_at IS NULL` = currently bound to this session.
CREATE TABLE disc_source_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    disc_id TEXT NOT NULL,
    source_agent TEXT NOT NULL,
    source_session_id TEXT NOT NULL,
    linked_at DATETIME NOT NULL,
    unlinked_at DATETIME,
    FOREIGN KEY (disc_id) REFERENCES discussions(id) ON DELETE CASCADE
);
CREATE INDEX idx_disc_src_hist_lookup ON disc_source_history(source_agent, source_session_id);
CREATE INDEX idx_disc_src_hist_disc ON disc_source_history(disc_id);

-- Dedup messages on append: an exported transcript provides its own
-- message ids (CC uses UUIDs; for others we hash content+role). Same
-- `(disc_id, source_msg_id)` pair = same message, skip the insert.
-- The Kronn messages table is named `messages`, not `discussion_messages`
-- (see migration 001).
ALTER TABLE messages ADD COLUMN source_msg_id TEXT;
CREATE INDEX idx_msg_source_dedup ON messages(discussion_id, source_msg_id);
