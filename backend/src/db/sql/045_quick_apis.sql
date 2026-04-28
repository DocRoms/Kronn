-- Quick APIs: reusable API call templates with {{variables}}.
-- Mirror of `quick_prompts` but the moteur d'exécution is HTTP, not LLM —
-- zero tokens consumed, fan-out parallel via the `BatchApiCall` step type.
--
-- Field naming follows the `WorkflowStep` ApiCall fields verbatim so the
-- frontend can reuse `ApiCallStepCard` (and therefore `ApiCallAiHelper`)
-- as the editor without remapping. JSON columns store the same shapes as
-- the corresponding step fields.
CREATE TABLE IF NOT EXISTS quick_apis (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    description         TEXT,
    icon                TEXT NOT NULL DEFAULT '🔌',
    project_id          TEXT REFERENCES projects(id) ON DELETE SET NULL,

    -- API request shape (same field names as WorkflowStep ApiCall fields).
    api_plugin_slug     TEXT NOT NULL,
    api_config_id       TEXT NOT NULL,
    api_endpoint_path   TEXT NOT NULL,
    api_method          TEXT,
    api_query_json      TEXT,           -- HashMap<String, String> as JSON
    api_path_params_json TEXT,          -- HashMap<String, String> as JSON
    api_headers_json    TEXT,           -- HashMap<String, String> as JSON
    api_body            TEXT,
    api_extract_json    TEXT,           -- ExtractSpec as JSON
    api_pagination_json TEXT,           -- PaginationSpec as JSON
    api_timeout_ms      INTEGER,
    api_max_retries     INTEGER,

    -- Variables prompted at run-time (same shape as Workflow.variables /
    -- QuickPrompt.variables). Stored as a JSON array of PromptVariable.
    variables_json      TEXT NOT NULL DEFAULT '[]',

    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_quick_apis_project ON quick_apis(project_id);
