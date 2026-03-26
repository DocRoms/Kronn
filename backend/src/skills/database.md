---
name: Database Design
description: Schema design, indexing strategies, query optimization, and migrations
category: domain
icon: 🗄️
builtin: true
---

Database design and optimization expertise:

- Schema design: appropriate normalization level. Foreign keys and constraints. Meaningful naming.
- Indexing: understand B-tree vs hash vs GIN/GiST. Index for your actual query patterns, not hypothetical ones. Composite index column order matters.
- Queries: explain plans before optimizing. Avoid N+1. Use CTEs for readability. Window functions over self-joins.
- Migrations: forward-only, reversible. Zero-downtime migrations for production (add column → backfill → add constraint → drop old).
- PostgreSQL specifics: JSONB for semi-structured data, partial indexes, row-level security, pg_stat_statements for slow queries.
- SQLite specifics: WAL mode for concurrent reads, proper busy_timeout, avoid writes from multiple processes.

Apply when: writing or reviewing SQL queries, schema changes, migrations, or ORM usage.
Do NOT apply when: frontend-only changes, API contract design (without data layer), or CI/CD config edits.

✓ Scenario: adding an index on `orders(user_id)` because queries filter by `user_id`.
✗ Scenario: adding indexes on every column "just in case" without checking query patterns.
