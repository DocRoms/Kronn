-- KT-195 — recorded Quick Exec runs, so a mechanical result is established once.
--
-- The saving Quick Exec makes is per RUN: a command executes and returns a
-- bounded summary instead of a full log. This table adds the second saving: a
-- run that already answered the question is not executed again.
--
-- Which makes reuse a correctness question, not a cache question. The rule the
-- schema is shaped around: only a CONCLUSIVE run may be reused. A timeout, a
-- cancellation and a partial log all produced no findings, and reusing one of
-- them is exactly how "nobody checked" becomes "checked, nothing found". Hence
-- `findings_complete` and `head_sha` sit next to `status` — all three are read
-- before a stored result is handed back.
--
-- Deliberately NOT stored here: stdout and stderr. They live in the artifact file
-- the row points at. Putting them in the DB would rebuild, inside the thing that
-- gets queried, the bulk this exists to keep out of a context.

CREATE TABLE IF NOT EXISTS quick_exec_runs (
    id                  TEXT PRIMARY KEY,
    -- NULL for an ad-hoc spec. A template id says the command line was reviewed
    -- once and reused, which is what makes the run comparable across time.
    template_id         TEXT,
    -- Identity of the work: binary, argv, cwd, stdin. NOT the timeout — a retry
    -- with a longer deadline is the same work.
    spec_fingerprint    TEXT    NOT NULL,
    -- The tree the result describes. NULL means unpinned, and an unpinned run is
    -- never reused: we cannot know what it was true of.
    head_sha            TEXT,

    status              TEXT    NOT NULL,
    -- NULL when the process died on a signal or never started. Never 0 — an
    -- unknown exit code must not read as a clean one.
    exit_code           INTEGER,
    summary             TEXT    NOT NULL,
    -- JSON arrays. Their emptiness only means "nothing found" when
    -- findings_complete is 1.
    failed_tests        TEXT    NOT NULL,
    diagnostics         TEXT    NOT NULL,

    artifact_path       TEXT,
    artifact_bytes      INTEGER,
    artifact_truncated  INTEGER NOT NULL DEFAULT 0,
    -- Whether the lists above can be treated as exhaustive.
    findings_complete   INTEGER NOT NULL DEFAULT 0,

    duration_ms         INTEGER NOT NULL,
    stdout_bytes        INTEGER NOT NULL,
    stderr_bytes        INTEGER NOT NULL,
    created_at          TEXT    NOT NULL
);

-- The lookup the idempotency check makes: "has this exact work already been done
-- against this exact tree?"
CREATE INDEX IF NOT EXISTS idx_quick_exec_runs_identity
    ON quick_exec_runs(spec_fingerprint, head_sha, created_at DESC);

-- What a run is evidence FOR. A separate table because one run answers several
-- questions at once: a full suite settles many findings, and a finding may be
-- settled by more than one run.
CREATE TABLE IF NOT EXISTS quick_exec_evidence (
    run_id      TEXT NOT NULL
                REFERENCES quick_exec_runs(id) ON DELETE CASCADE,
    -- 'task' or 'review_finding'. Not a foreign key: the two targets live in
    -- different tables, and a link that outlives a deleted target is still a
    -- true statement about what was run.
    target_kind TEXT NOT NULL,
    target_id   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    -- Re-attaching the same run to the same target is a no-op, so replaying a
    -- publish cannot invent extra evidence.
    PRIMARY KEY (run_id, target_kind, target_id)
);

CREATE INDEX IF NOT EXISTS idx_quick_exec_evidence_target
    ON quick_exec_evidence(target_kind, target_id);
