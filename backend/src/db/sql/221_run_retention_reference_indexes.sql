-- KT-1048 — every retention chunk materialises `id NOT IN (SELECT col ...)`
-- for each column of run_retention::REFERENCING_COLUMNS. These three had no
-- index, so each chunk read their tables in full on the write connection.
-- Partial: only rows that point at a run are listed, as the subqueries ask.
CREATE INDEX IF NOT EXISTS idx_live_page_dataset_points_run
    ON live_page_dataset_points(workflow_run_id) WHERE workflow_run_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_live_page_publications_run
    ON live_page_publications(workflow_run_id) WHERE workflow_run_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_discussion_questions_resume_run
    ON discussion_questions(resume_run_id) WHERE resume_run_id IS NOT NULL;
