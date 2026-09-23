-- how a project's isolated worktree is made usable by its own tool
-- chain. A worktree under `.kronn/worktrees/` is invisible to containers
-- mounted on the main checkout, so a validation run there reads the wrong
-- code and passes — green, silent, wrong. The recipe that fixes it belongs to
-- the project, which knows how it builds and how it tests; a workflow should
-- not have to carry a copy of it.
ALTER TABLE projects ADD COLUMN workspace_json TEXT DEFAULT NULL;
