-- Store which model tier was used for each agent message.
-- Allows showing per-message tier badges (eco/standard/reasoning).
ALTER TABLE messages ADD COLUMN model_tier TEXT;
