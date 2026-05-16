-- 0.8.5 — Quick Prompt version history + discussion lineage.
--
-- Two parts of the same feature:
--
-- 1. `quick_prompt_versions` is a full-snapshot append-only table.
--    Every `INSERT` and `UPDATE` on `quick_prompts` writes a new row
--    here so the QP history drawer can render the timeline and the
--    metrics aggregator can group launches by version.
--    `version_index` is per-QP (1-based): v1 = initial state at
--    insertion, v2 = first update, etc. `(quick_prompt_id, version_index)`
--    is UNIQUE; the version counter increments via
--    `SELECT COALESCE(MAX(version_index), 0) + 1 FROM ... WHERE qp_id = ?`.
--
-- 2. `discussions.originating_qp_id` + `discussions.originating_qp_version`
--    are set when the QP launch path spawns a discussion. The metrics
--    aggregator joins on these columns to compute `avg(duration_ms)` and
--    `avg(tokens_used)` of the FIRST agent reply per (qp, version) pair.
--    NULL for non-QP-launched discussions (briefing / validation / manual
--    new disc / batch from non-QP source).
--
-- Indexes:
--   - quick_prompt_versions(quick_prompt_id, version_index) UNIQUE
--   - discussions(originating_qp_id, originating_qp_version) — speeds up
--     the metrics aggregator's GROUP BY when a QP has hundreds of launches.

CREATE TABLE IF NOT EXISTS quick_prompt_versions (
    id                  TEXT PRIMARY KEY,
    quick_prompt_id     TEXT NOT NULL,
    version_index       INTEGER NOT NULL,
    name                TEXT NOT NULL,
    icon                TEXT NOT NULL DEFAULT '⚡',
    prompt_template     TEXT NOT NULL,
    variables_json      TEXT NOT NULL DEFAULT '[]',
    agent               TEXT NOT NULL DEFAULT 'ClaudeCode',
    project_id          TEXT,
    skill_ids_json      TEXT NOT NULL DEFAULT '[]',
    profile_ids_json    TEXT NOT NULL DEFAULT '[]',
    directive_ids_json  TEXT NOT NULL DEFAULT '[]',
    tier                TEXT NOT NULL DEFAULT 'default',
    description         TEXT NOT NULL DEFAULT '',
    created_at          TEXT NOT NULL,
    UNIQUE(quick_prompt_id, version_index)
);

CREATE INDEX IF NOT EXISTS idx_quick_prompt_versions_qp
    ON quick_prompt_versions(quick_prompt_id);

ALTER TABLE discussions ADD COLUMN originating_qp_id TEXT;
ALTER TABLE discussions ADD COLUMN originating_qp_version INTEGER;

CREATE INDEX IF NOT EXISTS idx_discussions_originating_qp
    ON discussions(originating_qp_id, originating_qp_version);
