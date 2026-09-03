-- KT-556 (migration 165): where an image came from, when it was not generated.
--
-- A frame taken out of a clip is not an upload and not an AI generation: the
-- browser decoded a picture the discussion already held. Recording the source
-- asset lets every surface say so — and lets the viewer offer the clip it came
-- from — without inferring provenance from a filename.
ALTER TABLE context_files ADD COLUMN extracted_from_asset_id TEXT
    REFERENCES context_files(id) ON DELETE SET NULL;
