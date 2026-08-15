-- v0.10.0 — turn Live Pages into a first-class library and let discussions
-- keep explicit provenance/attachment links to authored Pages.

ALTER TABLE live_pages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE live_pages ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_live_pages_library
    ON live_pages(archived, pinned DESC, updated_at DESC);

CREATE TABLE IF NOT EXISTS live_page_discussion_links (
    page_id        TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    discussion_id  TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    relation       TEXT NOT NULL DEFAULT 'attached'
                   CHECK(relation IN ('created_from', 'attached')),
    created_at     TEXT NOT NULL,
    PRIMARY KEY(page_id, discussion_id)
);

CREATE INDEX IF NOT EXISTS idx_live_page_discussions_discussion
    ON live_page_discussion_links(discussion_id, created_at DESC);
