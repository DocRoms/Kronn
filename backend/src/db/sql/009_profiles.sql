-- Add profile support to discussions and projects
ALTER TABLE discussions ADD COLUMN profile_id TEXT DEFAULT NULL;
ALTER TABLE projects ADD COLUMN default_profile_id TEXT DEFAULT NULL;
