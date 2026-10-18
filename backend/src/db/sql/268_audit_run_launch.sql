-- 268 (KT-1320): how the run was launched, so a step retry or a resume runs
-- on the same footing unless the human changes it.
--   tier           the model tier of the launch (`economy` / `default` /
--                  `reasoning`); NULL on a run recorded before this column.
--   connection_id  the named external API connection of an HTTP agent, if any.
ALTER TABLE audit_runs ADD COLUMN tier TEXT;
ALTER TABLE audit_runs ADD COLUMN connection_id TEXT;
