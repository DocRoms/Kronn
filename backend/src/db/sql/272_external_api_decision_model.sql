-- The decision model slot on an external API connection (KT-1366): the model
-- Decision steps call. A capability like the media slots, not a ModelTier.
ALTER TABLE external_api_connections ADD COLUMN decision_model TEXT;
