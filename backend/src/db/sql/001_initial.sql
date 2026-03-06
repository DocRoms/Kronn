-- ═══════════════════════════════════════════════════════════════════════════════
-- Kronn — Initial schema
-- ═══════════════════════════════════════════════════════════════════════════════

-- Projects
CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    path        TEXT NOT NULL UNIQUE,
    repo_url    TEXT,
    token_override_json TEXT,          -- JSON: { provider, token } or null
    ai_config_json      TEXT NOT NULL DEFAULT '{"detected":false,"configs":[]}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- MCP instances attached to projects
CREATE TABLE IF NOT EXISTS mcps (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    definition_id   TEXT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    enabled         INTEGER NOT NULL DEFAULT 1,
    config_json     TEXT,              -- JSON blob
    transport_json  TEXT,              -- JSON blob
    source          TEXT NOT NULL DEFAULT 'manual'
);

CREATE INDEX IF NOT EXISTS idx_mcps_project ON mcps(project_id);

-- Scheduled tasks (legacy — will become workflows)
CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    cron_expr       TEXT NOT NULL,
    human_interval  TEXT NOT NULL DEFAULT '',
    agent           TEXT NOT NULL DEFAULT 'ClaudeCode',
    prompt          TEXT NOT NULL,
    active          INTEGER NOT NULL DEFAULT 1,
    last_run        TEXT,
    last_status_json TEXT,             -- JSON blob
    tokens_used     INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id);

-- Discussions
CREATE TABLE IF NOT EXISTS discussions (
    id              TEXT PRIMARY KEY,
    project_id      TEXT,              -- nullable for global discussions
    title           TEXT NOT NULL,
    agent           TEXT NOT NULL DEFAULT 'ClaudeCode',
    language        TEXT NOT NULL DEFAULT 'fr',
    participants_json TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_discussions_project ON discussions(project_id);

-- Discussion messages
CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,      -- 'User', 'Agent', 'System'
    content         TEXT NOT NULL,
    agent_type      TEXT,              -- nullable
    timestamp       TEXT NOT NULL,
    sort_order      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_messages_discussion ON messages(discussion_id);
CREATE INDEX IF NOT EXISTS idx_messages_order ON messages(discussion_id, sort_order);
