ALTER TABLE quick_prompts
ADD COLUMN connection_id TEXT
REFERENCES external_api_connections(id) ON DELETE SET NULL;

ALTER TABLE quick_prompt_versions
ADD COLUMN connection_id TEXT
REFERENCES external_api_connections(id) ON DELETE SET NULL;
