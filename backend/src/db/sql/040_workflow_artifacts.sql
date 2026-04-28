-- 0.7.0 Phase 3 — declared workflow artifacts.
--
-- Stored as JSON like `guards` (migration 039): a `HashMap<String, ArtifactSpec>`
-- serialised. NULL or empty `{}` = the workflow declares no artifacts.
-- Adding a new ArtifactSpec field later (retention_days, commit_to_branch,
-- etc.) only requires expanding the Rust struct — no further migration.
-- The runner reads/parses this once at run start.
ALTER TABLE workflows ADD COLUMN artifacts TEXT;
