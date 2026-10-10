-- KT-1096: the definitions a workflow run executes, frozen when it starts. The
-- 'run' row holds the run's own workflow; the others its Quick Prompts, Quick
-- APIs, sub-workflows, skills, directives and profiles. Resumes, children and
-- fan-out read these rows, never the live definitions.
CREATE TABLE IF NOT EXISTS workflow_run_pins (
    run_id       TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    resource_id  TEXT NOT NULL,
    content_json TEXT NOT NULL,
    PRIMARY KEY (run_id, kind, resource_id)
);
