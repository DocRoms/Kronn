ALTER TABLE agent_dispatch_jobs ADD COLUMN progress_phase TEXT;
ALTER TABLE agent_dispatch_jobs ADD COLUMN progress_detail TEXT;
ALTER TABLE agent_dispatch_jobs ADD COLUMN last_progress_at TEXT;

