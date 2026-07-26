-- 0.9.2-H — durable Planning proposals + itemized, human-gated validation.
--
-- A `kronn-plan-action` fence in an Agent message is parsed and persisted AT
-- INSERT TIME (same transaction as the message), so a proposal exists in the
-- inbox even if nobody opened the message. IDs are deterministic from the
-- source message + fence/item position, so re-ingesting the same message
-- (bulk import, replay) is idempotent and never duplicates.
--
-- Agents PROPOSE; only a human DECIDES (accept/reject per item). Acceptance
-- applies the underlying task mutation + records the result, idempotently.

CREATE TABLE IF NOT EXISTS planning_proposals (
    -- Deterministic: `proposal:<source_message_id>:<fence_index>`.
    id                TEXT PRIMARY KEY,
    discussion_id     TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    -- Position of this fence within the source message (0-based).
    fence_index       INTEGER NOT NULL,
    -- Aggregate over item states: pending = all pending; partial = a mix of
    -- pending and terminal; dismissed = all rejected; applied = no pending and
    -- at least one accepted.
    aggregate_state   TEXT NOT NULL DEFAULT 'pending'
        CHECK (aggregate_state IN ('pending', 'partial', 'applied', 'dismissed')),
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    UNIQUE (source_message_id, fence_index)
);

CREATE TABLE IF NOT EXISTS planning_proposal_items (
    -- Deterministic: `<proposal_id>:<item_index>`.
    id                TEXT PRIMARY KEY,
    proposal_id       TEXT NOT NULL REFERENCES planning_proposals(id) ON DELETE CASCADE,
    item_index        INTEGER NOT NULL,
    -- The mutation this item requests. `open` is a local navigation, never an
    -- inbox item, so it is not persisted here.
    action            TEXT NOT NULL
        CHECK (action IN ('create', 'status', 'complete', 'unblock')),
    -- The item's fields as proposed (title/description/priority/placement for
    -- create; task_id/status for the mutations).
    payload_json      TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'accepted', 'rejected')),
    rejected_reason   TEXT,
    -- The task created/updated on acceptance — the idempotent result: a repeat
    -- accept with the same key returns this instead of applying twice.
    result_task_id    TEXT,
    decided_at        TEXT,
    UNIQUE (proposal_id, item_index)
);

-- NOTE: the decision idempotency key + receipt columns are added in migration
-- 088, NOT here. This file was already applied on dev DBs before those columns
-- existed; editing an applied migration is silently skipped by the runner. 088
-- ALTERs them in so a dev DB (087-only) and a fresh 086→087→088 upgrade converge.

CREATE INDEX IF NOT EXISTS idx_planning_proposals_disc
    ON planning_proposals(discussion_id, aggregate_state);
CREATE INDEX IF NOT EXISTS idx_planning_proposal_items_proposal
    ON planning_proposal_items(proposal_id, item_index);
