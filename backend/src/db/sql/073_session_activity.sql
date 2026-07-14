-- 0.8.12 PR B — presence phase 1, server-derived activity placeholder.
-- Two SERVER FACTS only (never agent self-declaration, never reasoning
-- text): an open wait_for_peer long-poll = "listening"; a wait that just
-- DELIVERED messages with no reply posted since = "reading". The TTL is
-- declarative: expiry is evaluated at READ time (expired ⇒ null), no
-- reaper involved. Cleared on append/leave.
ALTER TABLE discussion_sessions ADD COLUMN activity TEXT;
ALTER TABLE discussion_sessions ADD COLUMN activity_expires_at DATETIME;
