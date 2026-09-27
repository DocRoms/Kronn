-- KT-793 — room sessions a workflow Agent step joined with its runner
-- capability. They leave when the step ends or the backend restarts, and a
-- later step of the same run in that room takes over their executions.
CREATE TABLE IF NOT EXISTS workflow_step_room_sessions (
    session_pk INTEGER PRIMARY KEY REFERENCES discussion_sessions(id) ON DELETE CASCADE,
    run_id     TEXT NOT NULL,
    step_key   TEXT NOT NULL,
    disc_id    TEXT NOT NULL,
    joined_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_workflow_step_room_sessions_run
    ON workflow_step_room_sessions(run_id, disc_id);
