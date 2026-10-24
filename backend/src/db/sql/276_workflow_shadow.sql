-- KT-1449 — a workflow's shadow variant (JSON `WorkflowShadow`): each normal
-- run also starts a shadow run with other skill/QP versions, external effects
-- skipped. NULL = no shadow. Human-only, like `retention_json`.
ALTER TABLE workflows ADD COLUMN shadow_json TEXT;
