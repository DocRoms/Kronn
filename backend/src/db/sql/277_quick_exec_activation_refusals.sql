-- KT-1490 — a human refused to activate the line an agent saved. The refusal
-- names the line it was given for: a line edited since is asked again. The
-- Quick Exec stays inactive either way; this only lets the room's card say so
-- after a reload.
CREATE TABLE IF NOT EXISTS quick_exec_activation_refusals (
    quick_exec_id TEXT PRIMARY KEY REFERENCES quick_execs(id) ON DELETE CASCADE,
    command       TEXT NOT NULL,
    args_json     TEXT NOT NULL,
    refused_at    TEXT NOT NULL
);
