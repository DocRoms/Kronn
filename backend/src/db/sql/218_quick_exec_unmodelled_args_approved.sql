-- KT-1017 — a human confirmed that an unmodelled program of this Quick Exec
-- treats its arguments as plain data.
ALTER TABLE quick_execs ADD COLUMN unmodelled_args_approved INTEGER NOT NULL DEFAULT 0;
