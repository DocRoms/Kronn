-- KT-983 — `run_type` is stored after the run's step results, which can hold
-- hundreds of KB per run. Without an index, `WHERE run_type = 'batch'` walked
-- every run's results to reach it: 20-40 s on a 10 GB base, on the page that
-- lists discussions. Building it walks them once.
CREATE INDEX IF NOT EXISTS idx_workflow_runs_type_started
    ON workflow_runs(run_type, started_at);
