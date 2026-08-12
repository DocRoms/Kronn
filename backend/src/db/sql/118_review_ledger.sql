-- KT-196 — a review ledger keyed to the SHA it was established against.
--
-- The failure this replaces is the "final verdict, then a new symptom" loop seen
-- in the reference discussions: a reviewer declares a PR clean, a new comment
-- arrives, and the whole review is redone — re-reading the same diff, re-deriving
-- the same conclusions, and re-publishing findings that were already answered.
--
-- Two ideas carry the fix.
--
-- FINDINGS ARE KEYED TO A CAUSE, NOT TO A COMMENT. Five comments about the same
-- unwrapped error are one finding with five symptoms. Keying on the comment id
-- alone made them five, which is why the same thing kept coming back.
--
-- THE LEDGER IS PINNED TO A HEAD SHA. When the SHA moves, only the surfaces the
-- diff actually touched need replaying; everything else is already settled, and
-- its evidence is still valid. That is what makes a re-review a delta instead of
-- a repeat.
--
-- Deliberately NOT stored: the diff, the comment bodies, the agent's reasoning.
-- The ledger holds what a later run needs to avoid redoing work — status,
-- evidence, the test that proves it — and points at the rest by id. Storing the
-- material would rebuild the bulk this exists to remove.

CREATE TABLE IF NOT EXISTS review_findings (
    id                     TEXT PRIMARY KEY,
    -- Which PR, and the head it was established against. A finding recorded at
    -- one SHA is not automatically true at the next: `settled_at_sha` says where
    -- its evidence was gathered.
    repo                   TEXT    NOT NULL,
    pr_number              INTEGER NOT NULL,
    settled_at_sha         TEXT    NOT NULL,

    -- The identity that makes dedup possible. Same fingerprint = same cause,
    -- however many comments describe it.
    root_cause_fingerprint TEXT    NOT NULL,
    -- Where it lives. Range as start/end lines so a SHA change can ask "did the
    -- diff touch this?" without re-reading anything.
    path                   TEXT,
    line_start             INTEGER,
    line_end               INTEGER,
    -- The concrete failure, in one sentence. Kept because a fingerprint alone
    -- cannot be reviewed by a human.
    scenario               TEXT    NOT NULL,

    status                 TEXT    NOT NULL,
    -- What makes the status checkable: a command output, a test name, a commit.
    -- NULL means unproven, which must stay distinguishable from proven-clean.
    evidence               TEXT,
    proving_test           TEXT,
    fixing_commit          TEXT,
    created_at             TEXT    NOT NULL,
    updated_at             TEXT    NOT NULL
);

-- One finding per cause per PR. This UNIQUE constraint IS the dedup: a second
-- symptom of the same cause updates the row instead of adding one.
CREATE UNIQUE INDEX IF NOT EXISTS idx_review_findings_cause
    ON review_findings(repo, pr_number, root_cause_fingerprint);

-- The access path for a delta re-review: "what is on record for this PR?"
CREATE INDEX IF NOT EXISTS idx_review_findings_pr
    ON review_findings(repo, pr_number);

-- Every comment that described a finding. Separate table because the count
-- matters: five symptoms of one cause is a signal about the review, and
-- collapsing them into the finding row would erase it.
CREATE TABLE IF NOT EXISTS review_finding_symptoms (
    finding_id        TEXT NOT NULL
                      REFERENCES review_findings(id) ON DELETE CASCADE,
    -- The upstream comment. UNIQUE with finding_id so replaying a webhook cannot
    -- count the same comment twice.
    source_comment_id TEXT NOT NULL,
    observed_at_sha   TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    PRIMARY KEY (finding_id, source_comment_id)
);

CREATE INDEX IF NOT EXISTS idx_review_symptoms_comment
    ON review_finding_symptoms(source_comment_id);
