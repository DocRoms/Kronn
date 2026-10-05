-- Design note agent-secret-boundary §4.5 (D2): which projects hand their
-- agents a GitHub token. No row means "not connected", the default of a new
-- project. The upgrade seeding of existing GitHub projects runs in Rust
-- (migrations.rs) because it reads each repository's remote.
CREATE TABLE IF NOT EXISTS project_github_connections (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('not_connected', 'gh_login', 'stored_token')),
    -- AES-GCM ciphertext under the instance key; set only for 'stored_token'.
    token_encrypted TEXT,
    -- Last scope read from the GitHub API (JSON GithubScope), never a token.
    scope_json TEXT,
    connected_on_upgrade INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
