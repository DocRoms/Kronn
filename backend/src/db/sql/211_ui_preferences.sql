-- KT-972 — interface preferences kept by the server, so they survive a change
-- of browser origin (the desktop's local port) that empties localStorage.
-- One row: the whole map of synced keys, as a JSON object of strings.
CREATE TABLE IF NOT EXISTS ui_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    values_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
