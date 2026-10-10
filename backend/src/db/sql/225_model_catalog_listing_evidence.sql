-- KT-667: what the runtime's own model listing says about a model, kept apart
-- from availability. NULL = unknown, 'listed', or 'not_listed' by a complete
-- listing. Not listed is a warning, never a refusal: a listing is no access proof.
ALTER TABLE model_catalog_entries ADD COLUMN catalog_listing TEXT;
