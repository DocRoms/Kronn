-- KT-1957 — what a run's shadows spent, beside `tokens_used` (its steps, the
-- merge pass included). Shadows are detached from the run's own total, so the
-- run list, the workflow card and the statistics read both from the covering
-- indexes (see 212, 291) instead of joining the shadow rows.
ALTER TABLE workflow_runs ADD COLUMN shadow_tokens_used INTEGER NOT NULL DEFAULT 0;
UPDATE workflow_runs
   SET shadow_tokens_used = (
       SELECT COALESCE(SUM(s.tokens_used), 0) FROM step_shadow_runs s
        WHERE s.run_id = workflow_runs.id AND s.shadow_id != '')
 WHERE id IN (SELECT run_id FROM step_shadow_runs WHERE shadow_id != '');
DROP INDEX IF EXISTS idx_workflow_runs_summary;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_summary
    ON workflow_runs(workflow_id, started_at, id, status, finished_at, tokens_used,
                     heaviest_output_bytes, heaviest_output_step, shadow_tokens_used);
DROP INDEX IF EXISTS idx_workflow_runs_project_summary;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_project_summary
    ON workflow_runs(project_id, workflow_id, started_at, id, status, finished_at, tokens_used,
                     heaviest_output_bytes, heaviest_output_step, shadow_tokens_used)
    WHERE project_id IS NOT NULL;
