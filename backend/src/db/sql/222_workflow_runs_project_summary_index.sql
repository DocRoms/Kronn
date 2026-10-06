-- KT-1050 — a bridge token's workflow list filters runs by project. Without
-- an index on project_id that query walked every run, step payload included.
-- Covering, so the latest-run summary is answered from the index alone.
CREATE INDEX IF NOT EXISTS idx_workflow_runs_project_summary
    ON workflow_runs(project_id, workflow_id, started_at, id, status, finished_at, tokens_used)
    WHERE project_id IS NOT NULL;
