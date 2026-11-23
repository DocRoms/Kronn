-- KT-1990 — the Artifact a person made from one execution of a step (its main
-- execution or a shadow), so the run view opens it again instead of making a
-- duplicate, and the Artifact library groups a run's renders in one folder.
--
-- One link per Artifact (`page_id`) and one per execution
-- (`run_id`, `result_index`, `shadow_id`; `shadow_id` = '' is the main
-- execution, as in `step_shadow_runs`). Deleting the Artifact drops its link,
-- and the run view offers the button again. Deleting the run drops the link
-- too; the run delete routes delete the Artifacts first, in the same
-- transaction. A run named here is never trimmed by retention
-- (`run_retention::REFERENCING_COLUMNS`).
CREATE TABLE IF NOT EXISTS step_execution_artifacts (
    page_id      TEXT PRIMARY KEY REFERENCES live_pages(id) ON DELETE CASCADE,
    run_id       TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    result_index INTEGER NOT NULL,
    step_name    TEXT NOT NULL,
    shadow_id    TEXT NOT NULL,
    -- What the library shows of the execution: its label and the model that
    -- answered, as they were when the Artifact was made.
    label        TEXT NOT NULL DEFAULT '',
    model        TEXT,
    created_at   TEXT NOT NULL,
    UNIQUE (run_id, result_index, shadow_id)
);
