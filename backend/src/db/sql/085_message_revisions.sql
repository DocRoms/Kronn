-- Atomic edit/resend (0.9.2-D).
--
-- Trailing replies are moved out of the live `messages` projection rather
-- than destroyed. Their original ids and sort_order values remain available
-- for audit/debug while every existing message query keeps seeing only the
-- current projection.
CREATE TABLE IF NOT EXISTS message_tombstones (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    agent_type TEXT,
    timestamp TEXT NOT NULL,
    sort_order INTEGER NOT NULL,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    auth_mode TEXT,
    model_tier TEXT,
    cost_usd REAL,
    author_pseudo TEXT,
    author_avatar_email TEXT,
    source_msg_id TEXT,
    duration_ms INTEGER,
    lint_report TEXT,
    model TEXT,
    received_at TEXT,
    recovered_partial INTEGER NOT NULL DEFAULT 0,
    agent_run_succeeded INTEGER,
    agent_dispatch_job_id TEXT,
    revision_event_id TEXT NOT NULL,
    tombstoned_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_tombstones_discussion_order
ON message_tombstones(discussion_id, sort_order);

-- Revision events consume a fresh discussion sequence but are intentionally
-- separate from `messages`: long-poll clients observe them, while the normal
-- transcript projection does not render an extra System bubble.
CREATE TABLE IF NOT EXISTS message_revision_events (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    target_message_id TEXT NOT NULL,
    previous_content_hash TEXT NOT NULL,
    expected_revision TEXT NOT NULL,
    revision TEXT NOT NULL,
    content TEXT NOT NULL,
    target_agent_json TEXT,
    idempotency_key TEXT NOT NULL UNIQUE,
    sort_order INTEGER NOT NULL,
    dispatch_job_id TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(discussion_id, sort_order)
);

CREATE INDEX IF NOT EXISTS idx_message_revision_events_discussion_order
ON message_revision_events(discussion_id, sort_order);
