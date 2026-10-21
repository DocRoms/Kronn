-- 262 (KT-1122): durable validation completion, who started and who validated.
-- `validation_completed_at` is set once, when the linked discussion first
-- ends an agent message on KRONN:VALIDATION_COMPLETE; later messages never
-- clear it. Only an explicit relink to a new discussion resets it.
ALTER TABLE audit_runs ADD COLUMN validation_completed_at TEXT;
ALTER TABLE audit_runs ADD COLUMN validation_completed_message_id TEXT;
ALTER TABLE audit_runs ADD COLUMN validation_started_by TEXT;
ALTER TABLE audit_runs ADD COLUMN validated_by TEXT;
ALTER TABLE audit_runs ADD COLUMN validated_at TEXT;
