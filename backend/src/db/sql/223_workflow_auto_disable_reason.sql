-- KT-1037: why Kronn turned a workflow off on its own (an agent's edit, an
-- agent's change to a Quick API/Prompt it uses, an agent's create, an
-- import), so the Automations page can list them for a human to review.
-- Cleared when a human enables the workflow again.
ALTER TABLE workflows ADD COLUMN disabled_reason TEXT;
ALTER TABLE workflows ADD COLUMN disabled_at TEXT;
ALTER TABLE workflows ADD COLUMN disabled_by TEXT;
ALTER TABLE workflows ADD COLUMN disabled_summary TEXT;
