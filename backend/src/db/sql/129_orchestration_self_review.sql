-- ─────────────────────────────────────────────────────────────────────────────
-- 129 — Self-review policy on an orchestration run (KT-319 DoD-7).
--
-- DoD-7: "the worker cannot self-approve BY DEFAULT; any exception is an explicit
-- policy." The default is therefore NO self-review (0). A run whose launcher
-- opts in (KT-321 will surface the toggle) sets this to 1, and only then may the
-- execution's own worker identity decide its review.
--
-- This is 129, not an in-place edit of 128: 128 is already frozen (both are
-- registered and migrations are name-tracked and never replay), so a new column
-- must be a fresh migration. A plain ADD COLUMN with a NOT NULL DEFAULT is
-- backfilled to the safe value (0) on every existing run — no run silently
-- inherits self-approval.
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE orchestration_runs
    ADD COLUMN allow_self_review INTEGER NOT NULL DEFAULT 0
        CHECK (allow_self_review IN (0, 1));
