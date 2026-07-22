-- Keep an agent switch silent until the user's next real message. The value
-- stores the agent that owned the conversation before the first pending
-- switch; successive switches therefore collapse into one concise handoff.
ALTER TABLE discussions ADD COLUMN pending_agent_handoff_from TEXT;
