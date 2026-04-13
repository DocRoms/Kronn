-- Track when the in-flight agent started producing its (still partial)
-- response. Needed by recover_partial_responses so the recovered Agent
-- message falls chronologically BEFORE any user message posted after
-- restart — otherwise users who don't see the partial (WS not yet subscribed)
-- and resend their prompt end up with the recovered message appearing AFTER
-- their 2nd user message, which reads as a spurious third agent answer.
--
-- Set on the FIRST checkpoint for a given run; preserved across subsequent
-- checkpoints; cleared to NULL alongside partial_response on normal completion.
ALTER TABLE discussions ADD COLUMN partial_response_started_at TEXT;
