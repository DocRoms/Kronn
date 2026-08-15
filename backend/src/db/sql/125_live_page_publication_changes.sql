-- v0.10.0 — compact Page publication deltas for the refresh timeline.
-- Existing ledger rows predate comparison-at-write-time. Treat their touched
-- datasets as changed rather than falsely claiming that their data was equal.

ALTER TABLE live_page_publications
    ADD COLUMN changed_datasets_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE live_page_publications
    ADD COLUMN unchanged_datasets_json TEXT NOT NULL DEFAULT '[]';

UPDATE live_page_publications
   SET changed_datasets_json = datasets_json;
