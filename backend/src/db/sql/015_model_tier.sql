-- Model tier selection per discussion (economy / default / reasoning).
-- Split from 014 because 014 may have been partially applied without this column.
ALTER TABLE discussions ADD COLUMN model_tier TEXT DEFAULT 'default';
