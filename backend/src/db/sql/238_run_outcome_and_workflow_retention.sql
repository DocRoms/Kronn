-- KT-1100 — a finished run can be marked 'no_op' (no effect) or 'changed';
-- NULL means not classified (legacy, unfinished, failed or child runs).
-- A workflow can carry its own retention, overriding the global one.
ALTER TABLE workflow_runs ADD COLUMN outcome TEXT;
ALTER TABLE workflows ADD COLUMN retention_json TEXT;
-- The run list hides no-op runs and counts them per workflow; the purge
-- selects them by age.
CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_outcome
    ON workflow_runs(workflow_id, outcome, started_at);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_no_op_retention
    ON workflow_runs(finished_at) WHERE outcome = 'no_op';
