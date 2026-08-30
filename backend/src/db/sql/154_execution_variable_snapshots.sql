-- KT-537: secret values are encrypted as one run-scoped payload. Metadata is
-- intentionally separate so normal reads never load ciphertext.
CREATE TABLE execution_variable_snapshots (
    id TEXT PRIMARY KEY,
    run_kind TEXT NOT NULL,
    run_id TEXT NOT NULL,
    project_id TEXT,
    environment_ref TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    expires_at TEXT,
    values_encrypted TEXT,
    fingerprint TEXT NOT NULL,
    provenance_json TEXT NOT NULL,
    purged_at TEXT,
    UNIQUE(run_kind, run_id)
);
CREATE INDEX idx_execution_variable_snapshots_expiry
    ON execution_variable_snapshots(expires_at) WHERE values_encrypted IS NOT NULL;

CREATE TABLE execution_variable_reveal_audit (
    id TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL REFERENCES execution_variable_snapshots(id) ON DELETE CASCADE,
    variable_name TEXT NOT NULL,
    actor TEXT NOT NULL,
    revealed_at TEXT NOT NULL
);

ALTER TABLE discussions ADD COLUMN execution_variable_retention_days INTEGER;
