-- How many times a resume job has been released back to the queue.
--
-- `release_after_error` put the job back as Pending with a 10 s delay and no
-- counter, so a job that cannot validate — a discussion that moved, a command
-- whose shape no longer parses — was re-attempted 8 640 times a day while the
-- room stayed silent and nothing was logged. The retry is right; the absence
-- of an end to it is not.
ALTER TABLE agent_resume_jobs ADD COLUMN release_attempts INTEGER NOT NULL DEFAULT 0;
