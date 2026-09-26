-- The run whose TriggerWorkflow step launched this one. Separate from
-- parent_run_id: a triggered run is neither cancelled nor resumed with it.
ALTER TABLE workflow_runs ADD COLUMN triggered_by_run_id TEXT;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_triggered_by
    ON workflow_runs(triggered_by_run_id)
    WHERE triggered_by_run_id IS NOT NULL;
