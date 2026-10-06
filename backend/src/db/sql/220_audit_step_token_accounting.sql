-- What an audit step's stored input contains (`db::audit_runs::TokenAccounting`):
-- `inclusive_read_write` (Claude, made inclusive), `inclusive_read` (Codex) or
-- `as_reported` (cache split unknown). NULL marks a row stored before it was
-- recorded: its `step_tokens` was input plus output with the cache included,
-- and is shown as stored, without a breakdown or an estimated cost.
ALTER TABLE audit_run_steps ADD COLUMN token_accounting TEXT;
