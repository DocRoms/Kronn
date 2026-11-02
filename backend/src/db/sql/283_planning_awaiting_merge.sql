-- KT-1519 — a task whose delivery was integrated into a branch that is not the
-- default branch (a campaign or feature branch) is NOT done: it is integrated
-- and awaits the merge. `awaiting_merge_branch` names that branch and
-- `awaiting_merge_sha` the integrated commit; both are NULL otherwise. The task
-- closes (`done`) once that commit is reachable from the default branch, or when
-- a human closes it.
ALTER TABLE planning_tasks ADD COLUMN awaiting_merge_branch TEXT;
ALTER TABLE planning_tasks ADD COLUMN awaiting_merge_sha TEXT;
