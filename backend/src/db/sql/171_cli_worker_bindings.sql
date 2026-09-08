-- The bridge's durable source identity is not discussion_sessions.session_id.
-- Keep the accepted identity per exact worker assignment, including superseded
-- assignments, so return/reassignment never guesses an owner from a room.
CREATE TABLE task_execution_cli_bindings (
    task_execution_id TEXT NOT NULL REFERENCES task_executions(id) ON DELETE CASCADE,
    cli_session_id INTEGER NOT NULL,
    source_agent TEXT NOT NULL CHECK (length(trim(source_agent)) > 0),
    source_session_id TEXT NOT NULL CHECK (length(trim(source_session_id)) > 0),
    pinned_at TEXT NOT NULL,
    PRIMARY KEY (task_execution_id, cli_session_id)
);
