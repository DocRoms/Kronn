-- Track how many times the agent has called the kronn-internal
-- introspection tools (disc_meta / disc_get_message / disc_summarize)
-- on this discussion. Surface as a small pill in the ChatHeader so the
-- user knows the agent has been digging in the history — useful both
-- as a "this agent is using its context tools well" signal and as a
-- "wait, why did it call disc_summarize 30 times?" anomaly detector.

ALTER TABLE discussions
ADD COLUMN introspection_call_count INTEGER NOT NULL DEFAULT 0;
