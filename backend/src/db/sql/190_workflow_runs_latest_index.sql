-- The latest run per workflow is found from this index alone: without it the
-- aggregation reads every run row, including its large step results.
CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_started
    ON workflow_runs(workflow_id, started_at);
