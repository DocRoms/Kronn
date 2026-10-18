-- 269 (KT-1353): what the documentary gate let through or fixed during the run,
-- kept with the run so the audit timeline can show it after the stream is gone.
--   documentary_notes  JSON {"preexisting":[{path,message}],"autofixes":[{step,
--                      path,kind,from,to}]}; NULL when there was nothing to say.
ALTER TABLE audit_runs ADD COLUMN documentary_notes TEXT;
