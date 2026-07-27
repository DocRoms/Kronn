-- 0.9.2 (KT-116) — preserve every explicitly addressed agent, in text order.
--
-- This is the original plural-AgentType schema. Migration 099 upgrades it to
-- durable typed identities. Keeping 098 stable matters for developer/user
-- databases that applied it before the typed-target refinement was completed.
CREATE TABLE message_targets (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    agent_type TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (message_id, position)
);

CREATE INDEX idx_message_targets_agent
ON message_targets(agent_type, message_id);

INSERT INTO message_targets (message_id, agent_type, position)
SELECT id, target_agent, 0
FROM messages
WHERE target_agent IS NOT NULL;
