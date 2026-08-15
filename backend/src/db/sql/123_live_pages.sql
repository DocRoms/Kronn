-- v0.10.0 — Live Pages: versioned HTML plus workflow-linked JSON datasets.

CREATE TABLE IF NOT EXISTS live_pages (
    id                    TEXT PRIMARY KEY,
    project_id            TEXT REFERENCES projects(id) ON DELETE SET NULL,
    title                 TEXT NOT NULL,
    slug                  TEXT NOT NULL UNIQUE,
    current_revision_id   TEXT,
    data_revision         INTEGER NOT NULL DEFAULT 0,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    last_published_at     TEXT
);

CREATE TABLE IF NOT EXISTS live_page_revisions (
    id                    TEXT PRIMARY KEY,
    page_id               TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    revision              INTEGER NOT NULL,
    html                  TEXT NOT NULL,
    created_by_agent      TEXT,
    created_at            TEXT NOT NULL,
    UNIQUE(page_id, revision)
);

CREATE TABLE IF NOT EXISTS live_page_datasets (
    id                    TEXT PRIMARY KEY,
    page_id               TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    name                  TEXT NOT NULL,
    kind                  TEXT NOT NULL CHECK(kind IN ('snapshot', 'time_series', 'collection')),
    current_json          TEXT,
    schema_json           TEXT,
    max_points            INTEGER NOT NULL DEFAULT 50000 CHECK(max_points > 0),
    max_age_days          INTEGER CHECK(max_age_days IS NULL OR max_age_days > 0),
    updated_at            TEXT NOT NULL,
    UNIQUE(page_id, name)
);

CREATE TABLE IF NOT EXISTS live_page_dataset_points (
    id                    TEXT PRIMARY KEY,
    dataset_id            TEXT NOT NULL REFERENCES live_page_datasets(id) ON DELETE CASCADE,
    observed_at           TEXT NOT NULL,
    payload_json          TEXT NOT NULL,
    dedupe_key            TEXT,
    workflow_run_id       TEXT REFERENCES workflow_runs(id) ON DELETE SET NULL,
    created_at            TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_live_page_points_dedupe
    ON live_page_dataset_points(dataset_id, dedupe_key)
    WHERE dedupe_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_live_page_points_observed
    ON live_page_dataset_points(dataset_id, observed_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS live_page_publications (
    id                    TEXT PRIMARY KEY,
    page_id               TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    data_revision         INTEGER NOT NULL,
    workflow_id           TEXT REFERENCES workflows(id) ON DELETE SET NULL,
    workflow_run_id       TEXT REFERENCES workflow_runs(id) ON DELETE SET NULL,
    datasets_json         TEXT NOT NULL,
    points_added          INTEGER NOT NULL DEFAULT 0,
    points_removed        INTEGER NOT NULL DEFAULT 0,
    published_at          TEXT NOT NULL,
    UNIQUE(page_id, data_revision)
);

CREATE INDEX IF NOT EXISTS idx_live_page_publications_page
    ON live_page_publications(page_id, data_revision DESC);

-- A durable activation marker keeps the navigation visible after the first
-- Page is deleted. It is intentionally not derived from COUNT(live_pages).
CREATE TABLE IF NOT EXISTS live_pages_capability (
    singleton             INTEGER PRIMARY KEY CHECK(singleton = 1),
    activated_at          TEXT NOT NULL
);
