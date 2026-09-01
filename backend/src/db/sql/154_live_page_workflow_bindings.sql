-- v0.12.0 — Live Page ↔ workflow bindings.
--
-- A binding lets a Page mirror a workflow run's live step state into one of its
-- datasets (read path, Phase 2) and declares which of that workflow's gates the
-- Page is allowed to decide (write path, Phase 3). The reshape/phase grouping is
-- carried as opaque JSON interpreted client-side; the backend is a typed store
-- plus the authorization boundary (a Page may only act on the bound workflow).

CREATE TABLE IF NOT EXISTS live_page_workflow_bindings (
    id                      TEXT PRIMARY KEY,
    page_id                 TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    workflow_id             TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    dataset                 TEXT NOT NULL,
    run_selector            TEXT NOT NULL DEFAULT 'latest'
                                CHECK(run_selector IN ('latest', 'latest_active')),
    phase_map_json          TEXT NOT NULL,
    meta_map_json           TEXT NOT NULL,
    allowed_gate_steps_json TEXT NOT NULL DEFAULT '[]',
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(page_id, dataset)
);

CREATE INDEX IF NOT EXISTS idx_live_page_bindings_page
    ON live_page_workflow_bindings(page_id);
