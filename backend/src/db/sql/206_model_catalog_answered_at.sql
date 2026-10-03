-- KT-957 — when a real call to this model last answered.
--
-- `availability` only says the provider lists the model; a LiteLLM proxy lists
-- models it cannot serve. NULL = never proven by a call, not "broken".
ALTER TABLE model_catalog_entries ADD COLUMN last_answered_at TEXT;
