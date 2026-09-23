-- Record the identity of recreated dependencies without modifying their definitions.
-- Multiple explicit copies are allowed; deleting a target leaves a harmless mapping
-- which is ignored after the target lookup fails.
CREATE TABLE artifact_import_origins (
    kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    PRIMARY KEY (kind, source_id, target_id)
);
