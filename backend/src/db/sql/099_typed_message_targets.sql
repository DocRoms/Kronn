-- 0.9.2 (KT-116) — route to identities, not provider names.
--
-- A configured discussion agent, a punctual native agent and a joined CLI can
-- all be Codex (or Claude, Vibe, …) at the same time. Rebuild the 098 child
-- table so the durable contract identifies which one owns each reply.
ALTER TABLE message_targets RENAME TO message_targets_agent_only;

DROP INDEX IF EXISTS idx_message_targets_agent;

CREATE TABLE message_targets (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    target_kind TEXT NOT NULL
        CHECK (target_kind IN ('discussion_agent', 'agent', 'cli')),
    agent_type TEXT NOT NULL,
    cli_session_id INTEGER REFERENCES discussion_sessions(id),
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (message_id, position)
);

INSERT INTO message_targets (
    message_id, target_kind, agent_type, cli_session_id, position
)
SELECT message_id, 'agent', agent_type, NULL, position
FROM message_targets_agent_only;

DROP TABLE message_targets_agent_only;

CREATE UNIQUE INDEX idx_message_targets_identity
ON message_targets(
    message_id,
    target_kind,
    agent_type,
    COALESCE(cli_session_id, -1)
);

CREATE INDEX idx_message_targets_agent
ON message_targets(agent_type, target_kind, message_id);
