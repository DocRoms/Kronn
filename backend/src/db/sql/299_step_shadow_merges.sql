-- KT-1544 — the optional merge of a step's main execution with its shadows. One
-- row per merged step of a run: which shadows it selected, which finished and
-- which are missing, what the condense pass kept, dropped as duplicate or as
-- unverified (`report_json`, with the provenance of every point), and the
-- merged result that became the step's output. The main execution's own output
-- stays in its `step_shadow_runs` row, so the two can be compared.
CREATE TABLE IF NOT EXISTS step_shadow_merges (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    result_index  INTEGER NOT NULL,
    step_name     TEXT NOT NULL,
    -- waiting | condensing | merged | skipped | rejected | failed
    status        TEXT NOT NULL,
    report_json   TEXT NOT NULL,
    merged_output TEXT NOT NULL DEFAULT '',
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    UNIQUE (run_id, result_index)
);
CREATE INDEX IF NOT EXISTS idx_step_shadow_merges_run ON step_shadow_merges(run_id);
