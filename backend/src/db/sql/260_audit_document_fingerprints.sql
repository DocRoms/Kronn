-- Local proof of the documentation checked by a completed run, then its
-- linked validation agent. Historical runs deliberately remain unproven.
ALTER TABLE audit_runs ADD COLUMN final_doc_fingerprints_json TEXT;
