-- Periodic checkpoint of an in-flight agent's `full_response` so a backend
-- crash/restart doesn't lose the user's work-in-progress (Option A from the
-- 2026-04-13 design discussion: simple, additive, no OS plumbing).
--
-- During make_agent_stream, we flush the accumulated response to this column
-- every ~30s or ~100 chunks. On normal completion the column is set back to
-- NULL (the final message lives in `messages`). On restart, the boot orphan
-- scan converts any non-null partial into an Agent message with a clear
-- "interrupted" footer so the user sees what was thought + can re-run.
ALTER TABLE discussions ADD COLUMN partial_response TEXT;

CREATE INDEX IF NOT EXISTS idx_discussions_partial_response
    ON discussions(partial_response)
    WHERE partial_response IS NOT NULL;
