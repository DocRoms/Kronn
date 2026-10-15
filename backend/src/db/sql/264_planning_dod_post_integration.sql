-- KT-1254 — a DoD item can be a principal LIVE check made after integration.
--
-- Some items can only be verified once the code is integrated and the backend
-- restarted (a replay on a real framing). `verification = 'post_integration'`
-- lets approve integrate with such an item pending: the task then stays open
-- and closes when the principal ticks the item with `evidence`.
ALTER TABLE planning_task_dod_items
    ADD COLUMN verification TEXT NOT NULL DEFAULT 'worker'
    CHECK (verification IN ('worker', 'post_integration'));
ALTER TABLE planning_task_dod_items ADD COLUMN evidence TEXT;
