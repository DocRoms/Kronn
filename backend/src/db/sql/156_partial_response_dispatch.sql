-- Bind an in-flight response checkpoint to the exact durable dispatch that
-- produced it. A discussion may have one Running reply and newer Pending
-- follow-ups, so "latest active job" is not a safe recovery identity.
ALTER TABLE discussions ADD COLUMN partial_response_dispatch_id TEXT;
ALTER TABLE discussions ADD COLUMN partial_response_trigger_message_id TEXT;
ALTER TABLE discussions ADD COLUMN partial_response_connection_id TEXT;

-- Upgrade adoption: before the backend restart reconciler turns Running jobs
-- into Pending retries, bind a legacy checkpoint when exactly one Running job
-- can possibly own it. Ambiguous rooms remain NULL rather than being guessed.
UPDATE discussions
SET partial_response_dispatch_id = (
        SELECT job.id FROM agent_dispatch_jobs job
        WHERE job.discussion_id = discussions.id AND job.status = 'Running'
        LIMIT 1
    ),
    partial_response_trigger_message_id = (
        SELECT job.trigger_message_id FROM agent_dispatch_jobs job
        WHERE job.discussion_id = discussions.id AND job.status = 'Running'
        LIMIT 1
    ),
    partial_response_connection_id = (
        SELECT job.connection_id FROM agent_dispatch_jobs job
        WHERE job.discussion_id = discussions.id AND job.status = 'Running'
        LIMIT 1
    )
WHERE partial_response IS NOT NULL
  AND partial_response_dispatch_id IS NULL
  AND (
      SELECT COUNT(*) FROM agent_dispatch_jobs job
      WHERE job.discussion_id = discussions.id AND job.status = 'Running'
  ) = 1;
