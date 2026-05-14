-- 0.8.3 — Linked repos / companion projects (TD-20260512).
--
-- Companion repos that an agent on this project should know about:
-- a frontend pointing at its backend API repo + IaC repo + design
-- system repo, etc. Surfaced in every agent's system prompt prelude
-- so cross-repo context flows naturally.
--
-- Stored as in-row JSON (small data, projects rarely have more than
-- 5 links). Default `'[]'` so existing project rows load cleanly.

ALTER TABLE projects
    ADD COLUMN linked_repos_json TEXT NOT NULL DEFAULT '[]';
