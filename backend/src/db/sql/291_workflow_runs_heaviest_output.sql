-- KT-1525 — the heaviest step output of a run (bytes, and its step's name), so
-- the workflow list can warn about a heavy output without reading the run's
-- step results (see 212: the list is answered from a covering index). 0 / NULL
-- on a run recorded before this column: unknown, never a warning.
ALTER TABLE workflow_runs ADD COLUMN heaviest_output_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow_runs ADD COLUMN heaviest_output_step TEXT;
DROP INDEX IF EXISTS idx_workflow_runs_summary;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_summary
    ON workflow_runs(workflow_id, started_at, id, status, finished_at, tokens_used,
                     heaviest_output_bytes, heaviest_output_step);
DROP INDEX IF EXISTS idx_workflow_runs_project_summary;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_project_summary
    ON workflow_runs(project_id, workflow_id, started_at, id, status, finished_at, tokens_used,
                     heaviest_output_bytes, heaviest_output_step)
    WHERE project_id IS NOT NULL;
