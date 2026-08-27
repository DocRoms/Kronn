-- 0.11.0 (KT-317) — durable multi-agent task orchestration persistence.
--
-- ADR-002 (docs/design/adr-002-orchestration-multi-agent.md) decides a distinct
-- `task_executions` aggregate (O2) rather than folding a TaskExecution into
-- `workflow_runs`: a TaskExecution is a two-agent review loop across two
-- discussions, not a step-graph. It borrows the proven invariants — sticky
-- SQL-predicate transitions, boot reconcile, terminal-lock — via a shared
-- `run_state` helper, and composes existing primitives (dispatch, worktree,
-- budget). This migration lays only the persistence + lineage; provisioning
-- (KT-318) and the protected Git merge (KT-320) come later.
--
-- What this migration must NOT do (kept for downstream tasks): create a real
-- sub-discussion or worktree, dispatch a worker, or run any git merge/validate.
-- The saga checkpoint columns exist here so KT-320's integration is replay-safe,
-- but nothing writes them yet.

-- ─────────────────────────────────────────────────────────────────────────────
-- OrchestrationRun — the mandatory campaign envelope (ADR §1, §2).
--
-- Every TaskExecution carries a non-null FK to one of these. A single-task
-- "Create and run" auto-creates an implicit `single_task` run, so there are
-- never standalone/nullable rows for KT-321 (coordination) to reinterpret. It
-- owns the tree-wide policy: target workspace/branch, budget, concurrency,
-- review budget, integration strategy and validations.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS orchestration_runs (
    id                          TEXT PRIMARY KEY,
    -- `single_task` is the V1 shape (one implicit run per launch). `campaign`
    -- is reserved for KT-321 (a principal driving several ready tasks).
    kind                        TEXT NOT NULL DEFAULT 'single_task'
                                    CHECK (kind IN ('single_task', 'campaign')),
    -- The principal's home discussion; the campaign lives and escalates here.
    discussion_id               TEXT NOT NULL
                                    REFERENCES discussions(id) ON DELETE CASCADE,
    project_id                  TEXT REFERENCES projects(id) ON DELETE SET NULL,

    -- Integration target, pinned explicitly — never an implicit/inferred branch.
    target_workspace_id         TEXT
                                    REFERENCES discussion_workspaces(id) ON DELETE SET NULL,
    target_branch               TEXT,

    -- Policy carried tree-wide.
    max_review_rounds           INTEGER NOT NULL DEFAULT 3,
    max_concurrent_executions   INTEGER NOT NULL DEFAULT 1,
    -- NULL = inherit / unbounded; otherwise a SharedBudget token quota.
    token_budget                INTEGER,
    integration_strategy        TEXT NOT NULL DEFAULT 'two_phase_ff_only'
                                    CHECK (integration_strategy IN ('two_phase_ff_only')),
    -- JSON: Vec<ValidationSpec> run on the exact candidate before apply (§6).
    validation_json             TEXT NOT NULL DEFAULT '[]',
    escalation_notify_url       TEXT,

    status                      TEXT NOT NULL DEFAULT 'active'
                                    CHECK (status IN ('active', 'completed', 'cancelled', 'failed')),

    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_orchestration_runs_discussion
    ON orchestration_runs(discussion_id, status);
CREATE INDEX IF NOT EXISTS idx_orchestration_runs_status
    ON orchestration_runs(status);

-- ─────────────────────────────────────────────────────────────────────────────
-- TaskExecution — the durable unit of work (ADR §1, §3, §4bis).
--
-- One task → one worker → one sub-discussion → one worktree → review →
-- integration. The task (planning_tasks) stays the source of truth; this row is
-- its separable, archivable execution shadow. State machine + saga checkpoints
-- live here; the sticky-transition invariant is enforced in `db/run_state.rs`.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS task_executions (
    id                      TEXT PRIMARY KEY,
    orchestration_run_id    TEXT NOT NULL
                                REFERENCES orchestration_runs(id) ON DELETE CASCADE,
    task_id                 TEXT NOT NULL
                                REFERENCES planning_tasks(id) ON DELETE CASCADE,

    -- Execution space. `parent_discussion_id` is where the principal reviews and
    -- the result is delivered; `sub_discussion_id`/`workspace_id`/`dispatch_job_id`
    -- are filled by provisioning (KT-318) and stay NULL until then.
    parent_discussion_id    TEXT NOT NULL
                                REFERENCES discussions(id) ON DELETE CASCADE,
    sub_discussion_id       TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    workspace_id            TEXT REFERENCES discussion_workspaces(id) ON DELETE SET NULL,
    dispatch_job_id         TEXT REFERENCES agent_dispatch_jobs(id) ON DELETE SET NULL,

    -- Git targeting contract (ADR §4). `base_sha` is the pinned parent HEAD the
    -- child branch is created from; the child branch is explicit, never inferred.
    base_sha                TEXT,
    child_branch            TEXT,

    -- The specialized worker chosen explicitly at launch (auto-selection is
    -- KT-324+). NULL until provisioning (KT-318) wires the dispatch.
    --
    -- Worker IDENTITY is the durable typed `MessageTarget` contract (ADR §5), not
    -- a loose provider string: `worker_target_kind` mirrors MessageTargetKind
    -- (discussion_agent | agent | cli) and, for a `cli` worker, `worker_cli_session_id`
    -- pins the EXACT joined session so two CLIs of the same provider are never
    -- confused. The identity FK is RESTRICT (no ON DELETE clause = NO ACTION): a
    -- session referenced by an execution cannot be deleted out from under the audit
    -- trail. The triplet is all-or-nothing — enforced by the table CHECK below.
    worker_target_kind      TEXT
                                CHECK (worker_target_kind IS NULL
                                       OR worker_target_kind IN ('discussion_agent', 'agent', 'cli')),
    worker_cli_session_id   INTEGER REFERENCES discussion_sessions(id),
    worker_agent_type       TEXT,
    worker_model            TEXT,
    worker_model_tier       TEXT,

    -- State machine (ADR §3). Only Done/Failed/Cancelled are terminal & sticky;
    -- Interrupted is a quiescent, resumable reconcile target, never an outcome.
    status                  TEXT NOT NULL DEFAULT 'Pending'
                                CHECK (status IN (
                                    'Pending', 'Provisioning', 'Blocked', 'Working',
                                    'AwaitingReview', 'Approved', 'ChangesRequested',
                                    'Integrating', 'Validating', 'Applying',
                                    'Escalated', 'Interrupted',
                                    'Done', 'Failed', 'Cancelled'
                                )),

    -- Resume-target checkpoints (ADR §3: "Blocked clears back to the state it
    -- left"). A hold records its origin so a *guarded* resume returns to the exact
    -- state, never a broader structurally-legal one: a Provisioning-origin Blocked
    -- must not resume Applying. Preserved across an Interrupt of a Blocked row so
    -- the deblock still knows where to go; cleared once the hold is resumed out of.
    --   blocked_from_status     : Provisioning|Applying — the state Blocked resumes to
    --   interrupted_from_status : exact pre-interruption state, for a deterministic resume
    -- These columns ARE part of the durable state machine, so their domain is
    -- constrained here: a bad value is a hard write error, never silently coerced
    -- to NULL (which would erase the resume target the §4bis saga depends on).
    -- Only Provisioning|Applying can enter Blocked; interrupted_from is any
    -- non-terminal state except Interrupted itself.
    blocked_from_status     TEXT
                                CHECK (blocked_from_status IS NULL
                                       OR blocked_from_status IN ('Provisioning', 'Applying')),
    interrupted_from_status TEXT
                                CHECK (interrupted_from_status IS NULL
                                       OR interrupted_from_status IN (
                                           'Pending', 'Provisioning', 'Blocked', 'Working',
                                           'AwaitingReview', 'Approved', 'ChangesRequested',
                                           'Integrating', 'Validating', 'Applying', 'Escalated'
                                       )),

    -- Review budget (ADR §6). Merge conflicts and request_changes both increment.
    review_rounds           INTEGER NOT NULL DEFAULT 0,
    max_review_rounds       INTEGER NOT NULL DEFAULT 3,

    -- Attempt counter (ADR §5: the Brief is "immutable per attempt"). This is the
    -- SEMANTIC brief/re-dispatch counter — NOT a technical retry of the same
    -- AgentDispatchJob. 0 at launch (KT-318); KT-319 increments it atomically on
    -- each new business attempt (ChangesRequested → Working) so the brief/dispatch
    -- dedupe keys (`orch-brief:{exec}:{attempt}` / `orch-dispatch:{exec}:{attempt}`)
    -- are attempt-scoped and a retry after a crash never double-posts or re-dispatches.
    attempt_no              INTEGER NOT NULL DEFAULT 0 CHECK (attempt_no >= 0),

    -- Integration saga checkpoints (ADR §4bis). Written by KT-320; declared here
    -- so the integration is replay-safe against the real Git refs at boot.
    --   candidate_target_sha : parent tip the candidate was built on (CAS anchor)
    --   candidate_merge_sha  : the exact validated commit
    --   integrated_sha       : what the parent actually became
    --   backup_ref           : refs/kronn-backup/<KT-ref> written before apply
    candidate_target_sha    TEXT,
    candidate_merge_sha     TEXT,
    integrated_sha          TEXT,
    backup_ref              TEXT,

    -- Bookkeeping. `blocked_reason` explains a non-terminal Blocked hold (free
    -- text, for humans); `outcome_reason` explains a terminal/Escalated resolution.
    blocked_reason          TEXT,
    outcome_reason          TEXT,
    -- Structured discriminant for a non-terminal `Blocked` hold (KT-328). Consumers
    -- classify the hold on THIS code, never on `blocked_reason` prose (KT-334 owns
    -- the attention-center split). Deliberately NO SQL CHECK: the domain is owned by
    -- the Rust `BlockedReasonCode` enum (a strict parse on read IS the domain guard),
    -- so KT-322/KT-334 can add codes without a 128. V1 codes:
    -- `awaiting_worker_acceptance` (normal — the CLI worker will act) and
    -- `worker_session_committed_elsewhere` (needs a human decision).
    blocked_reason_code     TEXT,
    -- Idempotent launch: a retry with the same key returns the existing row.
    idempotency_key         TEXT,

    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    finished_at             TEXT,

    -- Worker identity is all-or-nothing (KT-318 §B). Once a kind is set:
    --   • `worker_agent_type` is mandatory for ALL three kinds — MessageTarget
    --     carries a non-optional `agent_type`, so a kind without it could never be
    --     rebuilt into a routable target;
    --   • a `cli` worker additionally requires the exact `worker_cli_session_id`;
    --   • the two native kinds forbid a session id.
    -- No partial identity row the dispatcher could not resolve can exist.
    CHECK (
        worker_target_kind IS NULL
        OR (
            worker_agent_type IS NOT NULL
            AND (
                (worker_target_kind = 'cli' AND worker_cli_session_id IS NOT NULL)
                OR (worker_target_kind IN ('discussion_agent', 'agent')
                    AND worker_cli_session_id IS NULL)
            )
        )
    )
);

-- One active (non-terminal) execution per task (ADR §6 concurrency; DoD-1).
-- A new execution is allowed only once the previous one is terminal.
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_executions_one_active_per_task
    ON task_executions(task_id)
    WHERE status NOT IN ('Done', 'Failed', 'Cancelled');

-- Idempotent launch key, scoped per task: replaying the same launch is a no-op.
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_executions_idempotency
    ON task_executions(task_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- Lineage lookups (DoD-4): the whole chain is queryable without rebuilding it
-- from chat messages.
CREATE INDEX IF NOT EXISTS idx_task_executions_run
    ON task_executions(orchestration_run_id);
CREATE INDEX IF NOT EXISTS idx_task_executions_task
    ON task_executions(task_id);
CREATE INDEX IF NOT EXISTS idx_task_executions_parent_disc
    ON task_executions(parent_discussion_id);
CREATE INDEX IF NOT EXISTS idx_task_executions_sub_disc
    ON task_executions(sub_discussion_id)
    WHERE sub_discussion_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_task_executions_status
    ON task_executions(status);

-- ─────────────────────────────────────────────────────────────────────────────
-- TaskExecution event journal (ADR §3; DoD-3).
--
-- The audit source of truth: every authorized transition is journaled with an
-- attributed actor. Autonomous backend transitions (claim, integrate, reconcile)
-- are attributable via the `backend`/`system` actor kinds a chat message cannot
-- forge.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS task_execution_events (
    id                  TEXT PRIMARY KEY,
    task_execution_id   TEXT NOT NULL
                            REFERENCES task_executions(id) ON DELETE CASCADE,
    action              TEXT NOT NULL,
    from_status         TEXT,
    to_status           TEXT,
    actor_kind          TEXT NOT NULL
                            CHECK (actor_kind IN ('human', 'agent', 'backend', 'system')),
    actor_id            TEXT,
    changes_json        TEXT NOT NULL DEFAULT '{}',
    source_message_id   TEXT REFERENCES messages(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_execution_events_exec
    ON task_execution_events(task_execution_id, created_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- Validation runs (ADR §6; DoD from KT-316 §6).
--
-- A DEDICATED table, not `quick_exec_runs`: orchestration validations must not
-- pollute the Quick Exec ROI/history stream. Reuses the QE executor / allowlist
-- / timeout / result shape at the code layer; `quick_exec_id` records provenance
-- when a validation is sourced from a saved QE. `exit_code` IS the verdict —
-- NULL (process died / never started) is never a pass, closing "exit 0 always".
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS task_execution_validation_runs (
    id                  TEXT PRIMARY KEY,
    task_execution_id   TEXT NOT NULL
                            REFERENCES task_executions(id) ON DELETE CASCADE,
    -- The exact commit the validation describes (§4bis step 2). NULL only for a
    -- pre-candidate probe; a NULL candidate is never treated as validated.
    candidate_merge_sha TEXT,
    command             TEXT NOT NULL,
    -- NULL when the process died on a signal or never started. Never coerced to
    -- 0 — an unknown exit must not read as a clean one.
    exit_code           INTEGER,
    duration_ms         INTEGER,
    -- Bounded summary; large logs belong in an artifact, not the DB.
    output              TEXT,
    -- Set when the validation came from a saved Quick Exec (provenance only).
    quick_exec_id       TEXT REFERENCES quick_execs(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_execution_validation_runs_exec
    ON task_execution_validation_runs(task_execution_id, created_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- CLI worker control offers (KT-328 — child-bound CLI handshake).
--
-- A joined CLI worker cannot be launched atomically like a native one: an active
-- session owns exactly one discussion (060_discussion_sessions), and
-- `wait_for_peer` only wakes a session already in the target room. So provisioning
-- a `cli` worker publishes a durable CONTROL OFFER addressed to the exact target
-- session in the ORIGIN room; only that session may accept (server-derived
-- identity), which then transfers the session/binding to the sub-discussion and
-- runs the final checkpoint. The offer id is the opaque handle exposed to the
-- agent — never a raw kr-join token (DoD-2).
--
-- Expiry is LAZY: `expires_at` is evaluated AT READ (any accept/reoffer/provision
-- that sees a past-deadline pending|accepting offer treats it as expired in one
-- CAS). This keeps KT-328 independent of the native-agent wake infra (KT-335) that
-- does not exist yet — no scheduler, no timer zombie.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS task_execution_worker_offers (
    id                      TEXT PRIMARY KEY,
    task_execution_id       TEXT NOT NULL
                                REFERENCES task_executions(id) ON DELETE CASCADE,
    -- Attempt-scoped like the brief/dispatch dedupe keys (ADR §5): a KT-319 review
    -- re-offer for the same execution uses the next attempt.
    attempt_no              INTEGER NOT NULL DEFAULT 0 CHECK (attempt_no >= 0),
    -- The EXACT joined CLI session the offer targets. FK is ON DELETE CASCADE, NOT
    -- RESTRICT: sessions ARE hard-deleted. discussion_sessions.disc_id is itself
    -- ON DELETE CASCADE (060), so deleting ANY room the session has moved into
    -- (post-acceptance transfer, then reused elsewhere) wipes the session row. A
    -- RESTRICT here would then make THAT room undeletable on a raw FK error while
    -- the offer's own origin/child rooms stay alive. An offer whose target session
    -- no longer exists is meaningless, and the durable "who accepted" trace lives in
    -- task_execution_events, not in this row — so the offer is swept with the
    -- session. Insert-time integrity is still enforced (cannot target a nonexistent
    -- session), which is the only guarantee this FK owes.
    target_cli_session_id   INTEGER NOT NULL
                                REFERENCES discussion_sessions(id) ON DELETE CASCADE,
    -- The room the target session currently lives in (offer posted here) and the
    -- sub-discussion it will be attached to on acceptance.
    origin_discussion_id    TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    child_discussion_id     TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    status                  TEXT NOT NULL DEFAULT 'pending'
                                CHECK (status IN (
                                    'pending', 'accepting', 'accepted',
                                    'declined', 'expired', 'cancelled'
                                )),
    -- Evaluated at read (lazy expiry). NULL = no deadline.
    expires_at              TEXT,
    -- The opaque control-offer message posted in the origin room (provenance).
    offer_message_id        TEXT REFERENCES messages(id) ON DELETE SET NULL,
    reason                  TEXT,
    accepted_at             TEXT,
    declined_at             TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

-- One live offer per execution+attempt: an idempotent re-post reattaches the
-- existing row, and two racing offers for one attempt are impossible.
CREATE UNIQUE INDEX IF NOT EXISTS idx_worker_offers_one_active_per_attempt
    ON task_execution_worker_offers(task_execution_id, attempt_no)
    WHERE status IN ('pending', 'accepting');

-- A target session can hold at most one live (pending|accepting) offer at a time:
-- it can never be double-committed to two executions concurrently.
CREATE UNIQUE INDEX IF NOT EXISTS idx_worker_offers_one_active_per_session
    ON task_execution_worker_offers(target_cli_session_id)
    WHERE status IN ('pending', 'accepting');

CREATE INDEX IF NOT EXISTS idx_worker_offers_execution
    ON task_execution_worker_offers(task_execution_id, created_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- Lineage columns on discussion_workspaces (ADR §4; DoD-4).
--
-- A managed child worktree records its parent discussion, the base SHA it was
-- pinned at, and the TaskExecution that owns it, so KT-318's backend "managed
-- writer" can identify and reconcile the worktree it created without a joined
-- CLI session (audit gap #3). SQLite ADD COLUMN allows a nullable REFERENCES
-- column; the FK is enforced going forward.
-- ─────────────────────────────────────────────────────────────────────────────
ALTER TABLE discussion_workspaces
    ADD COLUMN parent_discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL;
ALTER TABLE discussion_workspaces
    ADD COLUMN base_sha TEXT;
ALTER TABLE discussion_workspaces
    ADD COLUMN task_execution_id TEXT REFERENCES task_executions(id) ON DELETE SET NULL;

-- One managed workspace per TaskExecution (KT-318). UNIQUE so a compensable
-- retry re-attaches its own row instead of creating a second; also the
-- `ON CONFLICT(task_execution_id)` upsert target for the backend managed writer,
-- which the `(disc_id, session_pk)` external index cannot serve (managed rows
-- carry session_pk IS NULL and escape it).
CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_workspaces_task_execution
    ON discussion_workspaces(task_execution_id)
    WHERE task_execution_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_discussion_workspaces_parent_disc
    ON discussion_workspaces(parent_discussion_id)
    WHERE parent_discussion_id IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Widen planning_task_events.actor_kind to admit `backend`/`system` (ADR §0
-- Adapt, §8 — a shared prerequisite surfaced in KT-317).
--
-- When a TaskExecution closes its task on integration (KT-320), the backend
-- writes a planning event attributed to itself. The current CHECK only allows
-- ('human','agent'). SQLite cannot ALTER a CHECK, so rebuild the table
-- data-preservingly. Nothing references planning_task_events, so the drop/rename
-- is safe; the referenced planning_tasks/messages rows are untouched.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE planning_task_events_new (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    action            TEXT NOT NULL,
    actor_kind        TEXT NOT NULL
                          CHECK (actor_kind IN ('human', 'agent', 'backend', 'system')),
    actor_id           TEXT,
    changes_json       TEXT NOT NULL DEFAULT '{}',
    source_message_id  TEXT REFERENCES messages(id) ON DELETE SET NULL,
    created_at         TEXT NOT NULL
);

INSERT INTO planning_task_events_new
    (id, task_id, action, actor_kind, actor_id, changes_json, source_message_id, created_at)
SELECT
    id, task_id, action, actor_kind, actor_id, changes_json, source_message_id, created_at
FROM planning_task_events;

DROP TABLE planning_task_events;
ALTER TABLE planning_task_events_new RENAME TO planning_task_events;

CREATE INDEX IF NOT EXISTS idx_planning_task_events_task
    ON planning_task_events(task_id, created_at);
