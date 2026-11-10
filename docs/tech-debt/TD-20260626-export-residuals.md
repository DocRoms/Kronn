# TD-20260626-export-residuals

- **ID**: TD-20260626-export-residuals
- **Area**: Backend (whole-database export)
- **Shipped (KT-1602, export v8)**: uploaded context files. The rows travel
  in `DbExport.context_files` (data.json); the bytes travel as optional
  `context-files/<id>` entries of the ZIP, written by `build_export_zip` and
  read back by `read_context_blobs`. They are counted against a decompressed
  budget at import: 160 MiB per file and 512 MiB for all of them (the import
  body limit), and only entries a `has_blob` row names are read. The exporter
  stops at the same budget, so an archive this instance wrote is never refused
  for its blobs; a file left out keeps its row, and the import reports rows
  that come back without bytes. The source machine's `disk_path` is never
  exported; the import writes the bytes under its own context-file store.
  `[src: file: backend/src/models/db.rs:150#ContextFileRecord]`
  `[src: file: backend/src/api/setup.rs:3104#read_context_blobs]`
  `[src: file: backend/src/api/setup_context_files_tests.rs:1]`
- **Problem (fact, still open)**: three table families are still missing from
  `DbExport`, so a restore does not bring them back:
  - Planning: `planning_tasks` and its satellites (`planning_task_projects`,
    `planning_task_discussions`, `planning_task_dod_items`,
    `planning_task_links`, `planning_task_tags`, `planning_task_blockers`,
    `planning_task_events`), plus `planning_proposals` /
    `planning_proposal_items` and `task_board_orders`.
  - Live Pages: `live_pages`, `live_page_revisions`, `live_page_publications`,
    `live_page_slug_aliases`, `live_page_discussion_links`,
    `live_page_datasets` / `live_page_dataset_points`,
    `live_page_action_trusts` / `live_page_action_trust_deps`.
  - Task executions: `task_executions` and the `task_execution_*` tables
    (events, deliveries, reviews, validation runs, recovery, commit leases,
    worker offers, quota rows), plus `orchestration_runs` and its event /
    policy tables.
  Also not exported: the AI-generation provenance of a context file (it lives
  in `media_jobs`).
  `[src: file: backend/src/db/sql/081_planning_tasks.sql:1]`
  `[src: file: backend/src/db/sql/123_live_pages.sql:1]`
  `[src: file: backend/src/db/sql/127_task_orchestration.sql:1]`
- **Why we can't fix now (constraint)**: these are about forty tables with
  cross-references to each other and to projects, discussions, worktrees and
  CLI sessions. A task execution points at a worktree and a session of the
  exporting machine, so a restore needs a decision on what such a row means on
  another machine (and on the trust and approval records it carries, which
  must not be reinstated blindly: see the KT-1017 seal for the same question
  on workflows). Live Pages carry published slugs and trust grants with the
  same concern. Each family needs its own model, import order and tests; none
  fits in a context-file change.
- **Impact**: correctness (a restore loses the Planning backlog, the Live
  Pages and the task execution history).
- **Where (pointers)**: `backend/src/models/db.rs` (`DbExport`), the
  `/api/config/export` and `/api/config/import` handlers in
  `backend/src/api/setup.rs` (`build_export`, `do_import_db`,
  `import_clear_statements`).
- **Suggested direction (non-binding)**: one ticket per family, each adding an
  optional `#[serde(default)]` field to `DbExport` (bump
  `CURRENT_EXPORT_VERSION`), a selective clear in `import_clear_statements`
  that only runs when the archive carries rows, and a round-trip test.
  Planning first (self-contained, no machine-bound rows); task executions last
  (machine-bound). Secrets stay stripped by design.
- **Ticket**: NOT YET CREATED. The agent that wrote this entry has no
  ticket-creation tool; the principal must file the follow-up tickets (one per
  family above) and write their ids here.
- **Next step**: file the follow-up tickets.
