-- 0.8.5 — Track real wall-clock duration per agent message.
--
-- Pre-0.8.5 the UI computed "duration" as `msg.timestamp - prev_user_ts`,
-- which silently included the user's typing time. Useless for any
-- aggregate ("avg duration of this QP's first reply"). We now stamp
-- the actual delta between `agent_run_started_at` and the message
-- commit at the streaming layer.
--
-- INTEGER, milliseconds. NULL for legacy rows + User/System rows
-- (only Agent commits carry it). Index NOT added — queries that
-- aggregate this scan small windows scoped to a discussion or
-- originating_qp_id (other indexes catch the predicate).

ALTER TABLE messages ADD COLUMN duration_ms INTEGER;
