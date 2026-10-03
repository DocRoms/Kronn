-- KT-894 — the counters a reply's cost was computed from.
--
-- `messages.tokens_used` is a TOTAL. For Codex it includes the cache reads
-- (98.6% of one 25M-token run), so a total alone cannot say what a reply
-- really consumed and was billed as if all of it were fresh input. One row per
-- agent reply that reported its usage, kept in a side table so the message
-- rows, their tombstones and their copies stay untouched.
--
-- Every counter is NULL when NOT REPORTED, never 0. `input_tokens` is the input
-- that was neither served from cache nor written to it (a provider that folds
-- the cached share into its input total is normalised on the way in), so the
-- four counters never overlap. `cost_unknown_reason` says why `messages.cost_usd`
-- is NULL when a reply consumed tokens.
CREATE TABLE message_usage (
    message_id TEXT PRIMARY KEY
        REFERENCES messages(id) ON DELETE CASCADE,
    input_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    output_tokens INTEGER,
    cost_unknown_reason TEXT
);
