-- KT-1098: former slugs of a renamed Live Page keep resolving to it, and stay
-- reserved for it until it is deleted so no other page can take its old links.
CREATE TABLE IF NOT EXISTS live_page_slug_aliases (
    slug        TEXT PRIMARY KEY,
    page_id     TEXT NOT NULL REFERENCES live_pages(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_live_page_slug_aliases_page
ON live_page_slug_aliases(page_id);
