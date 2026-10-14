DATABASE AUDIT (0.8.4) — focused re-scan on data-layer hygiene. Read existing `docs/inconsistencies-database.md` if present (anti-repetition: don't re-discover the same finding under a new slug — refresh / reuse the ID per § C below).

# A. SCHEMA + MIGRATIONS
- **Migration safety**: any migration that ALTERs a table > 1M rows must be backed by a non-blocking strategy (online DDL, shadow column + backfill, pt-online-schema-change). Flag `ALTER TABLE ... ADD COLUMN NOT NULL DEFAULT ...` on big tables — locks the whole table on MySQL pre-8.0.
- **Migration reversibility**: each migration file should declare a `down()` or be explicitly marked irreversible. Check the `migrations/` folder (Flyway, Alembic, Diesel, Knex, sqlx, etc.).
- **Schema drift**: compare the live schema (or the dump committed alongside migrations) with what the ORM models declare. Stale columns, missing NOT NULL, missing FK constraints.
- **Charset/collation**: in 2026, only `utf8mb4` is acceptable on MySQL. `utf8` (3-byte) breaks emoji + supplementary planes.

# B. INDEXES + PERFORMANCE
- Missing indexes on common WHERE clauses (read the ORM hot paths — list-by-user-id, filter-by-status, ordered-by-created-at).
- Over-indexing: tables with > 8 indexes incur write penalty + RAM pressure.
- Compound indexes with wrong column order (low-cardinality column first wastes the index).
- Missing `WHERE clause` in DELETE/UPDATE in app code (PostgreSQL: missing `... WHERE id = $1` on `DELETE FROM users` is catastrophic).

# C. ORM + N+1
- **N+1 queries**: scan the most-used list / dashboard endpoints. ORM lazy-load on a collection of N parent rows produces N+1 queries.
- **Unbounded SELECT**: queries without LIMIT on user-facing endpoints (`User.all` on the admin page when the table grew to 500k rows).
- **Transaction boundaries**: long-running transactions that hold locks (PostgreSQL `idle in transaction`, MySQL row-locking) blocking other writers. Check that DB calls inside HTTP handlers commit promptly.

# D. DATA INTEGRITY
- Foreign keys: missing constraints (ORM-level FK only is not enforced at the DB layer if `disable_constraints` was ever set in tests).
- Soft-delete vs hard-delete inconsistency (some queries scope WHERE deleted_at IS NULL, others don't — leak risk).
- Nullable columns that the app treats as required: TypeScript / Rust types may say `string` but the column allows NULL.

# E. ANTI-REPETITION + OUTPUT FORMAT

Same rules as the Full audit Step 8 (§ C and § D): scan `docs/tech-debt/` for existing TDs; for each finding, decide REUSE / REFINE / NEW per § C; create `docs/tech-debt/TD-<date>-<slug>.md` for new ones; UPDATE `audit_history` for existing. Detail file schema = the Full audit's. Index file = `docs/inconsistencies-database.md` (not `inconsistencies-tech-debt.md`). Severity calibration is the same.

Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions). If no findings, the index reads `'None identified during this database audit pass'`.