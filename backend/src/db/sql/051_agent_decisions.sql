-- 0.8.3 — Feasibility-Gated Implementation: agent decisions log.
-- One row per non-trivial choice the triage agent surfaces. The
-- existence of this table is what makes the "auto-approve" mode safe:
-- skipping the human Gate is OK only if every decision the agent made
-- ends up queryable, with provenance (run, step, ticket, agent) and
-- the rationale the agent gave.
--
-- Three categories track the four manifest buckets (clear entries
-- are NOT logged here — they're trivial by definition and would
-- flood the table with noise):
--   - 'decided' : agent had multiple viable options, picked one
--   - 'mocked'  : value/integration faked, real one missing
--   - 'blocked' : cannot proceed, external dependency
--
-- `gate_status` evolves over time: most rows start `auto_approved` or
-- `human_approved`, can flip to `overridden` if the operator changes
-- the choice at the Gate, or `resolved` later when the mock/block is
-- filled in. The drift detector (PR5) cross-references the markers in
-- the code with these rows to detect undeclared decisions.

CREATE TABLE agent_decisions (
    id TEXT PRIMARY KEY,                -- UUID
    run_id TEXT NOT NULL,               -- workflow_runs.id
    step_name TEXT NOT NULL,            -- which step emitted this row
    workflow_id TEXT NOT NULL,          -- workflows.id
    project_id TEXT,                    -- nullable for project-less workflows

    -- Ticket reference, extracted from `{{issue.key}}` when present
    -- in the run's trigger_context. Null for manual runs without a
    -- tracker trigger.
    ticket_ref TEXT,

    -- Category. CHECK enforced at app level so we can add categories
    -- without a migration.
    category TEXT NOT NULL,             -- decided | mocked | blocked

    -- Stable, kebab-case identifier from the manifest entry. Repeats
    -- across runs of the same workflow when the agent re-encounters
    -- the same decision — the unique constraint is on the composite
    -- (run_id, decision_id) so a re-run rewrites its own rows.
    decision_id TEXT NOT NULL,

    -- The 'what' field — short human description of the sub-task.
    what TEXT NOT NULL,

    -- Category-specific fields. NULL where not applicable.
    -- decided:
    chosen TEXT,
    options_json TEXT,                  -- JSON array of strings
    why TEXT,
    -- mocked:
    placeholder TEXT,
    strategy TEXT,
    revisit_when TEXT,
    -- blocked:
    needed_from TEXT,
    workaround TEXT,

    -- Lifecycle. Values: auto_approved | human_approved | overridden | resolved | pending.
    gate_status TEXT NOT NULL DEFAULT 'pending',
    -- If the human overrode the agent's choice at the Gate, the new
    -- value is captured here. The implement step receives this
    -- override instead of the manifest's `chosen` field.
    override_value TEXT,

    -- JSON array of `"file:line"` strings where the corresponding
    -- KRONN-<MARKER>(<decision_id>) was found in the code. Populated
    -- by the drift detector (PR5). Empty until then.
    code_locations TEXT,

    created_at TEXT NOT NULL,           -- ISO-8601 UTC
    resolved_at TEXT,                   -- set when status flips to 'resolved'

    UNIQUE (run_id, decision_id)
);

-- Most common query: "show me all decisions for this project, newest
-- first". The compound index covers it without scanning.
CREATE INDEX idx_agent_decisions_project_time
    ON agent_decisions(project_id, created_at DESC);

-- "What's still open (mocked + blocked, not resolved)" — drives the
-- Decision log page's "Open mocks / blocks" counters.
CREATE INDEX idx_agent_decisions_open
    ON agent_decisions(category, gate_status)
    WHERE gate_status != 'resolved';

-- "All decisions in a single run" — used by the drift detector + the
-- ticket-level rollup.
CREATE INDEX idx_agent_decisions_run
    ON agent_decisions(run_id);
