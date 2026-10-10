-- KT-1030: the order of a task board's open cards, per board tag. The planning
-- rank is renumbered across a whole priority band, so it cannot hold it.
CREATE TABLE IF NOT EXISTS task_board_orders (
    tag         TEXT PRIMARY KEY COLLATE NOCASE,
    task_ids    TEXT NOT NULL DEFAULT '[]',
    updated_at  TEXT NOT NULL
);

-- KT-1030: content Kronn ships by default. A row means it was handled once
-- (installed, or not installed because the user had their own), so Kronn never
-- installs it again on its own; deleting the content keeps the row.
CREATE TABLE IF NOT EXISTS default_contents (
    key           TEXT PRIMARY KEY,
    status        TEXT NOT NULL CHECK (status IN ('installed', 'kept_existing')),
    page_id       TEXT,
    workflow_ids  TEXT NOT NULL DEFAULT '[]',
    updated_at    TEXT NOT NULL
);
