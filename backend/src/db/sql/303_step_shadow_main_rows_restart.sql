-- KT-2029 — the main execution's row of `step_shadow_runs` (`shadow_id` = '')
-- is identity only until a shadow merge closes it: its output and metrics live
-- in the run's step result. The boot interruption used to mark it interrupted
-- with the shadows, and the run view then read that empty, finished row
-- instead of the step result. The boot step now leaves it alone; the rows it
-- already flipped (empty output, the restart's own stopped text) go back to
-- identity only. A main row a merge closed holds its own output: untouched.
UPDATE step_shadow_runs
   SET status = 'running', stopped = NULL, finished_at = NULL
 WHERE shadow_id = ''
   AND status = 'interrupted'
   AND output = ''
   AND stopped = 'Kronn restarted while this shadow was running';
