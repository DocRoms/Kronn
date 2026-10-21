-- KT-1131: a human may accept a failed audit step as it stands. Who accepted
-- it, when and why; NULL on every step nobody accepted, legacy rows included.
ALTER TABLE audit_run_steps ADD COLUMN accepted_at TEXT;
ALTER TABLE audit_run_steps ADD COLUMN accepted_by TEXT;
ALTER TABLE audit_run_steps ADD COLUMN accepted_reason TEXT;
