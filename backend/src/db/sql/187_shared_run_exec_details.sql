-- keep measured Quick Exec diagnostics separately from business data.
-- Existing runs have no recoverable process metadata; NULL stays unknown.
ALTER TABLE shared_runs ADD COLUMN exec_details_json TEXT;
