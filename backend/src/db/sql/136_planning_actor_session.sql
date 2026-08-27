-- KT-324 — distinguish two joined CLI sessions of the same provider in the
-- planning audit log. `actor_id` remains the provider/alias for compatibility;
-- this durable source-session id is the second half of the typed identity.
ALTER TABLE planning_task_events ADD COLUMN actor_session_id TEXT;
ALTER TABLE task_execution_events ADD COLUMN actor_session_id TEXT;
