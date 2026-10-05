-- KT-851: the projects a workflow serves besides its home project, as JSON
-- (`{"type":"All"}` or `{"type":"Projects","project_ids":[…]}`). NULL keeps
-- the single-project behaviour.
ALTER TABLE workflows ADD COLUMN project_scope_json TEXT;
