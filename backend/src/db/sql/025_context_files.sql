-- Context files: user-uploaded files attached to a discussion for agent context.
-- Text files: content extracted and stored in extracted_text.
-- Images: saved to disk, path in disk_path, agents reference by path.
CREATE TABLE IF NOT EXISTS context_files (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    filename        TEXT NOT NULL,
    mime_type       TEXT NOT NULL DEFAULT '',
    original_size   INTEGER NOT NULL DEFAULT 0,
    extracted_text  TEXT NOT NULL DEFAULT '',
    extracted_size  INTEGER NOT NULL DEFAULT 0,
    disk_path       TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_context_files_discussion ON context_files(discussion_id);
