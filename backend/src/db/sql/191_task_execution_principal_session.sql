-- KT-790 — the joined CLI session that steers an execution from its parent
-- room. Orchestrator notices address it, so its wait wakes on them.
ALTER TABLE task_executions ADD COLUMN principal_cli_session_id INTEGER;
