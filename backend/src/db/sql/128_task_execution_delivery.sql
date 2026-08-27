-- ─────────────────────────────────────────────────────────────────────────────
-- 128 — Structured delivery + review contracts (KT-319).
--
-- The durable review ping-pong: a worker submits a versioned DeliveryManifest,
-- the principal answers with a versioned ReviewDecision. Both are persisted
-- here, ATTEMPT-SCOPED like 127's brief / dispatch / offer dedupe keys: one row
-- per (execution, attempt). A request_changes bumps the execution to the next
-- attempt, so each review round lands on its own row → a versioned, auditable
-- per-round history (DoD-1). An idempotent re-submit / re-decide of the SAME
-- attempt is an upsert on the unique index, never a duplicate row (DoD-8:
-- double-clicks and crash replays neither lose nor duplicate).
--
-- Actor identity ("who delivered / who decided") is deliberately NOT stored
-- here: like the worker-offers table (127), the durable, non-spoofable actor
-- trace lives in task_execution_events (backend/agent/human actor kinds). That
-- also keeps these rows free of any discussion_sessions FK — no CASCADE/RESTRICT
-- deletion trap. Both tables cascade with their execution (127 base row).
--
-- This is 128, not an in-place edit of 127: 127 is already applied on the
-- durable base and migrations are name-tracked and never replay, so new tables
-- must be a fresh migration.
-- ─────────────────────────────────────────────────────────────────────────────

-- Worker → backend/principal: the versioned DeliveryManifest (DoD-1).
CREATE TABLE IF NOT EXISTS task_execution_deliveries (
    id                 TEXT PRIMARY KEY,
    task_execution_id  TEXT NOT NULL
                           REFERENCES task_executions(id) ON DELETE CASCADE,
    -- Attempt-scoped like 127's brief/dispatch/offer keys: a KT-319 review round
    -- (request_changes) delivers again under the next attempt_no.
    attempt_no         INTEGER NOT NULL DEFAULT 0 CHECK (attempt_no >= 0),
    -- The exact HEAD the worker delivered. Denormalized from manifest_json so the
    -- DoD-5 drift check (compare against the real worktree HEAD at approve time)
    -- is a column read, not a JSON extract.
    head_sha           TEXT NOT NULL,
    -- The full versioned DeliveryManifest v1 (files, tests, dod_status, docs,
    -- migrations, risks, limitations, summary) — validated before insert.
    manifest_json      TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

-- One delivery per (execution, attempt). Full unique (not partial): a delivery
-- has no status lifecycle (unlike an offer), so there is never more than one
-- meaningful row per attempt — a re-submit upserts it.
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_exec_deliveries_one_per_attempt
    ON task_execution_deliveries(task_execution_id, attempt_no);

-- Principal → backend: the versioned ReviewDecision (approve | request_changes).
CREATE TABLE IF NOT EXISTS task_execution_reviews (
    id                 TEXT PRIMARY KEY,
    task_execution_id  TEXT NOT NULL
                           REFERENCES task_executions(id) ON DELETE CASCADE,
    -- The attempt that was reviewed (pairs 1:1 with the delivery of that attempt).
    attempt_no         INTEGER NOT NULL DEFAULT 0 CHECK (attempt_no >= 0),
    decision           TEXT NOT NULL
                           CHECK (decision IN ('approve', 'request_changes')),
    -- The full versioned ReviewDecision v1 (comment, structured findings) —
    -- validated before insert.
    decision_json      TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

-- One decision per (execution, attempt): request_changes bumps the attempt, so
-- the next decision lands on its own row (per-round audit); a re-decide of the
-- same attempt upserts.
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_exec_reviews_one_per_attempt
    ON task_execution_reviews(task_execution_id, attempt_no);
