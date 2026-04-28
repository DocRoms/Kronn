-- 0.7.0 Phase 7 — compensating steps run on Failed.
--
-- Stored as JSON: `Vec<WorkflowStep>` serialised. NULL or empty `[]` = no
-- rollback. Fires only when the main pipeline ends in `RunStatus::Failed`
-- (not Cancelled, not StoppedByGuard, not Gate-Reject — those are
-- intentional stops). Each rollback step sees `{{failed_step.name}}` and
-- `{{failed_step.output}}` in addition to the regular template context.
ALTER TABLE workflows ADD COLUMN on_failure TEXT;
