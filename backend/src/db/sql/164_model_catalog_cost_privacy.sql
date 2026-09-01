-- KT-543 (migration 164): generic cost-hint / privacy-note overlay on the
-- KT-531 model catalog (162_model_catalog.sql). Not provider-specific in
-- schema — any runtime_target_id's entries may carry it. First consumer is
-- OpenCode Zen, whose gateway exposes no live pricing signal (KT-543).
ALTER TABLE model_catalog_entries ADD COLUMN cost_hint TEXT
    CHECK(cost_hint IS NULL OR cost_hint IN ('free','paid','unknown'));
ALTER TABLE model_catalog_entries ADD COLUMN privacy_note TEXT;
