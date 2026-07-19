-- 0.8.3 (#311) — track per-step progress on audit_runs so an interrupted
-- audit (rate-limit, crash, manual cancel mid-stream) can be RESUMED
-- from the next un-completed step instead of restarting from step 1.
--
-- Before this column, any interruption forced a full re-run — which
-- meant burning 30-100k tokens redoing work the agent had already
-- produced. The DOCROMS_WEB rate-limit on 2026-05-15 (step 5/10 done,
-- 5 steps lost) is what motivated this column.
--
-- Values:
--   0    — audit started but no step has finished yet (default).
--   1..N — last successfully completed step in the executed 1-based chain.
--          For the current chained Full audit, N=16 means completion.
--
-- Updated on each `step_done` SSE event where `validate_and_repair_step_output`
-- returns success=true. NOT updated for step_warning or step_error.

ALTER TABLE audit_runs ADD COLUMN last_completed_step INTEGER NOT NULL DEFAULT 0;
