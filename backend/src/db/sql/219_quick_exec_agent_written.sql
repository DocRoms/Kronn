-- KT-1017 — who last wrote a Quick Exec's line, apart from its approval: an
-- agent-written line can then be approved and stay the agent's.
ALTER TABLE quick_execs ADD COLUMN agent_written INTEGER NOT NULL DEFAULT 0;
-- The approval column briefly encoded "agent-written, not approved" as 2.
UPDATE quick_execs SET agent_written = 1, unmodelled_args_approved = 0
 WHERE unmodelled_args_approved = 2;
