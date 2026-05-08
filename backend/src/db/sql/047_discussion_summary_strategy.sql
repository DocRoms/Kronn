-- Per-discussion summary strategy.
--
-- Pre-fix every discussion ran the auto-summary loop unconditionally with
-- per-agent thresholds (12/8/4 msgs depending on context budget). On big-
-- context models (Claude Opus 1M, GPT-5 ultra) the summary fires too early
-- and burns tokens for a compression the agent doesn't need. On small
-- models / debate mode the summary IS necessary. User feedback on
-- 2026-05-09 asked for this to be tunable per-discussion.
--
-- Values:
--   'Auto'     — current behaviour (default, backward-compatible)
--   'OnDemand' — no auto summary; agent can request via the kronn-internal
--                MCP tool surface (planned, see project_disc_introspection.md)
--   'Off'      — never summarize; agent receives the raw transcript until
--                its context window saturates

ALTER TABLE discussions
ADD COLUMN summary_strategy TEXT NOT NULL DEFAULT 'Auto';
