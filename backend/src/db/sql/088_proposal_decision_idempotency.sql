-- 0.9.2-H — decision idempotency + receipt columns for planning proposal items.
--
-- These MUST live in their own migration (not folded into 087): 087 was already
-- applied on dev databases before these columns existed, and the runner never
-- re-applies an already-recorded migration. Adding them here makes a dev DB
-- (087-only) and a fresh 086→087→088 upgrade converge on the same schema.

-- The decision's idempotency key. A retry with the SAME key returns the same
-- result/receipt; a DIFFERENT key on an already-terminal item is a conflict.
ALTER TABLE planning_proposal_items ADD COLUMN decision_idempotency_key TEXT;

-- The `[kronn-planning: …]` System receipt emitted for this decision.
ALTER TABLE planning_proposal_items ADD COLUMN receipt_message_id TEXT;

-- One decision per idempotency key: a replay maps to the same row.
CREATE UNIQUE INDEX IF NOT EXISTS idx_planning_item_idempotency
    ON planning_proposal_items(decision_idempotency_key)
    WHERE decision_idempotency_key IS NOT NULL;
