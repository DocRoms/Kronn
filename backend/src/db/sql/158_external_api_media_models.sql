-- Media model slots on an external API connection.
--
-- Separate columns rather than new ModelTier variants: image and video are
-- output MODALITIES, not quality levels, so making them tiers would let a
-- workflow step pick "tier Image" for a text agent and would force every
-- existing match on ModelTier to grow two meaningless arms.
--
-- Nullable on purpose: a provider with no media catalogue simply leaves them
-- empty, and the UI shows an explicit empty state instead of an unfillable
-- field.
ALTER TABLE external_api_connections ADD COLUMN image_model TEXT;
ALTER TABLE external_api_connections ADD COLUMN video_model TEXT;

-- Optional override for providers that serve media from another host than the
-- configured chat endpoint (NVIDIA serves visual generation from
-- ai.api.nvidia.com while its connection stores integrate.api.nvidia.com).
-- Empty means "derive it from the endpoint", which is what OpenRouter needs.
ALTER TABLE external_api_connections ADD COLUMN media_endpoint TEXT;
