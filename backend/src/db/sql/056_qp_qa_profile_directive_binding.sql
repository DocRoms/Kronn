-- 0.8.5 — Symmetrize QuickPrompt + QuickApi with the Discussion / Workflow
-- binding triplet (skill_ids, profile_ids, directive_ids). Pre-0.8.5, QPs
-- and QAs only carried skill_ids; the persona (profile) and rule-of-conduct
-- (directive) layers had to be re-selected every time a QP / QA was
-- launched. After this migration, a QP / QA can pin its persona +
-- directive set, mirroring `WorkflowStep` and the discussion form.
--
-- Both columns are JSON arrays of profile / directive ids, NOT NULL with
-- an empty-array default so legacy rows deserialize as "unbound" without a
-- migration callout (matches the pattern used for `skill_ids_json` in 026).

ALTER TABLE quick_prompts ADD COLUMN profile_ids_json   TEXT NOT NULL DEFAULT '[]';
ALTER TABLE quick_prompts ADD COLUMN directive_ids_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE quick_apis    ADD COLUMN profile_ids_json   TEXT NOT NULL DEFAULT '[]';
ALTER TABLE quick_apis    ADD COLUMN directive_ids_json TEXT NOT NULL DEFAULT '[]';
