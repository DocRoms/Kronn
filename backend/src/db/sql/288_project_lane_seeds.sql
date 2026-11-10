-- KT-1438 — gitignored paths a project declares as lane seeds: copied from its
-- main checkout into every task worktree (a generated asset directory the
-- tracked code needs but a fresh worktree does not have). A JSON array of
-- repository-relative paths; NULL is none. Only a human edits it.
ALTER TABLE projects ADD COLUMN lane_seeds_json TEXT;
