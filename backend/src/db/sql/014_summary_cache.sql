-- Summary cache for conversation history compression (eco-design).
-- Stores a condensed summary of older messages so agents don't need the full history.
ALTER TABLE discussions ADD COLUMN summary_cache TEXT;
ALTER TABLE discussions ADD COLUMN summary_up_to_msg_idx INTEGER;
