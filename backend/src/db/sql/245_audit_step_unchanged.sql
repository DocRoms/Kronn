-- KT-1130: a Full or partial audit step whose target was already up to date
-- (every gate green, file identical before and after the agent) ends
-- `unchanged`. 1 marks it; 0 is every other outcome, legacy rows included.
ALTER TABLE audit_run_steps ADD COLUMN unchanged INTEGER NOT NULL DEFAULT 0;
