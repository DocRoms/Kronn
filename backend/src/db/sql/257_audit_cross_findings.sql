-- KT-1183: cross-agent audit (experimental). One row per cross-checked step:
-- the agents, the findings dropped by the citation check, and what the
-- step's model calls cost.
CREATE TABLE audit_cross_steps (
    audit_run_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    file_label TEXT NOT NULL,
    agents_json TEXT NOT NULL,
    dropped_json TEXT NOT NULL,
    failed_agents_json TEXT NOT NULL,
    calls INTEGER NOT NULL,
    step_tokens INTEGER,
    cost_usd_micros INTEGER,
    dispute_discussion_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (audit_run_id, step_index),
    FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
);

-- The provenance of every finding after the cross-check, dismissed ones
-- included: who found it, who checked it, with what proof, the verdict.
CREATE TABLE audit_cross_findings (
    audit_run_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    group_id TEXT NOT NULL,
    td_id TEXT,
    outcome TEXT NOT NULL,
    detection_count INTEGER NOT NULL,
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (audit_run_id, step_index, group_id),
    FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
);
