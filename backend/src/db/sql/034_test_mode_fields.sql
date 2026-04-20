-- Test mode support — lets the user "switch" their main repo to a
-- discussion's worktree branch to try the agent's code in their IDE,
-- then come back to where they were.
--
-- `test_mode_restore_branch` : branch the main repo was on BEFORE entering
--   test mode. NULL means we're not in test mode. Used by `exit` to
--   checkout back to the user's previous workspace.
--
-- `test_mode_stash_ref` : if the main repo had uncommitted changes at
--   enter time AND the user opted in to auto-stash, this holds the stash
--   message used (e.g. "kronn:auto-<disc_id>") so `exit` can
--   `git stash pop` the right one. NULL if the repo was clean.
ALTER TABLE discussions ADD COLUMN test_mode_restore_branch TEXT;
ALTER TABLE discussions ADD COLUMN test_mode_stash_ref TEXT;
