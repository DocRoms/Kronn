-- 0.9.2 KT-37 — carry the agent + attempted model through the two provenance
-- gaps: (1) a mid-stream checkpoint recovered after a backend restart, and
-- (2) an external JOIN participant that self-declares its model at handshake.
--
-- Split from 088: 088 was already applied on dev DBs, and the runner never
-- re-applies a recorded migration. New columns MUST live in their own file so
-- a dev DB (088-only) and a fresh 086→…→089 upgrade converge on one schema.

-- Provenance for a recovered partial response: which agent was producing it,
-- and the concrete model it was attempting (NULL for provider-default runs
-- with no --model flag, and for legacy checkpoints written before 089).
ALTER TABLE discussions ADD COLUMN partial_response_agent_type TEXT;
ALTER TABLE discussions ADD COLUMN partial_response_model TEXT;

-- A JOIN participant may DECLARE the model it runs on. Declared at join,
-- durable, never inferred: NULL means "not declared" and the UI shows it as
-- such. An explicit value on a later rebind updates it; an omission never
-- overwrites an already-declared value.
ALTER TABLE discussion_sessions ADD COLUMN model TEXT;
