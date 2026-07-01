-- v0.8.10 (Phase 2b/2c) — explicit per-QuickPrompt / per-discussion model.
--
-- Until now the model was resolved solely from the tier (resolve_model_flag).
-- These two nullable columns carry an explicit model that wins over the tier
-- (see runner::effective_model_flag). NULL = no override → tier resolution,
-- so every existing row keeps its current behaviour.
--
--   quick_prompts.agent_settings_json : serialized AgentSettings { model, tier,
--       reasoning_effort, max_tokens } — mirrors WorkflowStep.agent_settings.
--   discussions.model                 : the model a QP-launched (or manually
--       set) discussion runs with; threaded to the agent as model_override.
ALTER TABLE quick_prompts ADD COLUMN agent_settings_json TEXT;
ALTER TABLE discussions ADD COLUMN model TEXT;
