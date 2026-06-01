-- 0.8.2 — Track every audit run so the health badge, the reconciliation
-- pass, and the audit-history doc can read coherent metrics over time.
-- Previously we only kept `audit_status` (a 5-state enum) on the project
-- row, so the entire history of "what did we find, when, with which
-- agent, how long did it take" was lost between runs.
--
-- Insert at audit start (status='Running', ended_at + counts NULL) →
-- update at audit end (status='Completed' | 'Failed' | 'Cancelled',
-- counts populated). Older rows are kept indefinitely — they're tiny
-- and the trend is the value.

CREATE TABLE audit_runs (
    id TEXT PRIMARY KEY,             -- UUID
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,

    -- Type of audit. 'Full' is the existing 9-step generic pass; the
    -- specialized variants land in 0.8.2 S2 (Security/Docker/etc.) but
    -- the column is forward-compatible.
    kind TEXT NOT NULL DEFAULT 'Full',
    agent_type TEXT NOT NULL,        -- ClaudeCode | Codex | Kiro | ...

    started_at TEXT NOT NULL,        -- ISO-8601 UTC, set on insert
    ended_at TEXT,                   -- null while running
    duration_ms INTEGER,             -- null while running, fill at end

    -- Status: Running while in flight, terminal otherwise.
    status TEXT NOT NULL DEFAULT 'Running',
        -- CHECK on app side, not SQL — keeps migration forward-compat.

    -- Severity counts at the moment of completion (snapshot of the TD
    -- folder). Zero until the run completes. Used by the health-badge
    -- score formula.
    td_critical INTEGER NOT NULL DEFAULT 0,
    td_high     INTEGER NOT NULL DEFAULT 0,
    td_medium   INTEGER NOT NULL DEFAULT 0,
    td_low      INTEGER NOT NULL DEFAULT 0,
    td_total    INTEGER NOT NULL DEFAULT 0,

    -- Reconciliation outcome counts — surfaced in the ProjectCard
    -- "trend" chip and in `docs/audit-history.md`.
    td_resolved_since_last INTEGER NOT NULL DEFAULT 0,
    td_new_since_last      INTEGER NOT NULL DEFAULT 0,
    td_carried_over        INTEGER NOT NULL DEFAULT 0,

    -- Pre-computed health score (0-100). Cached so the dashboard can
    -- render N project badges without recomputing the formula in JS.
    health_score INTEGER,

    -- Path to the markdown report this audit produced
    -- (e.g. `docs/audit-history.md` entry index, or
    -- `docs/tech-debt/_reconciliation-2026-05-13.md`).
    report_path TEXT,

    -- JSON list of `{ kind, reason, cluster_size }` items emitted by
    -- Step 9's cluster detector. Read by the frontend to populate the
    -- "Audits recommandés" dropdown on the ProjectCard.
    recommendations_json TEXT
);

-- Most queries read "latest N runs for a project, newest first" — the
-- compound index covers that path exactly.
CREATE INDEX idx_audit_runs_project_time
    ON audit_runs(project_id, started_at DESC);

-- Sparse index on `kind` for the rarer "show me all Security audits
-- across all projects" query.
CREATE INDEX idx_audit_runs_kind
    ON audit_runs(kind, started_at DESC);
