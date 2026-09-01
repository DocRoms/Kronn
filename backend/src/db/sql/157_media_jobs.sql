-- Media generation jobs (image / video) on HTTP providers.
--
-- Video generation is asynchronous on the provider side (submit → poll →
-- download) and takes ~100 s even for the cheapest 5 s / 480p clip, so the job
-- must survive a backend restart: a generation that is already billed can
-- never be lost. Follows the `agent_resume_jobs` pattern (due-selection +
-- atomic claim + orphan reclaim) rather than the delegated-task lifecycle,
-- which assumes a workspace, a review and a commit.
CREATE TABLE IF NOT EXISTS media_jobs (
    id TEXT PRIMARY KEY,
    -- Modality lives on the job, not in a separate kind: the execution family
    -- is the same, only the output differs.
    modality TEXT NOT NULL CHECK (modality IN ('image','video')),
    status TEXT NOT NULL CHECK (status IN ('pending','running','completed','failed','cancelled','timed_out')),
    connection_id TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    -- Requested parameters (duration, resolution, aspect_ratio, audio…). The
    -- provider does NOT guarantee them: a "480p 16:9" request came back as
    -- 864x496, so the rendered dimensions below are read from the file.
    params_json TEXT,
    discussion_id TEXT REFERENCES discussions(id) ON DELETE CASCADE,
    message_id TEXT,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    -- Stamped BEFORE the billable POST leaves. If the process dies between
    -- that POST and the handle being stored, recovery finds this mark with no
    -- handle and REFUSES to resubmit: the provider may already be generating
    -- and charging, and a blind retry would pay twice.
    submit_attempted_at TEXT,
    -- Provider identifiers: the job handle used for polling, and the
    -- generation id used to reconcile with provider-side accounting.
    provider_job_id TEXT,
    provider_generation_id TEXT,
    -- Resulting asset, once downloaded server-side and persisted as a
    -- context file. The provider URL is never stored.
    context_file_id TEXT,
    rendered_width INTEGER,
    rendered_height INTEGER,
    rendered_duration_ms INTEGER,
    -- Cost as DECLARED by the provider. Never recomputed from rate × duration:
    -- a measured 5.04 s clip billed 0.0708932 USD where the rate implied
    -- 0.0678, and the usage payload carries no token count at all.
    cost_usd REAL,
    is_byok INTEGER NOT NULL DEFAULT 0,
    -- Bounded, actionable error. No secrets, no raw payload, no signed URL.
    last_error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    scheduled_at TEXT NOT NULL,
    deadline_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Due-job selection: the worker asks for pending rows whose time has come.
CREATE INDEX IF NOT EXISTS idx_media_jobs_due ON media_jobs(status, scheduled_at);
CREATE INDEX IF NOT EXISTS idx_media_jobs_discussion ON media_jobs(discussion_id, created_at DESC);

-- `shared_runs.kind` gains 'media'. SQLite cannot ALTER a CHECK constraint, so
-- the table is rebuilt; existing rows, indexes and foreign keys are preserved.
PRAGMA foreign_keys=OFF;

CREATE TABLE shared_runs_new (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('quick_prompt','quick_api','quick_exec','workflow','media')),
    source_id TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('preflight_failed','queued','running','success','failed','cancelled','timeout')),
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER,
    result_json TEXT,
    diagnostic TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO shared_runs_new
    SELECT id, kind, source_id, project_id, discussion_id, status, started_at,
           finished_at, duration_ms, result_json, diagnostic, created_at, updated_at
    FROM shared_runs;

DROP TABLE shared_runs;
ALTER TABLE shared_runs_new RENAME TO shared_runs;

CREATE INDEX IF NOT EXISTS idx_shared_runs_discussion ON shared_runs(discussion_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_shared_runs_source ON shared_runs(kind, source_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_shared_runs_project ON shared_runs(project_id, created_at DESC);

PRAGMA foreign_keys=ON;
