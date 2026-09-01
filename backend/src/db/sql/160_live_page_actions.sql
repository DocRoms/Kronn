-- Durable, human-gated Kronn action proposals authored inline in a Live
-- Page's HTML (KT-538). Mirrors 159_discussion_actions.sql's kind/state/value
-- contract exactly (see backend/src/db/kronn_action_engine.rs): the shared
-- launch state machine is table-agnostic, only the origin anchor differs.
--
-- Keyed by (live_page_id, action_ref) rather than a revision so the row
-- survives a later HTML edit: a still-`proposed` action is refreshed in place
-- when its block is republished, while an active/terminal action is frozen at
-- the revision it launched from. `live_page_revision_id` therefore records
-- "the last revision this row's definition matched", not "the current Page
-- revision" — the API layer compares it against the Page's live
-- `current_revision_id` to surface an explicit stale-source explanation.
CREATE TABLE live_page_actions (
    id TEXT PRIMARY KEY,
    live_page_id TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    live_page_revision_id TEXT NOT NULL REFERENCES live_page_revisions(id) ON DELETE CASCADE,
    action_ref TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('quick_prompt','quick_api','quick_exec','workflow','invalid')),
    target_id TEXT NOT NULL,
    target_name TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'proposed' CHECK(state IN ('proposed','launching','running','succeeded','failed','cancelled','preflight_failed')),
    values_json TEXT NOT NULL DEFAULT '[]',
    shared_run_id TEXT REFERENCES shared_runs(id) ON DELETE SET NULL,
    result_discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    deep_link TEXT,
    diagnostic TEXT,
    launched_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(live_page_id, action_ref)
);

CREATE INDEX idx_live_page_actions_page ON live_page_actions(live_page_id, created_at, id);
CREATE INDEX idx_live_page_actions_state ON live_page_actions(state, updated_at);
