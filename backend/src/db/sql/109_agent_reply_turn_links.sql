-- Native replies are appended when they finish, which may be after a newer
-- User turn. Preserve the immutable sort_order ledger and attach the reply to
-- the User message that created its durable dispatch instead.
UPDATE messages
SET reply_to_message_id = (
    SELECT agent_dispatch_jobs.trigger_message_id
    FROM agent_dispatch_jobs
    WHERE agent_dispatch_jobs.id = messages.agent_dispatch_job_id
)
WHERE role IN ('Agent', 'System')
  AND reply_to_message_id IS NULL
  AND agent_dispatch_job_id IS NOT NULL
  AND EXISTS (
      SELECT 1
      FROM agent_dispatch_jobs
      WHERE agent_dispatch_jobs.id = messages.agent_dispatch_job_id
        AND agent_dispatch_jobs.discussion_id = messages.discussion_id
  );
