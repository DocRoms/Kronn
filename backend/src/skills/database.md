---
name: database
description: Use when writing or reviewing SQL queries, schema changes, migrations, or ORM usage. Covers indexing strategies, query optimization, and zero-downtime migrations.
license: AGPL-3.0
category: domain
icon: 🗄️
builtin: true
---

## Procedures

1. **Run EXPLAIN before optimizing** — never guess. Read the query plan, find seq scans, check row estimates.
2. **Index for actual queries** — check WHERE/JOIN/ORDER BY clauses. Composite index column order = query filter order.
3. **Migrate safely** — add column (nullable) → deploy code → backfill → add constraint → drop old. Never lock tables.
4. **Pick normalization level deliberately** — 3NF for OLTP, denormalize for read-heavy paths with materialized views.

## Gotchas

- **Composite index order matters**: `(user_id, created_at)` serves `WHERE user_id = ?` but NOT `WHERE created_at > ?` alone.
- **SQLite WAL mode** is required for concurrent reads; default journal mode blocks readers during writes. Always set `busy_timeout`.
- **PostgreSQL JSONB** indexes need GIN — a B-tree index on a JSONB column does nothing useful.
- **Partial indexes** (Postgres) are underused — `CREATE INDEX ON orders(status) WHERE status = 'pending'` is tiny and fast.
- **N+1 in ORMs**: eager-loading/joins at query level, not in application loops. `SELECT N+1` is the #1 ORM perf killer.
- **Migrations must be forward-only in prod** — never edit a migration that has already run. Create a new one.

## Validation

- New queries have EXPLAIN output reviewed (no unexpected seq scans on large tables).
- Every new index corresponds to a real query pattern.
- Migration up/down both tested locally before merge.

✓ Index on `orders(user_id)` because queries filter by `user_id`.
✗ Indexes on every column "just in case" without checking query patterns.
