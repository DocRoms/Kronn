---
name: Database Design
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
