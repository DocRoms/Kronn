-- Migrate from single profile_id to multi profile_ids (JSON array)
ALTER TABLE discussions ADD COLUMN profile_ids_json TEXT DEFAULT '[]';

-- Migrate existing profile_id data to profile_ids_json
UPDATE discussions SET profile_ids_json = json_array(profile_id) WHERE profile_id IS NOT NULL AND profile_id != '';
