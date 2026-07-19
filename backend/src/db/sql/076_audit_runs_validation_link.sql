-- 076 (Codex A5/A3): durable run→validation-discussion link + structured
-- per-step outcomes. `validation_discussion_id` is filled inside the SAME
-- transaction that inserts the discussion and stamps the run Completed —
-- the validate endpoint trusts ONLY this link (title/date heuristics were
-- forgeable). `step_outcomes_json` records requested/succeeded/unchanged
-- steps for partial runs (provenance for the drift oracle).
ALTER TABLE audit_runs ADD COLUMN validation_discussion_id TEXT;
ALTER TABLE audit_runs ADD COLUMN step_outcomes_json TEXT;
CREATE INDEX IF NOT EXISTS idx_audit_runs_validation_disc
    ON audit_runs(validation_discussion_id)
    WHERE validation_discussion_id IS NOT NULL;
