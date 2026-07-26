-- During pre-release development, migration 083 was applied locally with a
-- unique index covering both Pending and Running jobs. Editing 083 afterwards
-- cannot repair databases that already recorded it as applied. Rebuild the
-- index so only concurrent Running claims are forbidden; multiple durable
-- Pending turns are expected.
DROP INDEX IF EXISTS idx_agent_dispatch_one_active_discussion;
DROP INDEX IF EXISTS idx_agent_dispatch_one_running_discussion;

CREATE UNIQUE INDEX idx_agent_dispatch_one_running_discussion
ON agent_dispatch_jobs(discussion_id)
WHERE status = 'Running';
