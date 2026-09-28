-- Repository resource synchronization baselines and approvals are local
-- state. They deliberately do not live in kronn/ so cloning a repository
-- never imports trust from another machine.
CREATE TABLE repository_resource_alignments (
    project_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    slug TEXT NOT NULL,
    target_id TEXT NOT NULL,
    repository_hash TEXT NOT NULL,
    database_hash TEXT NOT NULL,
    aligned_at TEXT NOT NULL,
    imported INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_key, kind, slug)
);

CREATE INDEX idx_repository_resource_alignments_target
    ON repository_resource_alignments (kind, target_id, aligned_at DESC);

CREATE TABLE repository_resource_approvals (
    project_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    slug TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    approved_at TEXT NOT NULL,
    PRIMARY KEY (project_key, kind, slug, content_hash)
);
