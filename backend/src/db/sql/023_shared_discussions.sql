-- Shared discussions: cross-Kronn replicated discussions
ALTER TABLE discussions ADD COLUMN shared_id TEXT;
ALTER TABLE discussions ADD COLUMN shared_with_json TEXT NOT NULL DEFAULT '[]';
CREATE INDEX idx_discussions_shared_id ON discussions(shared_id);
