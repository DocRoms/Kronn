-- KT-643 — SQLite cannot extend a CHECK constraint in place. Rebuild the
-- important-message table additively, retaining every existing row and index.
CREATE TABLE discussion_important_messages_next (
    id TEXT NOT NULL PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    category TEXT NOT NULL CHECK (category IN (
        'information', 'decision', 'scope_change', 'dod_waiver',
        'blocking_alert', 'human_action_required', 'accepted_delivery'
    )),
    schema_version INTEGER NOT NULL,
    dedup_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    author_kind TEXT NOT NULL CHECK (author_kind IN ('orchestrator', 'human')),
    author_label TEXT NOT NULL,
    source_kind TEXT,
    source_id TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (discussion_id, dedup_key)
);
INSERT INTO discussion_important_messages_next
SELECT id, discussion_id, message_id, category, schema_version, dedup_key,
       payload_json, author_kind, author_label, source_kind, source_id, created_at
FROM discussion_important_messages;
DROP TABLE discussion_important_messages;
ALTER TABLE discussion_important_messages_next RENAME TO discussion_important_messages;
CREATE UNIQUE INDEX idx_disc_important_message
    ON discussion_important_messages(message_id);
CREATE INDEX idx_disc_important_discussion
    ON discussion_important_messages(discussion_id, category);
CREATE INDEX idx_disc_important_source
    ON discussion_important_messages(source_kind, source_id)
    WHERE source_kind IS NOT NULL;
