---
name: data-engineering
description: Use when building or reviewing ETL/ELT pipelines, data models, warehouse queries, or orchestration DAGs. Covers idempotency, data quality, and storage selection.
license: AGPL-3.0
category: domain
icon: 📊
builtin: true
---

## Procedures

1. **Make every job idempotent** — use UPSERT/MERGE or partition-overwrite. Re-running must not create duplicates.
2. **Choose storage by access pattern** — OLTP (Postgres), OLAP (ClickHouse/BigQuery), object store (S3), queue (Kafka/SQS).
3. **Model for the consumer** — star schema for BI dashboards, normalized for transactional, data vault for auditability.
4. **Validate at boundaries** — schema checks on ingest, null/uniqueness assertions after transforms, freshness monitors on outputs.
5. **Orchestrate with DAGs** — Airflow/Dagster/Prefect. Explicit dependencies, retries with backoff, alerts on failure.

## Gotchas

- **Batch vs stream** is a cost decision, not a tech one — streaming costs 5-10x more. Use batch unless latency SLA demands <1min.
- **Full table scans in OLAP** are fine (columnar storage). In OLTP they are not. Don't apply OLTP indexing patterns to warehouses.
- **Backfill capability is non-negotiable** — every pipeline must accept a date range param. Hardcoded "today" is a maintenance trap.
- **Data contracts** between producer/consumer prevent silent schema drift — enforce with JSON Schema or protobuf at the boundary.
- **Airflow gotcha**: default `start_date` + `catchup=True` will replay every missed interval on first deploy. Set `catchup=False` or be explicit.
- **dbt** — always `ref()` models, never hardcode table names. Use `dbt test` in CI.

## Validation

- Pipeline re-run on same input produces identical output (idempotency check).
- Schema assertions exist at ingest and after transform steps.
- DAG has alerting on failure and SLA on freshness.

✓ Pipeline uses partition-overwrite — re-running replaces, never duplicates.
✗ Pipeline appends on every run with no deduplication, corrupting aggregates.
