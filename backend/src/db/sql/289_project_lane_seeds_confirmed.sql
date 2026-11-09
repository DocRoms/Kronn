-- KT-1438 — the secret-looking files (a `.env`, a key) a human confirmed inside
-- each declared lane seed. A JSON object: seed path -> confirmed file paths. A
-- file the seed holds that is not listed here keeps the seed out of the task
-- worktrees until a human confirms it; NULL is nothing confirmed.
ALTER TABLE projects ADD COLUMN lane_seeds_confirmed_json TEXT;
