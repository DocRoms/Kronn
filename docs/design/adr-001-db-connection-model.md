# ADR-001 — SQLite connection model: keep the single connection, defer a separate read connection

- **Status**: PROPOSED (pass 7 of the 0.9 plan — Codex review requested)
- **Date**: 2026-07-13
- **Upstream context**: 2026-07 audit ("structural amplifier" finding), the
  pass-A1 writer inventory, 0.8.11 incidents (API freeze during backup,
  poisoned mutex).

## Context

All DB access goes through ONE shared connection:
`Arc<Mutex<rusqlite::Connection>>`, exposed by `Database::with_conn`, which
runs every closure inside `tokio::task::spawn_blocking` while holding the lock
(`backend/src/db/mod.rs`). WAL is on by default (`KRONN_DB_WAL=0` for network
mounts), `busy_timeout=5000`.

Observed structural consequences:

1. **Total serialization** — reads AND writes compete for the same mutex; WAL
   (which would allow N readers concurrent with 1 writer) is neutralized by
   our own application-level lock.
2. **Incident amplification** — any slow code under the lock freezes the WHOLE
   API. Lived through in 0.8.11: the backup copy ran in 5-page steps with a
   50 ms pause while holding the lock (~2.5 s of global freeze per MB); fixed
   with a single-step copy, but the bug CLASS stays open as long as the lock
   is unique.
3. **Total outage on panic** — a panic inside a closure poisoned the mutex
   forever (lived through, fixed in 0.8.11: poison recovery + catch_unwind).
   Same remark: symptom treated, structure unchanged.

## What the A1 inventory contributes to the decision

Pass A1 inventoried the **9 writers** of `workflow_runs.status` and converged
them onto SQL-predicate primitives (`update_run_progress` guarded by status,
atomic `claim_run_status`, batch counters frozen outside Running). Two
properties follow:

- **Transition integrity does NOT depend on the single connection.** Every
  critical write is an atomic `UPDATE … WHERE status = ?` at the SQL level;
  two concurrent connections would produce the same outcome (exactly one
  winner), because the arbitration lives in the predicate, not in the Rust
  mutex. The single connection is therefore NOT a correctness ingredient —
  it is a simplicity choice.
- **Writers are few and short.** No long multi-step transactions on the runs
  path; large payloads (step_results_json) are single writes. The real
  contention comes from HEAVY READS (run lists with full step_results,
  exports) sharing the queue with these short writes.

## Options

### O1 — Strict status quo (single connection, nothing else)
- ✅ Zero risk, zero work.
- ❌ The "slow code under the lock = global freeze" class remains; every new
  read-heavy feature (dashboards, exports, learning) pays the tax again.

### O2 — Separate READ connection(s) (WAL readers)
A second connection (or a small pool, N=2-3) opened for reads, used by
consultation endpoints (`with_read_conn`); all writes stay on the current
single connection.
- ✅ Finally exploits WAL (readers never blocked by the writer); removes the
  global-freeze class for reads; incremental migration (endpoint by
  endpoint); NO change to the writers — the A1 inventory stays fully valid.
- ✅ Bounded regression risk: a WAL read sees a consistent snapshot; the worst
  case is a read slightly behind an in-flight write — already true today at
  the granularity of frontend polling.
- ❌ Two disciplines to maintain ("which connection for this handler?"); risk
  of drift if a writer sneaks onto the read connection (simple guard:
  `PRAGMA query_only=1` on read connections — a violation is an immediate
  SQLite error).

### O3 — Generalized pool (r2d2 / deadpool, writes included)
- ✅ The "normal" model for SQL applications.
- ❌ SQLite has only ONE writer at a time: a pool of writers parallelizes
  nothing, it moves the serialization to `SQLITE_BUSY`/busy_timeout with a
  WORSE failure mode (scattered non-deterministic timeouts instead of a
  wait queue). All writes would need routing to a dedicated connection
  anyway — that is O2 with more dependencies.
- ❌ Invalidates the serialization assumption some read-modify-write closures
  still rely on outside the runs path (config, contacts): a full audit would
  be required first, for no benefit beyond O2.

## Decision (proposed)

**Adopt O2 as the target; defer its execution until after 0.9.**

1. **Now (0.9)**: nothing blocks the release on this front. Both known
   structural incidents have point fixes, transition correctness is
   guaranteed by the SQL predicates (A1), and the `kronn::invariant`
   observability will surface blocked writes.
2. **Post-0.9, first O2 iteration**: add `with_read_conn` (a `query_only`
   read connection, same WAL), migrate the 3 heaviest read endpoints (run
   list, exports, dashboards). Measure before/after on P99 handler freeze
   during a heavy write.
3. **Non-regression guard**: a test that opens both connections, writes on
   one, and verifies visibility plus the `query_only` enforcement on the
   other.
4. **O3 is explicitly rejected** as long as SQLite remains the engine — the
   day a real multi-writer becomes necessary, the question is "Postgres?",
   not "SQLite pool?".

## Consequences

- No code change in 0.9 (this document only).
- The `with_conn` API remains the write path; every new heavy-read endpoint
  must target `with_read_conn` from its creation post-0.9.
- The project rule "no writes outside `with_conn`" extends to: "no heavy
  SELECT on the write connection once `with_read_conn` exists".
