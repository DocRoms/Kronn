-- KT-874: retain provider metadata needed to identify a live replacement for
-- a disappeared Claude CLI alias. Both columns are nullable because other
-- discovery protocols do not necessarily expose either value.
ALTER TABLE model_catalog_entries ADD COLUMN resolved_model TEXT;
ALTER TABLE model_catalog_entries ADD COLUMN description TEXT;
