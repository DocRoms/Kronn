-- KT-190 — native token counters for a JOINED CLI session.
--
-- Kronn knows what the agents it spawns cost. A CLI that joined a room on its
-- own was never spawned, so `messages.tokens_used` is 0 for everything it
-- posts. Measured on one real Claude Code session: 4 143 787 451 tokens of
-- traffic recorded as zero.
--
-- NULL MEANS NOT MEASURED. Never write 0 to mean "this vendor does not report
-- it". Vibe publishes no cache breakdown at all; storing 0 there would let a
-- dashboard state that Vibe performs no cache reads — an assertion about a
-- field nobody measured. A real zero and an absent counter must stay tellable
-- apart, which is what nullable columns buy.
--
-- The counters are kept APART on purpose. Cache reads were 98.4% of the traffic
-- on that session and are billed at roughly a tenth of input, so traffic and
-- billable differ by a factor of ~62. Any schema that pre-summed them would
-- make both numbers unrecoverable.
--
-- One row per CLI session, upserted as the collector advances. `read_offset` is
-- the byte cursor into an append-only transcript (Claude Code); it stays 0 for
-- a vendor whose source is a rewritten snapshot (Vibe), where an offset would
-- be meaningless.

CREATE TABLE IF NOT EXISTS cli_session_telemetry (
    cli_session_pk        INTEGER PRIMARY KEY
                          REFERENCES discussion_sessions(id) ON DELETE CASCADE,
    vendor                TEXT    NOT NULL,
    -- Where the numbers came from, e.g. `claude-code-transcript`. Explicit so a
    -- consumer can tell a vendor counter from an estimate; nothing in this
    -- table is ever an estimate.
    provenance            TEXT    NOT NULL,
    input_tokens          INTEGER,
    cache_creation_tokens INTEGER,
    cache_read_tokens     INTEGER,
    output_tokens         INTEGER,
    measured_responses    INTEGER,
    models_json           TEXT,
    window_start          TEXT,
    window_end            TEXT,
    -- The VENDOR's own cost figure when it publishes one (Vibe does, Claude
    -- Code does not). Never mixed with a Kronn estimate: two numbers that can
    -- each be checked beat one that cannot.
    vendor_cost_usd       REAL,
    read_offset           INTEGER NOT NULL DEFAULT 0,
    updated_at            TEXT    NOT NULL
);

-- Coverage is a per-vendor question ("how many ClaudeCode sessions are
-- attributed?"), so it is the access path worth indexing.
CREATE INDEX IF NOT EXISTS idx_cli_session_telemetry_vendor
    ON cli_session_telemetry(vendor);
