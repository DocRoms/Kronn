-- Pin / favorite discussions so they appear in a dedicated "Favorites" section
-- at the top of the sidebar, easy to find regardless of project grouping.
ALTER TABLE discussions ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
