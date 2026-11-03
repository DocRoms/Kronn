# Media generations are bounded per human request (KT-1481)

A media result wakes the agent through a new dispatch (`wake_media_requester`,
`backend/src/api/shared_runs.rs`). That dispatch is not a new human request, so
`backend/src/api/media_request_guard.rs` ties every agent generation to the
**request root**: the dispatch of the human message at the base of the wake chain
(`request_root` follows `agent-resume-result:` trigger messages back through
`agent_resume_jobs.source_dispatch_job_id`). The root is stored on
`media_jobs.request_root_id` (migration 284).

`authorize` runs before `media_generate` creates anything and refuses, for one
root: a turn whose dispatch was cancelled (Stop); any generation after a
failed / timed-out / cancelled job; more than `MAX_JOBS_PER_REQUEST`; a
notification turn that does not build on the notified result; another
connection than the one already used for that modality (KT-1420 — a provider
change needs the human); fewer references than an earlier job of the request.

Stop (`stop_agent`) clears `media_jobs.wake_agent_json` for the discussion in
the same transaction as the dispatch cancellation; the publisher also drops a
wake whose source dispatch is Cancelled. Both read the database, so a restart
changes nothing.

A new human message starts a new root: the way to ask again after a failure, a
Stop or the cap is to say so in the discussion. Nothing else resets a root.
