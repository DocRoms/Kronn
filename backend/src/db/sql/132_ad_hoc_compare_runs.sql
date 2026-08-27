-- A free comparison is represented by a lightweight workflow_run so the
-- existing durable human/AI evaluation tables can keep using their run_id
-- foreign key. Unlike a batch, its candidate discussions already exist and
-- therefore cannot be linked through discussions.workflow_run_id (that would
-- destroy their original run provenance). This table owns the ordered scope.

CREATE TABLE IF NOT EXISTS compare_run_scopes (
    run_id          TEXT PRIMARY KEY REFERENCES workflow_runs(id) ON DELETE CASCADE,
    selection_key   TEXT NOT NULL UNIQUE,
    created_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS compare_run_discussions (
    run_id          TEXT NOT NULL REFERENCES compare_run_scopes(run_id) ON DELETE CASCADE,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    PRIMARY KEY (run_id, discussion_id),
    UNIQUE (run_id, position)
);

CREATE INDEX IF NOT EXISTS idx_compare_run_discussions_discussion
    ON compare_run_discussions(discussion_id);
