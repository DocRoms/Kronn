-- KT-1346: the room a general resource was created from, recorded by the
-- bridge gate at creation (never read from a request body). With « General ·
-- write », a room's agents edit only the general resources the room created.
CREATE TABLE general_resource_origins (
    kind            TEXT NOT NULL,
    resource_id     TEXT NOT NULL,
    discussion_id   TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (kind, resource_id, discussion_id)
);

CREATE INDEX idx_general_resource_origins_room
    ON general_resource_origins(discussion_id);
