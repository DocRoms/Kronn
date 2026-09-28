-- KT-883 — durable workflow-step identity for room activity, message
-- attribution and question delivery. Finished rows stay as provenance; only
-- `finished_at IS NULL` is live activity.
CREATE TABLE workflow_step_room_activities (
    run_id        TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    step_key      TEXT NOT NULL,
    workflow_id   TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    workflow_name TEXT NOT NULL,
    step_name     TEXT NOT NULL,
    disc_id       TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    agent_type    TEXT NOT NULL,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    PRIMARY KEY (run_id, step_key)
);
CREATE INDEX idx_workflow_step_room_activities_disc_live
    ON workflow_step_room_activities(disc_id, finished_at);

-- A resolved question may claim one workflow run. `resume_run_id` is reserved
-- before launch, so concurrent HTTP retries cannot create two runs.
ALTER TABLE discussion_questions ADD COLUMN resume_state TEXT;
ALTER TABLE discussion_questions ADD COLUMN resume_run_id TEXT;
ALTER TABLE discussion_questions ADD COLUMN resume_error TEXT;
