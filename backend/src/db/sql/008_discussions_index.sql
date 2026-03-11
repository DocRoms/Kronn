-- Index for filtering discussions by archived status and ordering by updated_at
CREATE INDEX IF NOT EXISTS idx_discussions_archived ON discussions(archived, updated_at DESC);
