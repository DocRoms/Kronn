-- Preserve the exact named connection selected by a dynamic mention alias.
ALTER TABLE message_targets
    ADD COLUMN connection_id TEXT
        REFERENCES external_api_connections(id);

DROP INDEX idx_message_targets_identity;

CREATE UNIQUE INDEX idx_message_targets_identity
ON message_targets(
    message_id,
    target_kind,
    agent_type,
    COALESCE(connection_id, ''),
    COALESCE(cli_session_id, -1)
);
