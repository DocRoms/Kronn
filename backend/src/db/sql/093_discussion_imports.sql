-- Idempotency ledger for portable discussion bundles.
--
-- A source discussion may be imported once per Kronn instance. Replaying the
-- exact same bundle returns the first imported discussion; replaying a bundle
-- with the same source id but different content is reported as a conflict.

CREATE TABLE IF NOT EXISTS discussion_imports (
    source_discussion_id   TEXT PRIMARY KEY,
    content_sha256         TEXT NOT NULL,
    imported_discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    imported_at            TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_imports_target
    ON discussion_imports(imported_discussion_id);
