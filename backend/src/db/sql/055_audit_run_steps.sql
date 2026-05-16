-- 0.8.4 (#298) — Per-step audit metrics for the post-audit recap panel.
-- Pre-fix, the live chips (⏱ elapsed / 💬 tokens / 🔧 tool) only existed
-- WHILE the audit was running, dropped at audit end. Users had no way
-- to retro-answer "which step burned 80% of the tokens on this run?"
-- This table makes step-level perf permanently introspectable.
--
-- Inserted at `step_start` with (started_at, file_label); updated at
-- `step_done` with (ended_at, duration_ms, step_tokens, cumulative_tokens,
-- cli_success). `step_warning` (from #292) decorates the row when it
-- fires. Frontend reads `GET /api/audit-runs/:run_id/steps` for the
-- collapsed recap panel on ProjectCard.

CREATE TABLE audit_run_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    audit_run_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,            -- 1..10 (or 1..N for sub-audits)
    file_label TEXT NOT NULL,                -- "docs/glossary.md" | "Final review"
    started_at DATETIME NOT NULL,
    ended_at DATETIME,                       -- NULL while the step is running
    duration_ms INTEGER,                     -- NULL while running; populated at step_done
    step_tokens INTEGER,                     -- max(input + output) the step's CLI reported
    cumulative_tokens INTEGER,               -- running sum at end of this step
    cli_success INTEGER NOT NULL DEFAULT 1,  -- 0 = CLI exit non-zero OR step_warning fired
    step_warning TEXT,                       -- reason from validate_and_repair_step_output (#292)
    step_repaired_from_template INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
);

-- UNIQUE so that an interrupted+resumed run (#311) re-firing
-- step_start for an already-completed step does NOT duplicate the
-- row — `INSERT OR IGNORE` silently skips when the pair already
-- exists. Doubles as the lookup index for the recap query.
CREATE UNIQUE INDEX idx_audit_run_steps_run ON audit_run_steps(audit_run_id, step_index);
