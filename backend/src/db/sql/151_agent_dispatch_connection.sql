-- 0.12.0 — keep the exact named external API connection on durable dispatches.
-- Several connections can use AgentType::Custom, so the agent enum alone is
-- insufficient to recover the endpoint, credential and tier models at runtime.
ALTER TABLE agent_dispatch_jobs
    ADD COLUMN connection_id TEXT
        REFERENCES external_api_connections(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_agent_dispatch_jobs_connection
    ON agent_dispatch_jobs(connection_id);
