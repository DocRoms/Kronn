-- KT-1492: a grant remembers every room its caller was bound to, not only the
-- first. The level in force for a grant is the lowest of its rooms' levels, so
-- withdrawing it must rebuild the whole set. A JSON array of discussion ids;
-- NULL on rows written before this column (they fall back to `discussion_id`).
ALTER TABLE trust_audit_log ADD COLUMN discussion_ids TEXT;
