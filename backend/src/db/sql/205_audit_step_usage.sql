-- KT-927 — what the agent of an audit step reported it consumed.
--
-- `step_tokens` stays the step's headline figure (input + output). It could only
-- be read from Claude's own stream-json, so every step of an ACP agent (OpenCode,
-- Gemini, Copilot, Kiro, Vibe, and Claude / Codex through their adapters) was
-- recorded as 0. These four columns keep the parts apart, as the agent reports
-- them: whether `input_tokens` already contains the cached share depends on the
-- agent (see `core::pricing::TokenCounters::from_agent_report`).
--
-- Every column is NULL when the runtime did NOT report it — never 0. A step with
-- no figure at all is unknown, not free.
ALTER TABLE audit_run_steps ADD COLUMN input_tokens INTEGER;
ALTER TABLE audit_run_steps ADD COLUMN output_tokens INTEGER;
ALTER TABLE audit_run_steps ADD COLUMN cache_read_tokens INTEGER;
ALTER TABLE audit_run_steps ADD COLUMN cache_write_tokens INTEGER;
