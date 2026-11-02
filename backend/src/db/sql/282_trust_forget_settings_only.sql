-- KT-1492: the delete triggers of 279/280 also flipped the journal's `active`
-- flag of what the deleted room or workflow had been granted. A flag flipped in
-- SQL cannot take the approval back from its object (a Quick Exec, a workflow
-- step, a room scope grant) and it matched only `source = room`, never a grant
-- carrying several rooms in `discussion_ids`: any deletion that did not go
-- through `trust::forget_scope` left an approval no later revoke could find.
-- The triggers now only drop the setting. A grant stays active in the journal
-- and is judged by the next `trust::withdraw` against the level then in force
-- for its origin, which also takes the approval back from its object.
DROP TRIGGER IF EXISTS trust_forget_room;
DROP TRIGGER IF EXISTS trust_forget_workflow;

CREATE TRIGGER trust_forget_room AFTER DELETE ON discussions
BEGIN
    DELETE FROM trust_settings WHERE scope = 'room' AND scope_id = OLD.id;
END;

CREATE TRIGGER trust_forget_workflow AFTER DELETE ON workflows
BEGIN
    DELETE FROM trust_settings WHERE scope = 'workflow' AND scope_id = OLD.id;
END;
