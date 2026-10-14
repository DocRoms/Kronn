-- Explicit opt-in: native replies explain a workflow room without executing work.
-- no_agent remains the master switch; ordinary rooms keep their existing policy.
ALTER TABLE discussions ADD COLUMN workflow_observer INTEGER NOT NULL DEFAULT 0;
