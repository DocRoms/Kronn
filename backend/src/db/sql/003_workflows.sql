-- ═══════════════════════════════════════════════════════════════════════════════
-- Kronn — Workflow engine (replaces legacy tasks)
-- ═══════════════════════════════════════════════════════════════════════════════

-- Workflows
CREATE TABLE IF NOT EXISTS workflows (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    project_id          TEXT REFERENCES projects(id) ON DELETE SET NULL,
    trigger_json        TEXT NOT NULL,           -- JSON: WorkflowTrigger
    steps_json          TEXT NOT NULL,           -- JSON: Vec<WorkflowStep>
    actions_json        TEXT NOT NULL DEFAULT '[]',  -- JSON: Vec<WorkflowAction>
    safety_json         TEXT NOT NULL DEFAULT '{}',  -- JSON: WorkflowSafety
    workspace_config_json TEXT,                  -- JSON: WorkspaceConfig or null
    concurrency_limit   INTEGER,
    enabled             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workflows_project ON workflows(project_id);
CREATE INDEX IF NOT EXISTS idx_workflows_enabled ON workflows(enabled);

-- Workflow runs
CREATE TABLE IF NOT EXISTS workflow_runs (
    id                  TEXT PRIMARY KEY,
    workflow_id         TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    status              TEXT NOT NULL DEFAULT 'Pending',  -- Pending, Running, Success, Failed, Cancelled, WaitingApproval
    trigger_context     TEXT,                    -- JSON: issue data, cron tick, etc.
    step_results_json   TEXT NOT NULL DEFAULT '[]',  -- JSON: Vec<StepResult>
    tokens_used         INTEGER NOT NULL DEFAULT 0,
    workspace_path      TEXT,
    started_at          TEXT NOT NULL,
    finished_at         TEXT
);

CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow ON workflow_runs(workflow_id);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_status ON workflow_runs(status);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_started ON workflow_runs(started_at DESC);

-- Tracker reconciliation: track processed issue IDs to avoid duplicate runs
CREATE TABLE IF NOT EXISTS workflow_tracker_processed (
    workflow_id         TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    issue_id            TEXT NOT NULL,           -- external ID (e.g. GitHub issue number)
    processed_at        TEXT NOT NULL,
    PRIMARY KEY (workflow_id, issue_id)
);
