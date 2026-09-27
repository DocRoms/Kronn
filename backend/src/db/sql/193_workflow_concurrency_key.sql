-- The template a workflow's concurrency limit is counted by, and the key each
-- run rendered from it at launch. NULL keeps the limit per workflow.
ALTER TABLE workflows ADD COLUMN concurrency_key TEXT;
ALTER TABLE workflow_runs ADD COLUMN concurrency_key TEXT;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_active_concurrency_key
    ON workflow_runs(workflow_id, concurrency_key)
    WHERE status IN ('Pending', 'Running');
