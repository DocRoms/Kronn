-- 263 (KT-1211): cancel and failed steps keep the docs whole.
--   cancelled_at_step  the step in flight when the user cancelled the run (0 =
--                      before step 1); NULL on a run nobody cancelled. A run
--                      cancelled after a completed step is settled Interrupted
--                      and stays resumable: this column is its "cancelled" flag.
--   resume_drift       what moved in the sources between the run a resume
--                      continues and the resume itself (was a one-off toast).
--   agent_pgid /       the process group of the step's CLI agent and the start
--   agent_started_marker  stamp of its leader, so a boot after a crash can kill
--                      an agent the dead backend left running (never a reused pid).
ALTER TABLE audit_runs ADD COLUMN cancelled_at_step INTEGER;
ALTER TABLE audit_runs ADD COLUMN resume_drift TEXT;
ALTER TABLE audit_runs ADD COLUMN agent_pgid INTEGER;
ALTER TABLE audit_runs ADD COLUMN agent_started_marker TEXT;

-- What became of the step's target when the step did not succeed:
-- 'restored' (put back as it was before the step) or 'partial' (kept, marked).
ALTER TABLE audit_run_steps ADD COLUMN target_state TEXT;

-- The target of a step as it was before the agent ran, kept while the step is
-- not done: a cancel or a crash restores from it, a refused acceptance shows
-- the diff against it. `existed = 0`: the file was absent.
CREATE TABLE IF NOT EXISTS audit_step_snapshots (
    audit_run_id TEXT NOT NULL,
    step_index   INTEGER NOT NULL,
    target_file  TEXT NOT NULL,
    existed      INTEGER NOT NULL,
    content      TEXT,
    PRIMARY KEY (audit_run_id, step_index)
);

-- Runs cancelled before this migration: the step they were killed in is the
-- first one that never ended (0 when none started).
UPDATE audit_runs SET cancelled_at_step = COALESCE((
    SELECT MIN(step_index) FROM audit_run_steps s
     WHERE s.audit_run_id = audit_runs.id AND s.ended_at IS NULL
), 0) WHERE status = 'Cancelled' AND cancelled_at_step IS NULL;
