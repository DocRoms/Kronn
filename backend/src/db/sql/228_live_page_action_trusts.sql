-- KT-1029: a human's approval to run one Page action without its card. Bound
-- to a fingerprint of what was approved; a mismatch invalidates it for good.
CREATE TABLE IF NOT EXISTS live_page_action_trusts (
    action_id           TEXT PRIMARY KEY REFERENCES live_page_actions(id) ON DELETE CASCADE,
    live_page_id        TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    action_ref          TEXT NOT NULL,
    project_id          TEXT,
    target_id           TEXT NOT NULL,
    fingerprint         TEXT NOT NULL,
    -- New on every approval: a launch claimed under an older one never runs.
    approval_id         TEXT NOT NULL,
    approved_at         TEXT NOT NULL,
    invalidated_at      TEXT,
    invalidated_reason  TEXT
);

CREATE INDEX IF NOT EXISTS idx_live_page_action_trusts_page
ON live_page_action_trusts(live_page_id);

-- The stored resources an approval executes; any write to one invalidates it.
CREATE TABLE IF NOT EXISTS live_page_action_trust_deps (
    action_id  TEXT NOT NULL REFERENCES live_page_action_trusts(action_id) ON DELETE CASCADE,
    dep_kind   TEXT NOT NULL CHECK(dep_kind IN ('workflow','quick_api')),
    dep_id     TEXT NOT NULL,
    PRIMARY KEY (action_id, dep_kind, dep_id)
);

CREATE INDEX IF NOT EXISTS idx_live_page_action_trust_deps_dep
ON live_page_action_trust_deps(dep_kind, dep_id);

-- Only an update changing nothing but `pinned` or `updated_at` is exempt; a pin
-- change in the same statement as a content change still counts. A test keeps
-- these lists equal to the tables' columns.
CREATE TRIGGER IF NOT EXISTS trg_trust_workflow_update AFTER UPDATE ON workflows
WHEN NOT (
    OLD.id IS NEW.id
    AND OLD.name IS NEW.name
    AND OLD.project_id IS NEW.project_id
    AND OLD.trigger_json IS NEW.trigger_json
    AND OLD.steps_json IS NEW.steps_json
    AND OLD.actions_json IS NEW.actions_json
    AND OLD.safety_json IS NEW.safety_json
    AND OLD.workspace_config_json IS NEW.workspace_config_json
    AND OLD.concurrency_limit IS NEW.concurrency_limit
    AND OLD.enabled IS NEW.enabled
    AND OLD.created_at IS NEW.created_at
    AND OLD.guards IS NEW.guards
    AND OLD.artifacts IS NEW.artifacts
    AND OLD.on_failure IS NEW.on_failure
    AND OLD.exec_allowlist IS NEW.exec_allowlist
    AND OLD.variables IS NEW.variables
    AND OLD.concurrency_key IS NEW.concurrency_key
    AND OLD.project_scope_json IS NEW.project_scope_json
    AND OLD.disabled_reason IS NEW.disabled_reason
    AND OLD.disabled_at IS NEW.disabled_at
    AND OLD.disabled_by IS NEW.disabled_by
    AND OLD.disabled_summary IS NEW.disabled_summary)
BEGIN
    UPDATE live_page_action_trusts
    SET invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        invalidated_reason = CASE WHEN NEW.enabled = 0 THEN 'workflow_disabled' ELSE 'changed' END
    WHERE invalidated_at IS NULL AND action_id IN (
        SELECT action_id FROM live_page_action_trust_deps
        WHERE dep_kind = 'workflow' AND dep_id = OLD.id);
END;

CREATE TRIGGER IF NOT EXISTS trg_trust_workflow_delete AFTER DELETE ON workflows
BEGIN
    UPDATE live_page_action_trusts
    SET invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), invalidated_reason = 'target_missing'
    WHERE invalidated_at IS NULL AND action_id IN (
        SELECT action_id FROM live_page_action_trust_deps
        WHERE dep_kind = 'workflow' AND dep_id = OLD.id);
END;

CREATE TRIGGER IF NOT EXISTS trg_trust_quick_api_update AFTER UPDATE ON quick_apis
WHEN NOT (
    OLD.id IS NEW.id
    AND OLD.name IS NEW.name
    AND OLD.description IS NEW.description
    AND OLD.icon IS NEW.icon
    AND OLD.project_id IS NEW.project_id
    AND OLD.api_plugin_slug IS NEW.api_plugin_slug
    AND OLD.api_config_id IS NEW.api_config_id
    AND OLD.api_endpoint_path IS NEW.api_endpoint_path
    AND OLD.api_method IS NEW.api_method
    AND OLD.api_query_json IS NEW.api_query_json
    AND OLD.api_path_params_json IS NEW.api_path_params_json
    AND OLD.api_headers_json IS NEW.api_headers_json
    AND OLD.api_body IS NEW.api_body
    AND OLD.api_extract_json IS NEW.api_extract_json
    AND OLD.api_pagination_json IS NEW.api_pagination_json
    AND OLD.api_timeout_ms IS NEW.api_timeout_ms
    AND OLD.api_max_retries IS NEW.api_max_retries
    AND OLD.variables_json IS NEW.variables_json
    AND OLD.created_at IS NEW.created_at
    AND OLD.profile_ids_json IS NEW.profile_ids_json
    AND OLD.directive_ids_json IS NEW.directive_ids_json)
BEGIN
    UPDATE live_page_action_trusts
    SET invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), invalidated_reason = 'changed'
    WHERE invalidated_at IS NULL AND action_id IN (
        SELECT action_id FROM live_page_action_trust_deps
        WHERE dep_kind = 'quick_api' AND dep_id = OLD.id);
END;

CREATE TRIGGER IF NOT EXISTS trg_trust_quick_api_delete AFTER DELETE ON quick_apis
BEGIN
    UPDATE live_page_action_trusts
    SET invalidated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), invalidated_reason = 'target_missing'
    WHERE invalidated_at IS NULL AND action_id IN (
        SELECT action_id FROM live_page_action_trust_deps
        WHERE dep_kind = 'quick_api' AND dep_id = OLD.id);
END;

-- Launches started by a trust, not by the card, with the approval they claimed.
ALTER TABLE live_page_action_launches ADD COLUMN trusted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE live_page_action_launches ADD COLUMN trust_approval_id TEXT;
ALTER TABLE live_page_action_launches ADD COLUMN trust_fingerprint TEXT;
