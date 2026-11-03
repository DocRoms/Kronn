-- KT-1481 — the human request a media generation belongs to.
--
-- A wake (a media result, a scheduled resume) re-enters the agent through a new
-- dispatch, so "one request" used to be invisible to Kronn: each turn looked
-- like a fresh ask, and an agent could generate again after every notification
-- — ten jobs for one sentence. `request_root_id` is the dispatch of the human
-- message at the base of that chain; every job launched from it, however many
-- notifications later, carries the same value.
--
-- Nullable: UI launches and jobs created before this migration have no agent
-- request behind them.
ALTER TABLE media_jobs ADD COLUMN request_root_id TEXT;
CREATE INDEX IF NOT EXISTS idx_media_jobs_request_root ON media_jobs(request_root_id)
    WHERE request_root_id IS NOT NULL;
