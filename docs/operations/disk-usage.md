# Disk usage — what Kronn keeps, what it purges, when it refuses

**Status:** in force (2026-10-16) · **Origin:** KT-1318, after the 16/10 incident. It supersedes the disk
part of KT-1197 (worktree inventory); the git refs / branches part stays there.

## What happened

A 1.8 TB disk reached 683 MB free. Agents and workers stalled repeatedly: reviews died on
`No space left on device`, four subagents and two workers stopped. `<repo>/target/` held **195 per-lane
`CARGO_TARGET_DIR`s** (`wt-<worktree>`, 5-30 GB each): task lanes, campaign lanes, review and probe caches.
A lane's worktree was removed when its task ended, but its build dir stayed. Parallel lanes — the point of
Kronn's orchestration — multiplied the cost. 184 orphans were purged by hand.

Three things were missing, and this page describes the three that now exist: **end-of-life cleanup plus an
orphan sweeper**, a **free-space guard on launch**, and **visibility** (API, MCP tool, Settings panel).

## What Kronn creates

| Kind (`DiskKind`) | Path | Owner | Removed by |
|---|---|---|---|
| `worktree` | `<repo>/.kronn/worktrees/<name>` | the task execution that provisioned it | its integration or cancel flow only — **never by a purge** |
| `lane_build_dir` | `<repo>/target/wt-<lane>` (the lane's `CARGO_TARGET_DIR`) | the execution named in its ownership marker | the end-of-life cleanup, the sweeper, a purge |
| `cache` | `<repo>/.kronn/worktrees/<name>/target` | the same execution | the artefact reclaim (KT-373), a confirmed purge |

A lane's env is set in `WorkerLane::for_worktree`: a managed worktree gets `<repo>/target/wt-<lane>`, and the
worker runs with it as `CARGO_TARGET_DIR`. The scratch dir, the private `CARGO_HOME` and the Vite cache live
inside the lane or the worktree, so they go with it.
[src: file: backend/src/core/task_worker_env.rs:112-167#for_worktree]

**Ownership is a manifest, not a name.** `wt-*` proves nothing: humans and other tooling build in
`target/wt-0151-integration`. Kronn writes `.kronn-lane.json` (execution id, lane, worktree) when it
provisions a lane, and only a lane carrying it is ever swept. Everything else — an unmarked `wt-*`, the
interactive `target/debug`, any directory Kronn did not create — is **reported as `foreign` and never
touched**. [src: file: backend/src/core/worktree.rs:523-553#mark_lane_target]

## When something is removed

| Event | What goes | Guard |
|---|---|---|
| Execution **Done** (integrated), clean worktree removed | the lane build dir, with the worktree | `cleanup_integrated_execution` |
| Execution **Cancelled**, `RemoveIfClean` | the lane build dir, with the worktree | `settle_cancelled_workspace` |
| Boot: an orphaned managed checkout removed | its lane build dir, with it | `reconcile_at_boot` |
| Worktree **kept** (dirty, diverged, `Preserve`) | nothing: the lane is kept with it | same policy as the worktree |
| Boot, **every 30 min**, and under pressure | **orphan** lane build dirs | the sweeper rules below |

[src: file: backend/src/api/orchestration.rs:3668-3680#cleanup_integrated_execution]
[src: file: backend/src/api/orchestration.rs:10279-10290#settle_cancelled_workspace]

### The sweeper's rules

A lane build dir is an **orphan** — and only then removed by the sweep — when *all* hold:

1. it carries Kronn's ownership marker (Kronn created it);
2. durable state proves its execution **finished** (no live session, lease or unfinished execution);
3. its worktree is **gone**;
4. it has not been written for **`server.disk_sweep_idle_hours`** (default **6 h**). The age is the newest
   write among the lane, its entries and their children, so a build that just ran moves it forward; an
   unreadable age is *never* idle;
5. Cargo does not hold its build lock.

An aged lane of a live execution, or whose clean worktree is still there, is **not** an orphan.
[src: file: backend/src/core/worktree.rs:825-860#process_lane_target_idle]
[src: file: backend/src/core/worktree.rs:774-810#lane_target_last_write]
[src: file: backend/src/api/disk.rs:129-140#lane_entry_state]

The periodic sweep is started at boot, after the boot reconciliation:
[src: file: backend/src/api/disk.rs:833-855#spawn_periodic_sweep]

## The launch guard

Before provisioning a task worker (`provision_task_execution_inner`, phase A′) and before a **full audit**,
Kronn reads the free space of the volume holding the worktrees and `target/` (O(1), no walk). Below
**`server.disk_launch_min_free_gib`** (default **30**; `0` turns the guard off):

1. it runs the orphan sweep — the launching project first, then the others;
2. still under the threshold: it **refuses** the launch with the number, the setting and what the sweep
   gave back, **before creating anything**. A worker launch parks its execution `Blocked` (resumable once
   space is free) and posts a system message in the **principal room**.

[src: file: backend/src/api/disk.rs:764-830#ensure_launch_headroom]
[src: file: backend/src/api/orchestration.rs:7697-7720#post_disk_pressure_alert]

Until a config is published at boot the guard is off (`0`), so a process that never loaded one — a unit
test — is never refused by a threshold nobody chose.
[src: file: backend/src/core/disk_usage.rs:35-40#set_disk_policy]

The older hard floor still applies underneath: `server.disk_critical_gib` (default 5) refuses worktree
provisioning and builds; see [worktree-build-artifacts.md](worktree-build-artifacts.md#thresholds).

## Seeing it: API, MCP tool, UI

- `GET /api/maintenance/disk[?project_id=…]` — free space (smallest among the projects' volumes), the
  policy, `totals` by kind, and every `entries[]` largest first: `kind`, `path`, `bytes`
  (`bytes_are_partial` = a floor: the walk is bounded in entries and time), `idle_seconds`, `owner`
  (`task_ref`, `execution_id`, `status`), `state` and `purgeable`.
  [src: file: backend/src/api/disk.rs:455-470#build_disk_report]
- `POST /api/maintenance/disk/purge` — no `paths`: purge **orphans only**. With `paths`: delete exactly
  those entries, and only with `confirm: true`; each is re-judged at deletion time, so a report never
  authorises a delete. [src: file: backend/src/api/disk.rs:891-903#run_purge]
- MCP tool **`disk_usage`** (`project_id`, `purge_orphans`) — the same report, bounded to the 25 largest
  entries; `purge_orphans: true` takes the default purge and nothing else. A named delete is deliberately
  a confirmed UI action. [src: file: backend/scripts/disc-introspection-mcp.py:7689-7720#call_disk_usage]
- **Settings → Disk** — measure on demand (never on page open), totals by kind, the table, **Purge
  orphans**, a per-entry delete behind a confirmation, and the two thresholds.

`state` is one of `orphan` (the default purge takes it), `reclaimable` (finished but recent, or its clean
worktree is kept: only a confirmed per-entry delete), `active` (a live execution claims it: **never
removed**), `kept` (dirty worktree, unreadable state) and `foreign` (not Kronn's: never touched).
[src: file: backend/src/core/disk_usage.rs:80-95#DiskState]

## Settings

| Key | Default | Meaning |
|---|---:|---|
| `server.disk_launch_min_free_gib` | 30 | free GiB a worker launch / full audit needs; sweep first, then refuse |
| `server.disk_sweep_idle_hours` | 6 | hours a lane must be unwritten before the sweeper may take it |
| `server.disk_warning_gib` / `disk_critical_gib` | 20 / 5 | KT-373 floor, unchanged |

Both new keys are editable in Settings → Disk or `POST /api/config/server`, and apply without a restart.

## Limits

- **Pre-existing lanes carry no marker.** The 195 directories of the incident were created before the
  manifest existed; they show as `foreign` and are not touched. Remove them by hand once, with the lane's
  worktree gone and no build running (`ls -t target | grep ^wt-`).
- Sizes are floors on a very large target: the report says `≥` rather than stall (1.69 million files took
  44 minutes to walk in the 2026-08-21 incident).
- The guard measures free space *before* the launch; it cannot stop a build that fills the disk later. The
  idle threshold and the launch threshold exist so that a full disk is refused early instead.
- Bench runs have no launch endpoint of their own: they go through the audit launch, which is guarded.

## Tests

Unit: ownership and age rules (`core/worktree.rs` idle tests, `api/disk_test.rs`), the guard's decision and
the bounded measure (`core/disk_usage_test.rs`). End to end (`api/orchestration.rs`): an aged orphan is
swept and a fresh, foreign or live-execution lane never; a launch under pressure sweeps first, then is
refused, parks the execution and alerts the principal room.
