-- New launches only: never infer an old discussion's effort from a mutable QP.
CREATE TABLE discussion_effort_snapshots (
    discussion_id TEXT PRIMARY KEY REFERENCES discussions(id) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    tier TEXT NOT NULL,
    model TEXT NOT NULL,
    effort TEXT NOT NULL
);
