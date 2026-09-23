-- Capture only new launches; never infer old settings from a mutable template.
CREATE TABLE discussion_http_settings (
    discussion_id TEXT PRIMARY KEY REFERENCES discussions(id) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    tier TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    model TEXT NOT NULL,
    reasoning_effort TEXT,
    max_tokens INTEGER CHECK (max_tokens > 0)
);
