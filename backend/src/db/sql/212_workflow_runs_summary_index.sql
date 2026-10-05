-- KT-1019 — the workflow list and the token stats read only these columns.
-- They sit after the run's step results, so without a covering index each of
-- those queries walked every payload page of the table (9 GB on a 10 GB base).
CREATE INDEX IF NOT EXISTS idx_workflow_runs_summary
    ON workflow_runs(workflow_id, started_at, id, status, finished_at, tokens_used);
