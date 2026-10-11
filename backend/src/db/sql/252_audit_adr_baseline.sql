-- KT-1174: `docs/decisions.md` as an audit run found it. Only the ADRs in force
-- in this text may settle a TD at the end of the run; a resume inherits its
-- predecessor's. NULL on rows written before this column existed.
ALTER TABLE audit_runs ADD COLUMN adr_baseline TEXT;
