---
name: Data Engineering
description: ETL/ELT pipelines, data modeling, quality, and orchestration
category: domain
icon: 📊
builtin: true
---

Data engineering expertise:

- Pipelines: ETL/ELT design. Idempotent jobs. Backfill capability. Failure recovery.
- Data modeling: star schema, snowflake, or data vault depending on use case. Normalization vs denormalization trade-offs.
- Quality: schema validation, null checks, uniqueness constraints, freshness monitoring. Data contracts between producers and consumers.
- Storage: choose the right tool — OLTP (Postgres), OLAP (BigQuery, ClickHouse), object store (S3), cache (Redis), queue (Kafka/SQS).
- Processing: batch (Spark, dbt) vs streaming (Kafka Streams, Flink). Know when each is appropriate.
- Orchestration: Airflow, Dagster, or Prefect. DAGs with clear dependencies. Retry and alerting.
- Performance: partitioning, indexing, materialized views. Query optimization. Avoid full table scans.

Apply when: building or reviewing ETL/ELT pipelines, data models, warehouse queries, or orchestration DAGs.
Do NOT apply when: application-level CRUD operations, frontend code, or simple single-table queries.

✓ Scenario: pipeline job is idempotent — re-running it produces the same result without duplicates.
✗ Scenario: pipeline appends on every run with no deduplication, corrupting downstream aggregates.
