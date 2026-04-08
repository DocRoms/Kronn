-- Quick Prompts: reusable prompt templates with {{variables}}
CREATE TABLE IF NOT EXISTS quick_prompts (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    icon            TEXT NOT NULL DEFAULT '⚡',
    prompt_template TEXT NOT NULL,
    variables_json  TEXT NOT NULL DEFAULT '[]',
    agent           TEXT NOT NULL DEFAULT 'ClaudeCode',
    project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
    skill_ids_json  TEXT NOT NULL DEFAULT '[]',
    tier            TEXT NOT NULL DEFAULT 'default',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_quick_prompts_project ON quick_prompts(project_id);
