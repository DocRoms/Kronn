-- KT-1174: `docs/decisions.md` as an audit run found it. Only the ADRs in force
-- in this text may settle a TD at the end of the run; a resume inherits its
-- predecessor's. NULL on rows written before this column existed.
ALTER TABLE audit_runs ADD COLUMN adr_baseline TEXT;
-- The `Decision record` lines of the TD sheets at the same moment, one
-- `TD-id ADR-id` per line: only those pairs may settle a sheet by its record.
ALTER TABLE audit_runs ADD COLUMN adr_record_baseline TEXT;
