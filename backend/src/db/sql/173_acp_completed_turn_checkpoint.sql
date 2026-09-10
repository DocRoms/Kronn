-- KT-621: an input frontier and the response emitted by that same turn are
-- different facts. Advancing to the response would lose peers that arrived
-- while the agent was working; retaining only the input replays its response.
-- Existing rows deliberately have no complete checkpoint (no inferred repair).
ALTER TABLE acp_runtime_sessions ADD COLUMN last_output_message_id TEXT;

-- Cleared only by an atomic durable reply/checkpoint commit. A crash, failed
-- prompt or late completion of an older turn cannot certify unseen messages.
ALTER TABLE acp_runtime_sessions ADD COLUMN active_turn_id TEXT;
