ALTER TABLE discussions ADD COLUMN workspace_mode TEXT NOT NULL DEFAULT 'Direct';
ALTER TABLE discussions ADD COLUMN workspace_path TEXT;
ALTER TABLE discussions ADD COLUMN worktree_branch TEXT;
