-- The order a discussion's clips are played in as one film (Assets > Editor).
-- Kept per discussion and server-side, so it survives a reload and follows
-- the human from one device to another. Only the ids a person arranged are
-- stored; a clip generated afterwards joins the end when the order is read,
-- and one deleted since simply drops out.
CREATE TABLE discussion_video_sequences (
    discussion_id TEXT PRIMARY KEY REFERENCES discussions(id) ON DELETE CASCADE,
    file_ids_json TEXT NOT NULL DEFAULT '[]',
    updated_at TEXT NOT NULL
);
