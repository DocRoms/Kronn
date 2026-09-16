-- Reclaim the duplicated workflow step outputs held in shared_runs.
--
-- `shared_runs.result_json` mirrored `workflow_runs.step_results_json` in full,
-- outputs included. Measured on a live instance: 2 620 MB of a 7 200 MB
-- database, against 41 MB for every message ever sent. The card that reads this
-- row renders progress and step names and links to the run itself for the rest,
-- so the copy was never read.
--
-- Only rows whose `workflow_runs` twin still exists are blanked: a handful of
-- runs outlive their workflow (ON DELETE CASCADE removes the run row, not this
-- one) and for those this is the only surviving copy. They are left whole.
--
-- `output` is emptied rather than removed. `StepResult::output` has no serde
-- default, so a missing key fails the whole `Vec<StepResult>` decode and the
-- step counters a listing needs would silently come back empty.
UPDATE shared_runs
SET result_json = json_set(
        result_json,
        '$.steps',
        (SELECT json_group_array(json_set(value, '$.output', ''))
         FROM json_each(shared_runs.result_json, '$.steps'))
    )
WHERE kind = 'workflow'
  AND json_valid(result_json)
  AND json_type(result_json, '$.steps') = 'array'
  AND EXISTS (SELECT 1 FROM workflow_runs wr WHERE wr.id = shared_runs.id);
