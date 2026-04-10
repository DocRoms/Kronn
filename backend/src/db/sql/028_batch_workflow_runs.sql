-- Batch workflow runs: extend workflow_runs to track fan-out from a single
-- Quick Prompt to N parallel discussions, and link each child discussion back
-- to its batch run. Added 2026-04-10 for the Phase 1b batch workflows feature.

-- Batch tracking columns on workflow_runs.
-- `run_type` differentiates linear workflow runs (existing behaviour) from
-- batch runs (new in Phase 1b).
-- `batch_total` = target number of child discussions.
-- `batch_completed` = number of child discussions that finished successfully.
-- `batch_failed`    = number of child discussions that finished with an error.
-- `batch_name`      = display name shown in the sidebar group header, e.g.
--                     "Cadrage to-Frame — 10 avr 14:00". Null for linear runs.
ALTER TABLE workflow_runs ADD COLUMN run_type TEXT NOT NULL DEFAULT 'linear';
ALTER TABLE workflow_runs ADD COLUMN batch_total INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow_runs ADD COLUMN batch_completed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow_runs ADD COLUMN batch_failed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE workflow_runs ADD COLUMN batch_name TEXT;

-- Link child discussions back to their batch run.
-- Nullable: disc created outside a batch (manual, briefing, bootstrap) have no run.
-- ON DELETE SET NULL so deleting a batch run leaves its disc intact (user choice:
-- "archive batch" = clear the link, vs "delete batch" = explicit cascade in app layer).
ALTER TABLE discussions ADD COLUMN workflow_run_id TEXT REFERENCES workflow_runs(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_discussions_workflow_run_id ON discussions(workflow_run_id);
