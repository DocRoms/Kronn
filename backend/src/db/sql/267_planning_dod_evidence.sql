-- 267 (KT-1262): the evidence a DoD item was ticked on.
--   evidence  what the principal (or, for a review that supplied none, the
--             delivery manifest) recorded when the item was verified met. Set
--             when an approve integrates, cleared when the item is unticked.
--             NULL for an item ticked by hand or never ticked.
ALTER TABLE planning_task_dod_items ADD COLUMN evidence TEXT;
