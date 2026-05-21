-- 0.8.6 (#24) — unified persistent log for ALL Kronn-mediated API calls.
-- Sources: 'workflow' (ApiCall/BatchApiCall step), 'agent_broker'
-- (agent-driven calls via the kronn-internal MCP `api_call` tool),
-- 'manual_test' (the SetupWizard test endpoint).
--
-- Replaces the single-purpose `agent_api_calls` table sketched in the
-- 0.8.6 broker plan with one canonical store everyone writes to.
--
-- Request / response excerpts are capped at 2KB after secret redaction.

CREATE TABLE IF NOT EXISTS api_call_logs (
    id               TEXT PRIMARY KEY,
    source           TEXT NOT NULL CHECK (source IN ('workflow', 'agent_broker', 'manual_test')),
    project_id       TEXT,
    run_id           TEXT,
    disc_id          TEXT,
    agent            TEXT,
    plugin_slug      TEXT NOT NULL,
    config_id        TEXT,
    endpoint_path    TEXT NOT NULL,
    method           TEXT NOT NULL,
    http_status      INTEGER,
    status           TEXT NOT NULL CHECK (status IN ('OK', 'ERROR', 'RateLimited', 'TimedOut')),
    duration_ms      INTEGER NOT NULL,
    request_excerpt  TEXT,
    response_excerpt TEXT,
    error_message    TEXT,
    called_at        TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_api_logs_project ON api_call_logs(project_id, called_at DESC);
CREATE INDEX IF NOT EXISTS idx_api_logs_run     ON api_call_logs(run_id);
CREATE INDEX IF NOT EXISTS idx_api_logs_disc    ON api_call_logs(disc_id);
CREATE INDEX IF NOT EXISTS idx_api_logs_plugin  ON api_call_logs(plugin_slug, called_at DESC);
CREATE INDEX IF NOT EXISTS idx_api_logs_called  ON api_call_logs(called_at DESC);
