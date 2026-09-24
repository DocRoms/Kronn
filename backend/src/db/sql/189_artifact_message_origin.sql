-- A preview promoted to an Artifact retains a navigable source message.
ALTER TABLE live_page_discussion_links
    ADD COLUMN source_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL;
