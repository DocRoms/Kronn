-- Link a child batch workflow run back to the linear workflow run that
-- spawned it via a `BatchQuickPrompt` step (Phase 2 batch workflows, 2026-04-10).
--
-- Nullable: top-level runs (both linear runs and manual batches triggered
-- from the UI) have no parent. ON DELETE SET NULL so deleting a linear
-- parent run keeps its child batch run browsable (the user may want to
-- inspect what was produced before the parent was wiped).
ALTER TABLE workflow_runs ADD COLUMN parent_run_id TEXT
    REFERENCES workflow_runs(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_workflow_runs_parent ON workflow_runs(parent_run_id);
