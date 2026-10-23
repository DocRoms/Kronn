-- KT-1433: how far a project's integration validations reach beyond the lane
-- sandbox, set by a human: 'sandbox' (the default), 'docker' (the host's Docker
-- socket too) or 'full' (no sandbox at all).
ALTER TABLE projects ADD COLUMN validations_access TEXT NOT NULL DEFAULT 'sandbox';

-- KT-1433: a capability a human granted to one discussion's runs only, from
-- an elevation card ('validations_docker').
CREATE TABLE IF NOT EXISTS discussion_capability_grants (
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    capability TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    granted_at TEXT NOT NULL,
    PRIMARY KEY (discussion_id, capability)
);
