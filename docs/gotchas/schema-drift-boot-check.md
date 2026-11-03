# Schema drift is checked and repaired at boot (KT-1483)

`_migrations` only proves a migration ran, not that its tables still exist. After
the migrations, `db::schema_check` (`backend/src/db/schema_check.rs`) diffs the live
schema against the one every migration builds on a fresh in-memory database:

- missing table / index / trigger: recreated with `CREATE ... IF NOT EXISTS` from the
  SQL SQLite stored. A recreated table is **empty**, its old rows are gone.
- missing column: added back with `ALTER TABLE ADD COLUMN` when allowed (no key, no
  NOT NULL without default). CHECK / REFERENCES clauses are not restored.
- the rest is logged with `tracing::error!` and listed in `GET /api/health` under
  `schema` (`status`: `ok` | `repaired` | `degraded`, plus `repaired` / `unrepaired`).

Only runs for a database opened from a file (`run_with_backup` with a path), not for
in-memory test databases. No migration needs registering: a new migration is covered
automatically because the reference is built from `MIGRATIONS`.
