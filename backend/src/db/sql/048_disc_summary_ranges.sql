-- Ranged summary cache for the on-demand `disc_summarize` tool.
--
-- Pre-fix the cache only stored ONE prefix-summary per discussion
-- (`discussions.summary_cache` keyed implicitly by `summary_up_to_msg_idx`).
-- The on-demand tool can request arbitrary ranges (e.g. `summarize(5, 15)`),
-- so we need a multi-entry store keyed by `(from, to)`.
--
-- Compound primary key on (disc_id, from_idx, to_idx) makes lookups O(1)
-- via the implicit index. ON DELETE CASCADE keeps the table clean when
-- a discussion is removed.

CREATE TABLE IF NOT EXISTS disc_summary_ranges (
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    from_idx INTEGER NOT NULL,
    to_idx INTEGER NOT NULL,
    summary TEXT NOT NULL,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    model_name TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (discussion_id, from_idx, to_idx)
);

CREATE INDEX IF NOT EXISTS idx_disc_summary_ranges_disc
    ON disc_summary_ranges(discussion_id);
