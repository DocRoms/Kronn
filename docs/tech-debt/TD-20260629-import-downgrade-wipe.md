# TD-20260629-import-downgrade-wipe

- **ID**: TD-20260629-import-downgrade-wipe
- **Area**: Backend
- **Problem (fact)**: `do_import_db` (`backend/src/api/setup.rs`) is a **destructive full restore**: it `DELETE`s every table (incl. `quick_apis`, `learnings`) and then re-inserts **from the export payload**. When the export is an **older version than the running Kronn**, the tables that didn't exist in that export deserialize to `Vec::default()` (`[]`, via `#[serde(default)]`) — so the `DELETE` **wipes them and nothing is restored**. Concretely: importing a **v3** export onto a **v4** box silently empties `quick_apis` and `learnings`. Confirmed live 2026-06-29: a real v3 export imported cleanly but left `quick_apis = 0` (the source had 25) — partly because v3 never carried them, but the same path would also have erased any QAs already present on the target.
- **Why we can't fix now (constraint)**: It works for the common same-version case and the destructive semantics are intentional (restore = replace). A safe fix needs a small design decision (warn + confirm? selective clear? merge/upsert?) and shouldn't be bundled with the urgent body-limit fix that surfaced it.
- **Impact**: correctness · **data loss** (silent, on a downgrade-version import)
- **Where (pointers)**:
  - `backend/src/api/setup.rs` — `do_import_db` (the `DELETE FROM …` batch ~L1160) + `DbExport.version` handling.
  - `backend/src/models/db.rs` — `DbExport` (`version: u32`, the `#[serde(default)]` fields).
- **Suggested direction (non-binding)**:
  - **Warn loudly** when `export.version < CURRENT_EXPORT_VERSION` — surface in `ImportResult.warnings` + a UI confirm ("this backup predates feature X; importing will erase your current X").
  - **Don't clear tables absent from the payload** — only `DELETE` the tables the export actually carries (selective clear), so an old export can't wipe newer data.
  - Longer term: consider **merge/upsert** import instead of clear-then-insert.
- **Next step**: create ticket.
- **Status**: 🟢 RESOLVED in 0.8.10 (verified 0.8.11). `do_import_db` (`backend/src/api/setup.rs`) now clears SELECTIVELY — only the tables the payload actually carries (`import_clear_statements`) — and warns loudly on a downgrade import (`data.version < CURRENT_EXPORT_VERSION`). A v3 export onto a v4 box no longer wipes `quick_apis`/`learnings`. The destructive endpoints are also auth-gated even when app-auth is off (see `DESTRUCTIVE_PATHS` in `lib.rs`). Closing.

## Notes

- Surfaced 2026-06-29 while debugging "no QAs after import". Root cause was benign (the export was v3, made before `quick_apis` existed → re-export from the now-v4 source fixes it), but it exposed this destructive-downgrade footgun. The body-limit import bug fixed the same day is unrelated (`backend/src/lib.rs` `DefaultBodyLimit`).
