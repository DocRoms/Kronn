# TD-20260715-parse-dt-fallback-drift

- **ID**: TD-20260715-parse-dt-fallback-drift
- **Area**: Backend / DB layer
- **Problem (fact)**: `parse_dt()` only accepts RFC3339
  (`backend/src/db/workflows.rs:10`), but 5 workflow rows carry SQLite-style
  datetimes (`YYYY-MM-DD HH:MM:SS`, no `T`, no offset — e.g.
  `2026-07-07 19:11:11`). Two consequences:
  1. **Timestamp drift**: the fallback returns `Utc::now()`, so those rows'
     datetimes **change on every read** — anything comparing `created_at` /
     `updated_at` (cron "last run" logic, sorting, staleness checks) sees moving
     values.
  2. **Log flooding**: a ~2 s poll loop over the workflow list emits 5
     `WARN … Failed to parse datetime` lines per pass → the 2000-line debug ring
     buffer (`/api/debug/logs`) covers only ~10 min, drowning real signals
     (observed during incident #1 while looking for `Spawning agent` lines).
  The same helper is duplicated in `backend/src/db/discussions.rs:982` and
  `backend/src/db/projects.rs:214`.
- **Why we can't fix now (constraint)**: surfaced mid-incident under a repo
  freeze (rebuild kills in-flight agents). Trivial to fix afterwards.
- **Impact**: correctness (silent timestamp drift) · observability (debug ring
  buffer useless beyond ~10 min).
- **Where (pointers)**:
  - `backend/src/db/workflows.rs:10-17` — `parse_dt` (RFC3339-only + `Utc::now()`
    fallback).
  - `backend/src/db/discussions.rs:982`, `backend/src/db/projects.rs:214` — same
    pattern duplicated.
- **Suggested direction (non-binding)**:
  - Fallback parse `NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")`
    interpreted as UTC before warning; keep the WARN only for truly unparseable
    values, and dedupe/rate-limit it.
  - One-shot migration normalizing existing rows to RFC3339; find the writer
    that produced the SQLite format and fix it at the source.
  - Deduplicate the three `parse_dt` copies into one helper.
- **Next step**: fix directly (small), after the incident freeze lifts.
