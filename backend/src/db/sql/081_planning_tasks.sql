-- Planning workspace and discussion plans.
-- The `tasks` name is already occupied by the pre-workflow scheduler table,
-- so the new domain uses an explicit `planning_` prefix.

CREATE TABLE IF NOT EXISTS planning_tasks (
    id              TEXT PRIMARY KEY,
    task_number     INTEGER NOT NULL UNIQUE,
    parent_id       TEXT REFERENCES planning_tasks(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'idea',
    priority        TEXT NOT NULL DEFAULT 'normal',
    rank            INTEGER NOT NULL DEFAULT 0,
    blocked_reason  TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_planning_tasks_backlog
    ON planning_tasks(status, priority, rank, updated_at);
CREATE INDEX IF NOT EXISTS idx_planning_tasks_parent
    ON planning_tasks(parent_id);

CREATE TABLE IF NOT EXISTS planning_task_projects (
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_planning_task_projects_project
    ON planning_task_projects(project_id);

CREATE TABLE IF NOT EXISTS planning_task_discussions (
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    placement       TEXT NOT NULL DEFAULT 'active'
                        CHECK (placement IN ('active', 'later')),
    is_primary      INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1)),
    position        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (task_id, discussion_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_planning_one_primary_per_discussion
    ON planning_task_discussions(discussion_id)
    WHERE is_primary = 1;
CREATE INDEX IF NOT EXISTS idx_planning_task_discussions_plan
    ON planning_task_discussions(discussion_id, placement, position);

CREATE TABLE IF NOT EXISTS planning_task_dod_items (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    sentence        TEXT NOT NULL,
    completed       INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    position        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_planning_task_dod_items_task
    ON planning_task_dod_items(task_id, position);

CREATE TABLE IF NOT EXISTS planning_task_links (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    label           TEXT NOT NULL,
    url             TEXT NOT NULL,
    position        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_planning_task_links_task
    ON planning_task_links(task_id, position);

CREATE TABLE IF NOT EXISTS planning_task_tags (
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    tag             TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (task_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_planning_task_tags_tag
    ON planning_task_tags(tag);

CREATE TABLE IF NOT EXISTS planning_task_blockers (
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    blocker_task_id TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (task_id, blocker_task_id),
    CHECK (task_id <> blocker_task_id)
);
CREATE INDEX IF NOT EXISTS idx_planning_task_blockers_reverse
    ON planning_task_blockers(blocker_task_id);

CREATE TABLE IF NOT EXISTS planning_task_events (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    action            TEXT NOT NULL,
    actor_kind        TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id           TEXT,
    changes_json       TEXT NOT NULL DEFAULT '{}',
    source_message_id  TEXT REFERENCES messages(id) ON DELETE SET NULL,
    created_at         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_planning_task_events_task
    ON planning_task_events(task_id, created_at);
