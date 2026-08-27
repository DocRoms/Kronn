-- Favorites for every reusable Automation resource.
-- Existing rows remain unpinned; favorites are a local library preference.
ALTER TABLE quick_prompts ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE quick_apis ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE quick_execs ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;

