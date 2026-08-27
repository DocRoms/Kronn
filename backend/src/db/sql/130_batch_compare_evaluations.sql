-- Durable, independent human and AI quality signals for Quick Prompt Compare.
-- One AI judge run grades every usable child answer in a blind pass; the
-- per-discussion table keeps the latest verdict next to the human rating.

CREATE TABLE IF NOT EXISTS batch_compare_judge_runs (
    id                  TEXT PRIMARY KEY,
    run_id              TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    judge_discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    judge_agent_json    TEXT NOT NULL,
    judge_tier_json     TEXT NOT NULL,
    rubric_version      TEXT NOT NULL,
    labels_json         TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'Running'
                        CHECK (status IN ('Running', 'Completed', 'Failed')),
    error               TEXT,
    tokens_used         INTEGER,
    duration_ms         INTEGER,
    model               TEXT,
    started_at          TEXT NOT NULL,
    finished_at         TEXT
);

CREATE INDEX IF NOT EXISTS idx_compare_judge_runs_run
    ON batch_compare_judge_runs(run_id, started_at DESC);

CREATE UNIQUE INDEX IF NOT EXISTS idx_compare_judge_one_running
    ON batch_compare_judge_runs(run_id) WHERE status = 'Running';

CREATE TABLE IF NOT EXISTS batch_compare_evaluations (
    run_id                   TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    discussion_id            TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    manual_score             INTEGER CHECK (manual_score BETWEEN 1 AND 5),
    manual_updated_at        TEXT,
    ai_score                 INTEGER CHECK (ai_score BETWEEN 1 AND 5),
    ai_confidence            REAL CHECK (ai_confidence BETWEEN 0 AND 1),
    ai_positives_json        TEXT NOT NULL DEFAULT '[]',
    ai_negatives_json        TEXT NOT NULL DEFAULT '[]',
    ai_violations_json       TEXT NOT NULL DEFAULT '[]',
    ai_judge_run_id          TEXT REFERENCES batch_compare_judge_runs(id) ON DELETE SET NULL,
    ai_updated_at            TEXT,
    PRIMARY KEY (run_id, discussion_id)
);

CREATE INDEX IF NOT EXISTS idx_compare_evaluations_run
    ON batch_compare_evaluations(run_id);
