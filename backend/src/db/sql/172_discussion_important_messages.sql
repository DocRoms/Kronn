-- KT-619 — important messages: steering cards, kept apart from worker reports.
--
-- One row per important message, always alongside a message row created for it
-- in the same transaction.
--
-- What these constraints do and do not prove:
-- `UNIQUE(message_id)` stops a message carrying two cards. On its own it does
-- NOT stop an old message from gaining its first one — that is enforced in the
-- service, which mints the message and refuses any target that is not the one
-- it just wrote.
-- `author_kind` records which authority published. It constrains the value, not
-- the writer: a privileged SQL client could still choose 'human'. The identity
-- check lives where the caller's lineage is known, not here.
CREATE TABLE discussion_important_messages (
    id              TEXT NOT NULL PRIMARY KEY,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    message_id      TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    category        TEXT NOT NULL CHECK (category IN (
                        'decision',
                        'scope_change',
                        'dod_waiver',
                        'blocking_alert',
                        'human_action_required',
                        'accepted_delivery'
                    )),
    schema_version  INTEGER NOT NULL,
    -- Stable identity of the fact being reported. Structured events replay on
    -- restart and resume; the unique index below turns a replay into a no-op
    -- instead of a duplicate card.
    dedup_key       TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    author_kind     TEXT NOT NULL CHECK (author_kind IN ('orchestrator', 'human')),
    author_label    TEXT NOT NULL,
    -- Provenance of the steering event this card was minted from, so the link
    -- survives a restart and stays auditable.
    source_kind     TEXT,
    source_id       TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (discussion_id, dedup_key)
);

CREATE UNIQUE INDEX idx_disc_important_message
    ON discussion_important_messages(message_id);

-- Filter, counter and previous/next all read the same order the transcript
-- uses, so navigation cannot disagree with the list it navigates.
CREATE INDEX idx_disc_important_discussion
    ON discussion_important_messages(discussion_id, category);

CREATE INDEX idx_disc_important_source
    ON discussion_important_messages(source_kind, source_id)
    WHERE source_kind IS NOT NULL;
