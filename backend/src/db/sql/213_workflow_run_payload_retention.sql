-- KT-984 — run retention blanks the step outputs of old runs in place.
-- `payload_compacted_at` records it, so a trimmed run is not rewritten again.
-- Every column the retention selection reads sits after the step results;
-- the partial index answers it without walking a run's payload, and a
-- trimmed run leaves the index.
ALTER TABLE workflow_runs ADD COLUMN payload_compacted_at TEXT;
CREATE INDEX IF NOT EXISTS idx_workflow_runs_payload_retention
    ON workflow_runs(finished_at, status, run_type, workspace_path, parent_run_id, id,
                     payload_compacted_at)
    WHERE payload_compacted_at IS NULL;
