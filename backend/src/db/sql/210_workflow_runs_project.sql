-- KT-1015: the project a run resolved at launch. A global workflow launched
-- from a project has no project of its own; resume and worktree cleanup read
-- this column first. Older rows stay NULL and fall back to the workflow's.
ALTER TABLE workflow_runs ADD COLUMN project_id TEXT;
