-- KT-1459 — shadows live on the step. A step re-runs beside its main execution
-- with another agent, model, prompt or skills (or none of these, to measure its
-- determinism); this table keeps what each execution was and produced, the main
-- one included (`shadow_id` = '', identity only: its output and metrics are the
-- run's own step result). `workflows.shadow_json` now holds the step shadows
-- keyed by step id; the Rust half of this migration moves a workflow-level
-- shadow (KT-1449) onto each of its Agent steps.
CREATE TABLE IF NOT EXISTS step_shadow_runs (
    id            TEXT PRIMARY KEY,
    run_id        TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    -- Where the main execution of the step sits in the run's step results.
    result_index  INTEGER NOT NULL,
    step_name     TEXT NOT NULL,
    shadow_id     TEXT NOT NULL,
    label         TEXT NOT NULL DEFAULT '',
    -- running | success | failed | skipped | over_budget | timed_out | interrupted
    status        TEXT NOT NULL,
    -- What the execution loaded: agent, model, connection, prompt, skills.
    identity_json TEXT NOT NULL,
    output        TEXT NOT NULL DEFAULT '',
    tokens_used   INTEGER,
    duration_ms   INTEGER,
    cost_usd      REAL,
    model         TEXT,
    -- Why a budget, the time limit or a skip ended the shadow.
    stopped       TEXT,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    UNIQUE (run_id, result_index, shadow_id)
);
CREATE INDEX IF NOT EXISTS idx_step_shadow_runs_run ON step_shadow_runs(run_id);
