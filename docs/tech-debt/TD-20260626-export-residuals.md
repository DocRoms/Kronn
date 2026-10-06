# TD-20260626-export-residuals

- **ID**: TD-20260626-export-residuals
- **Area**: Backend (whole-database export)
- **Problem (fact)**: the export model carries projects, discussions,
  workflows, MCP data, skills, directives, profiles, contacts, Quick
  Prompts/APIs/Execs, learnings, Quick Prompt versions and learning
  rejections, but no uploaded context files: their rows and on-disk blobs
  are missing from the ZIP. `[src: file: backend/src/models/db.rs:84-128]`
- **Why we can't fix now (constraint)**: blobs can be large and live outside
  the database; the archive format and the import limits must account for
  them.
- **Impact**: correctness (an export does not restore attached files).
- **Where (pointers)**: `backend/src/models/db.rs` (`DbExport`), the
  `/api/config/export` and `/api/config/import` handlers.
- **Suggested direction (non-binding)**: an optional blob section in the ZIP,
  with sizes counted against the import body limit. Secrets stay stripped by
  design.
- **Next step**: create ticket.
