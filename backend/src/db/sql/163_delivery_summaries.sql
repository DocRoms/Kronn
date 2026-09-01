-- KT-544 — accepted delivery summaries.
--
-- One row per (execution, attempt): the canonical JSON Kronn validated, plus
-- the id of the single message it rendered into the discussion. The primary key
-- IS the idempotency guard — a replayed approval, a double click or a restart
-- mid-publication finds the row and republishes nothing.
CREATE TABLE IF NOT EXISTS delivery_summaries (
    execution_id    TEXT NOT NULL,
    attempt_no      INTEGER NOT NULL,
    -- Canonical JSON of DeliverySummaryV1, kept verbatim for audit: the
    -- rendered Markdown is a projection and must never be the record.
    canonical_json  TEXT NOT NULL,
    -- The message the summary was rendered into. Deterministic id, so the
    -- publication is idempotent even if this row lands before the message.
    message_id      TEXT NOT NULL,
    -- Discussion the summary was published in (the parent, not the worker's).
    discussion_id   TEXT NOT NULL,
    correlation_id  TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (execution_id, attempt_no)
);

CREATE INDEX IF NOT EXISTS idx_delivery_summaries_discussion
    ON delivery_summaries(discussion_id);
CREATE INDEX IF NOT EXISTS idx_delivery_summaries_correlation
    ON delivery_summaries(correlation_id);
