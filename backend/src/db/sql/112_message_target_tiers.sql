-- Per-turn model routing for native @mentions. NULL keeps the historical
-- discussion-wide tier, while explicit values let one user turn mix agents.
ALTER TABLE message_targets
ADD COLUMN model_tier TEXT
    CHECK (model_tier IS NULL OR model_tier IN ('economy', 'default', 'reasoning'));
