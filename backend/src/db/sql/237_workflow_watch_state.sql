-- KT-1099: what a Watch trigger last saw of its source and how its polls
-- went. A poll writes here, never to workflow_runs; only a change creates a
-- run. `source_key` is the hash of the watched source: when it changes, the
-- stored validators and fingerprint no longer apply.
CREATE TABLE IF NOT EXISTS workflow_watch_state (
    workflow_id          TEXT PRIMARY KEY REFERENCES workflows(id) ON DELETE CASCADE,
    source_key           TEXT NOT NULL,
    etag                 TEXT,
    last_modified        TEXT,
    fingerprint          TEXT,
    last_poll_at         TEXT,
    last_result          TEXT,
    last_http_status     INTEGER,
    last_error           TEXT,
    last_change_at       TEXT,
    unchanged_count      INTEGER NOT NULL DEFAULT 0,
    changed_count        INTEGER NOT NULL DEFAULT 0,
    error_count          INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    -- Bumped each time the baseline advances: a change is identified by the
    -- baseline it departs from plus the state it reaches.
    baseline_seq         INTEGER NOT NULL DEFAULT 0
);

-- One row per project a detected change was admitted for, written in the
-- transaction that inserts its run: a change detected again (crash before
-- the baseline advanced, another project still refused) never runs twice
-- for the same project. No foreign key to workflow_runs: run retention must
-- not make an admitted change look new.
CREATE TABLE IF NOT EXISTS workflow_watch_occurrences (
    workflow_id  TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    baseline_seq INTEGER NOT NULL,
    state_key    TEXT NOT NULL,
    project_key  TEXT NOT NULL,
    run_id       TEXT NOT NULL,
    admitted_at  TEXT NOT NULL,
    PRIMARY KEY (workflow_id, baseline_seq, state_key, project_key)
);
