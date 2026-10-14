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
| `worktree` | `<repo>/.kronn/worktrees/<name>` | the task execution that provisioned it | its integration or cancel flow, or the terminal-checkout release below — **never by a purge** |
| `lane_build_dir` | `<repo>/target/wt-<lane>` (the lane's `CARGO_TARGET_DIR`) | the execution named in its ownership marker | the end-of-life cleanup, the sweeper, a purge |
| `cache` | `<repo>/.kronn/worktrees/<name>/target` | the same execution | the artefact reclaim (KT-373), a confirmed purge |

A lane's env is set in `WorkerLane::for_worktree`: a managed worktree gets `<repo>/target/wt-<lane>`, and the
worker runs with it as `CARGO_TARGET_DIR`. The scratch dir, the private `CARGO_HOME` and the Vite cache live
inside the lane or the worktree, so they go with it.
[src: file: backend/src/core/task_worker_env.rs:112-167#for_worktree]

**Ownership is a manifest, not a name.** `wt-*` proves nothing: humans and other tooling build in
`target/wt-0151-integration`. Kronn writes `.kronn-lane.json` (execution id, lane, worktree) when it
provisions a lane, again on every Claude worker launch (resumes and reworks, task or campaign) and before
a validation builds in a worker's lane whatever the worker kind, so a lane Kronn made is never left
unowned. The file is written once per execution (a relaunch leaves the lane's idle clock alone) next to a
`.gitignore` of `*`, so a checkout that does not ignore `target/` never reads it as uncommitted work. A lane
carrying the marker is swept. A lane **without** a marker is swept only when durable lineage attributes it
to a managed workspace (see *Lane attribution* below). Everything else — an unmarked `wt-*` no workspace
row names, the interactive `target/debug`, any directory Kronn did not create — is **reported as `foreign`
and never touched**.
[src: file: backend/src/core/worktree.rs:523-570#mark_lane_target]
[src: file: backend/src/core/worktree.rs:580-598#mark_lane_target_for_worktree]

## When something is removed

| Event | What goes | Guard |
|---|---|---|
| Execution **Done** (integrated), clean worktree removed | the lane build dir, with the worktree | `cleanup_integrated_execution` |
| Execution **Cancelled**, `RemoveIfClean`, clean checkout | the lane build dir, with the worktree | `settle_cancelled_workspace` |
| Execution **Cancelled**, checkout **dirty** or policy `Preserve` | nothing at cancel time: the worker may still be writing | released later by the terminal-checkout release |
| Boot and **every 30 min**: a **Done, Failed or Cancelled** execution whose managed checkout is still on disk | the checkout (branch kept) and its lane build dir | the terminal-checkout release below |
| Boot: an orphaned managed checkout removed | its lane build dir, with it | `reconcile_at_boot` |
| Worktree **kept** (diverged, over the evidence cap, written recently) | nothing: the lane is kept with it | same policy as the worktree |
| Boot, **every 30 min**, and under pressure | **orphan** lane build dirs | the sweeper rules below |

### Terminal-checkout release (KT-1397)

A task execution that ends Done, Failed or Cancelled (including a cancel the principal followed by a manual
integration) no longer keeps `.kronn/worktrees/task-*` forever. At boot and from the periodic sweep, before
the lane sweep, Kronn releases the managed checkout of every such execution when *all* hold:

1. the workspace is Kronn's (managed layout) and the checkout is on exactly the execution's branch;
2. durable state proves the execution terminal with no live session, finished for at least
   `server.disk_sweep_idle_hours`;
3. nothing in the checkout (excluding the Git admin entry) was written within that same window: a
   finished execution does not prove nobody is working in the directory. A recent write, or an
   unreadable mtime, keeps the checkout with a recorded reason;
4. the uncommitted state fits the evidence cap (256 MiB): a larger one keeps the checkout, with a reason.

The **branch is kept**. Uncommitted work is saved first, under `<data dir>/task-evidence/<execution id>/`,
as two patches that reapply in order on the kept branch: `staged-changes.patch` (`HEAD` → the worktree's own
index, written only when something is staged) then `uncommitted-changes.patch` (that index → the files on
disk, untracked files included). `git apply --index` the first and `git apply` the second rebuild the index
and the working tree exactly; staged-only content (index differs from the files) is therefore not lost. The
worktree's index is only copied, never written. If a patch cannot be written, exceeds the cap, or the index
holds unmerged paths, nothing is removed. Each distinct *kept* reason is recorded once as a
`worktree_release_kept` event, a release as `worktree_released`. Evidence patches are not cleaned up by Kronn.

[src: file: backend/src/api/orchestration.rs:3840-3852#cleanup_integrated_execution]
[src: file: backend/src/api/orchestration.rs:11593-11604#settle_cancelled_workspace]

### The sweeper's rules

A lane build dir is an **orphan** — and only then removed by the sweep — when *all* hold:

1. Kronn created it: it carries the ownership marker, or — for a lane made before the marker existed —
   durable lineage attributes it (below);
2. durable state proves its execution **finished** (no live session, lease or unfinished execution);
3. its worktree is **gone**;
4. it has not been written for **`server.disk_sweep_idle_hours`** (default **6 h**, floor **1 h**: a
   configured `0` is read as 1, never as "no idle check"). The age is the newest
   write among the lane, its entries and their children, so a build that just ran moves it forward; an
   unreadable age is *never* idle;
5. Cargo does not hold its build lock.

An aged lane of a live execution, or whose clean worktree is still there, is **not** an orphan.

**Lane attribution.** An unmarked `target/wt-*` is attributed through the database: the workspace rows of
managed workspaces that still name their worktree give the lane name Kronn would have derived, and so the
execution that owned it. It is then judged by the rules above (worktree gone, execution terminal, idle). A
dir no managed workspace names, one that belongs to an unmanaged workspace, or a lane name two executions
derive in common is attributed to nobody and stays `foreign`.
[src: file: backend/src/core/worktree.rs:906-961#process_lane_target_idle]
[src: file: backend/src/core/worktree.rs:855-891#lane_target_last_write]
[src: file: backend/src/api/disk.rs:129-140#lane_entry_state]

The periodic sweep is started at boot, after the boot reconciliation:
[src: file: backend/src/api/disk.rs:864-886#spawn_periodic_sweep]

## The launch guard

Before provisioning a task worker (`provision_task_execution_inner`, phase A′) and before a **full audit**,
Kronn reads the free space of the volume holding the worktrees and `target/` (O(1), no walk). Below
**`server.disk_launch_min_free_gib`** (default **30**; `0` turns the guard off):

1. it runs the orphan sweep — the launching project first, then the others;
2. still under the threshold: it **refuses** the launch with the number, the setting and what the sweep
   gave back, **before creating anything**. A worker launch parks its execution `Blocked` (resumable once
   space is free) and posts a system message in the **principal room**.

[src: file: backend/src/api/disk.rs:791-857#ensure_launch_headroom]
[src: file: backend/src/api/orchestration.rs:8592-8615#post_disk_pressure_alert]

Until a config is published at boot the guard is off (`0`), so a process that never loaded one — a unit
test — is never refused by a threshold nobody chose.
[src: file: backend/src/core/disk_usage.rs:42-51#set_disk_policy]

The older hard floor still applies underneath: `server.disk_critical_gib` (default 5) refuses worktree
provisioning and builds; see [worktree-build-artifacts.md](worktree-build-artifacts.md#thresholds).

## Seeing it: API, MCP tool, UI

- `GET /api/maintenance/disk[?project_id=…]` — free space (smallest among the projects' volumes), the
  policy, `totals` by kind, and every `entries[]` largest first: `kind`, `path`, `bytes`
  (`bytes_are_partial` = a floor: the walk is bounded in entries and time), `idle_seconds`, `owner`
  (`task_ref`, `execution_id`, `status`), `state` and `purgeable`.
  [src: file: backend/src/api/disk.rs:468-483#build_disk_report]
  An agent's bridge token gets a **scoped view** (KT-1347): free space, `policy`, `totals` and `orphan_*`
  stay whole-instance aggregates, but `volumes` and `entries` (paths, lane names, task refs) are those of
  the token's own project only — none for a project-less launch — and the report carries `scoped: true`.
  Naming another project in `?project_id=` is refused (403). The operator and the UI keep the full report.
  [src: file: backend/src/api/disk.rs:519-528#scoped_for_bridge]
- `POST /api/maintenance/disk/purge` — no `paths`: purge **orphans only**. With `paths`: delete exactly
  those entries, and only with `confirm: true`; each is re-judged at deletion time, so a report never
  authorises a delete. **Human / UI only**: the route is deliberately not on `BRIDGE_ROUTES`, so a bridge
  token is refused (403). It deletes host files; even an own-project orphan purge was not worth giving to
  an agent, which can tell the user instead. [src: file: backend/src/api/disk.rs:932-944#run_purge]
- MCP tool **`disk_usage`** (`project_id`) — the scoped report above, read-only, bounded to the 25 largest
  entries. It never calls the purge; a leftover `purge_orphans: true` argument returns a note that purging
  is a human action in Settings > Disk. [src: file: backend/scripts/disc-introspection-mcp.py:7714-7746#call_disk_usage]
- **Settings → Disk** — measure on demand (never on page open), totals by kind, the table, **Purge
  orphans**, a per-entry delete behind a confirmation, and the two thresholds.

`state` is one of `orphan` (the default purge takes it), `reclaimable` (finished but recent, or its clean
worktree is kept: only a confirmed per-entry delete), `active` (a live execution claims it: **never
removed**), `kept` (dirty worktree, unreadable state) and `foreign` (not Kronn's: never touched).
[src: file: backend/src/core/disk_usage.rs:139-154#DiskState]

## Settings

| Key | Default | Meaning |
|---|---:|---|
| `server.disk_launch_min_free_gib` | 30 | free GiB a worker launch / full audit needs; sweep first, then refuse |
| `server.disk_sweep_idle_hours` | 6 | hours (minimum 1; `0` is read as 1) a lane must be unwritten before the sweeper may take it |
| `server.disk_warning_gib` / `disk_critical_gib` | 20 / 5 | KT-373 floor, unchanged |

Both new keys are editable in Settings → Disk or `POST /api/config/server`, and apply without a restart.

## Limits

- **Pre-existing lanes carry no marker.** Those that a managed workspace row still names are attributed
  and swept like any orphan. The others (no row, or a row of an unmanaged workspace) show as `foreign` and
  are not touched: remove them by hand once, with the lane's worktree gone and no build running
  (`ls -t target | grep ^wt-`).
- A checkout over the evidence cap, or recently written, stays until a human acts or it goes idle; the
  evidence patches under `task-evidence/` are never purged by Kronn.
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
