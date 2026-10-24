-- KT-1448: a workflow's named refs (`refs.<name>` → kind + id), as JSON.
-- What a ref names changes what the workflow runs, so the Page action trusts
-- built on it are invalidated like for any other content column (244).
ALTER TABLE workflows ADD COLUMN refs_json TEXT;
DROP TRIGGER IF EXISTS trg_trust_workflow_update;
CREATE TRIGGER trg_trust_workflow_update AFTER UPDATE ON workflows
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
    AND OLD.refs_json IS NEW.refs_json
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
