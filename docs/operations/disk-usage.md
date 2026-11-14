# Disk usage — what Kronn keeps, what it purges, when it refuses

**Status:** in force (2026-11-11) · **Origin:** KT-1318, after the 16/10 incident; second pass KT-1691 after
the 11/11 one (seed pruning, real sizes, purgeable foreign / ownerless / stalled build dirs). It supersedes the
disk part of KT-1197 (worktree inventory); the git refs / branches part stays there.

## What happened

A 1.8 TB disk reached 683 MB free. Agents and workers stalled repeatedly: reviews died on
`No space left on device`, four subagents and two workers stopped. `<repo>/target/` held **195 per-lane
`CARGO_TARGET_DIR`s** (`wt-<worktree>`, 5-30 GB each): task lanes, campaign lanes, review and probe caches.
A lane's worktree was removed when its task ended, but its build dir stayed. Parallel lanes — the point of
Kronn's orchestration — multiplied the cost. 184 orphans were purged by hand.

Three things were missing, and this page describes the three that now exist: **end-of-life cleanup plus an
orphan sweeper**, a **free-space guard on launch**, and **visibility** (API, MCP tool, Settings panel).

### The second incident (2026-11-11, KT-1691)

A 2 TB disk had 173 GB free. `disk_usage` listed 33 lane build dirs of ~190 GB each (1.85 TB), yet deleting
5 of them freed 39 GB: each lane is an APFS clone of the seed `target/debug`, and the size the files add up
to counts the blocks a clone shares in full. The seed itself was 174 GB, 91 GB of it stale `kronn-*` test
binaries and objects of months-old builds (1 440 files): Cargo never removes an old artifact, and every lane
cloned all of it. Three more things were never purged: `foreign` dirs, `kept` worktrees and their `target/`
no workspace row names, and the build dirs of executions Blocked or Interrupted for days. The three sections
*The seed build dir*, *Real and apparent sizes* and *Foreign, ownerless and stalled* below are the answer.

## What Kronn creates

| Kind (`DiskKind`) | Path | Owner | Removed by |
|---|---|---|---|
| `worktree` | `<repo>/.kronn/worktrees/<name>` | the task execution that provisioned it | its integration or cancel flow, or the terminal-checkout release below — **never by a purge** |
| `lane_build_dir` | `<repo>/target/wt-<lane>` (the lane's `CARGO_TARGET_DIR`) | the execution named in its ownership marker | the end-of-life cleanup, the sweeper, a purge |
| `cache` | `<repo>/.kronn/worktrees/<name>/target` | the same execution | the artefact reclaim (KT-373), a confirmed purge |

A lane's env is set in `WorkerLane::for_worktree`: a managed worktree gets `<repo>/target/wt-<lane>`, and the
worker runs with it as `CARGO_TARGET_DIR`. The scratch dir, the private `CARGO_HOME` and the Vite cache live
inside the lane or the worktree, so they go with it.
[src: file: backend/src/core/task_worker_env.rs:256-311#for_worktree]

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
[src: file: backend/src/core/worktree.rs:644-691#mark_lane_target]
[src: file: backend/src/core/worktree.rs:696-714#mark_lane_target_for_worktree]

## The seed build dir (KT-1691)

A lane starts as a copy-on-write clone of the main checkout's `target/debug` (the *seed*, KT-1250). Before
cloning it, Kronn prunes the seed to its latest build, so a lane never inherits months of stale artifacts and
never pays to rewrite them. The prune runs in `seed_lane`, on every lane start, and costs a walk of the
fingerprint directory: with nothing stale it removes nothing.
[src: file: backend/src/core/seed_prune.rs:389-401#prune_and_lock_shared]

**What is stale is decided from Cargo's bookkeeping, never from a file's age alone** (an unchanged dependency
keeps its months-old mtime and is exactly what a lane wants):

- a *unit* is a `.fingerprint/<package>-<hash>` directory; its *kind* is the artifact it describes
  (`lib-serde`, `bin-kronn`, `test-lib-kronn`, a build script's...), and its `<kind>.json` is the fingerprint
  Cargo itself compares (`rustc`, `features`, `profile`, `target`, `path`, `rustflags`, `compile_kind`...).
  Units of one package that differ by any of these are **distinct valid variants** (`hashbrown` with
  `["raw"]` and with its default features, a dependency built for the host and for the target): one build
  needs several of them, so a package is never reduced to its newest unit. Only units with the **same
  identity** (package, kind, features, profile, target, path, config, rustflags, compile kind) are one unit
  built several times; the one written last is current, the older ones are leftovers (a dependency changed
  underneath them) and go with every file of their hash (`deps/*-<hash>.*`, `build/<package>-<hash>/`). A
  rebuild makes its predecessor the leftover and nothing else, so pruning again right after a rebuild removes
  nothing: no ping-pong between the main checkout's build and the next lane;
- a unit built by a **different `rustc`** than the units written last (all those within an hour of the newest
  write) is stale whatever its age: a toolchain upgrade, Cargo rebuilds it anyway. When those last writes
  disagree on the compiler (a stable and a nightly build in the same hour) nothing is judged by it;
- **never judged, always kept**: a unit whose fingerprint cannot be read or parsed (or whose write time cannot
  be read), and the zeroed run of a build script (`target`, `path` and `profile` are all `0`: the runs of two
  versions of one package cannot be told apart);
- a unit of a package the main checkout's `Cargo.lock` files (root and each first-level directory) no longer
  list — a removed dependency — is stale whatever its age. No lock read means no such rule, never "everything
  is stale".
[src: file: backend/src/core/seed_prune.rs:267-342#stale_units]
[src: file: backend/src/core/task_worker_lane_warm.rs:577-595#main_checkout_locked_packages]

`incremental/` is left alone: its directories are keyed by crate name, and a library and its tests share one
and are both current. Pruning a seed at worst makes a lane rebuild one crate it would have found warm: it
never changes what a build produces.

**Never while a build uses it.** Cargo's own build lock (`debug/.cargo-lock`) is taken *exclusively* to prune;
a held lock, shared or exclusive, makes the prune a no-op and the seed is not cloned either (the lane starts
empty, as before). The lock is then handed back *shared* for the clone, so a build that starts waits instead of
changing what is being cloned. The result (units, entries and bytes removed) is recorded on the lane's
`lane_warm_start` event (`seed.pruned`) and logged.
[src: file: backend/src/core/task_worker_lane_warm.rs:598-650#seed_lane]

Only the main checkout's `target/` is ever a seed, and so the only one ever pruned: a lane's own target is
never a seed source (a worker may write it, KT-1250).

## Real and apparent sizes (KT-1691)

Every size now comes in two figures.

| Field | Means |
|---|---|
| `bytes` | **apparent**: the sum of the file sizes. A lane cloned from the seed counts the blocks it shares with the seed in full, so 33 lanes read 1.85 TB on a disk that held a fraction of it |
| `unique_bytes` | **real**: the blocks no other file shares, what deleting the entry frees. `null` when the file system cannot say |

The real size is APFS's own `ATTR_CMNEXT_PRIVATESIZE`, read per file with `getattrlist(2)`. Elsewhere (no such
call for reflinks) it is `null`, and the report, the panel and the MCP tool label the apparent figure as
apparent instead of guessing: a real total is `null` as soon as one entry of the set could not say. Real
sizes are per entry: deleting two lanes that share blocks with each other frees more than the sum of their
real sizes, never less. A walk that hit its budget is still a floor (`bytes_are_partial`).
[src: file: backend/src/core/disk_usage.rs:314-372#private_size]

Totals by kind, the report (`apparent_bytes`, `unique_bytes`, `orphan_unique_bytes`) and every purge item carry
both. A purge reports `freed_bytes` (apparent) and `freed_unique_bytes` (real) and the volume's free space
before and after it: the difference is the truth. The **pressure guard** reads the volume's free space (what
the file system says is free, never a sum of sizes) and reports what its sweep gave back in *real* bytes
(`swept_bytes`), so "swept 190 GB and still refused" cannot happen on cloned lanes.
[src: file: backend/src/api/disk.rs:579-611#sum_unique]

## When something is removed

| Event | What goes | Guard |
|---|---|---|
| Execution **Done** (integrated), clean worktree removed | the lane build dir, with the worktree | `cleanup_integrated_execution` |
| Execution **Cancelled**, `RemoveIfClean`, clean checkout | the lane build dir, with the worktree | `settle_cancelled_workspace` |
| Execution **Cancelled**, checkout **dirty** or policy `Preserve` | nothing at cancel time: the worker may still be writing | released later by the terminal-checkout release |
| Boot and **every 30 min**: a **Done, Failed or Cancelled** execution whose managed checkout is still on disk | the checkout (branch kept) and its lane build dir | the terminal-checkout release below |
| Boot and **every 30 min**, and **before a Claude launch is refused for the worktree cap**: a finished execution whose checkout is **clean** | the checkout (branch kept) and its lane build dir, with no idle window | `reclaim_clean_finished_checkout` (KT-1646) |
| A task is set to **done or archived** (by hand, or any update), and at boot / every 30 min: an execution of a closed task that never finished, or one kept by a `Preserve` cancel, whose checkout is **clean** and unused | the checkout (branch kept) and its lane build dir; the execution is not rewritten | `release_closed_task_checkouts`, `release_closed_task_executions` (KT-1676) |
| A managed checkout **removed outside Kronn** (`git worktree remove`, `rm -rf`) | nothing on disk: the workspace record stops being `attached` (`missing` for a worker's, retired for a finished one), the dead Git entry is pruned | `reconcile_missing_managed_checkouts` (KT-1676) |
| Boot: an orphaned managed checkout removed | its lane build dir, with it | `reconcile_at_boot` |
| Worktree **kept** (diverged, over the evidence cap, written recently) | nothing: the lane is kept with it | same policy as the worktree |
| Boot, **every 30 min**, and under pressure | **orphan** lane build dirs | the sweeper rules below |
| **Never automatic**, only a confirmed per-entry delete in Settings → Disk | `foreign` build dirs, ownerless worktrees and their `target/`, **stalled** build dirs | *Foreign, ownerless and stalled* below |

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

### Worktree cap at launch (KT-1646)

A Claude task worker is refused (`claude_sandbox_catalogue_unsafe`) once the repository lists more than 64 Git
worktrees: the sandbox policy enumerates them. The cap is not raised. When a launch (or `task_exec_prepare`) is
over it, Kronn first records the worktrees removed outside Kronn as gone (and prunes their Git entries), then
removes the **clean** checkouts of finished (Done, Failed, Cancelled) executions of that project **and of
executions of closed tasks** (KT-1676), branch kept, measures again, and refuses only if it is still over. The
refusal then names every worktree that blocks, by directory name only, each with its reason, and the action that
frees it: `finished task, not clean` (uncommitted changes, another branch or unreadable), `task still running`,
`attached by a CLI session, not managed by Kronn`, `not created by Kronn`. Close the task, or review the
worktree and run `git worktree remove <name>` (commit or discard uncommitted changes first). Worktrees that
belong to no finished execution (older lanes, discussion worktrees) are never touched. The catalogue listing
(`task_worker_catalogue`) is advisory and does not reclaim.

### Closed tasks (KT-1676)

Closing a task (`done` or `archived`) used to leave its worktrees behind when its execution never reached a
terminal state (a task set to `done` by hand), and a cancel with `preserve` kept its worktree for good. Now the
task update releases the managed checkouts of the task's executions in the background, and the boot and 30-minute
sweeps do the same for tasks closed without that hook. The same guards as above apply, plus: the execution is
not mid-integration (`Provisioning`, `Approved`, `Integrating`, `Validating`, `Applying`), has no pending or
running dispatch, no unsettled commit lease and no active session. Only a **clean** checkout goes; a dirty one
is kept and its reason recorded once (`worktree_release_kept`). A preserved checkout therefore waits at most for
the idle window of the terminal-checkout release, or for its task to close. The execution itself is never
changed by the release.

**"Clean" means `git status` clean.** Ignored files do not count: a Rust `target/` or `node_modules/` inside a
checkout that `.gitignore` covers goes with the worktree, and the lane build dir (`target/wt-<lane>`) is purged
with it. A checkout with any modified or untracked (non-ignored) file is kept.

The release runs in three steps so that a slow removal never freezes the API: under the database connection the
execution is picked (finished, no live session, no worker lease) and its checkout **claimed**; with no connection
held, Git checks the branch and `git status` and removes the worktree; under the connection again the outcome is
recorded (`worktree_released`, workspace row retired). While the claim is held no second release takes the
checkout, and a reassign, a resume or an extension of the review rounds of that execution is refused ("its
checkout is being released") until the release has settled. The claim is in memory, not durable: a crash drops it
and the next sweep judges the checkout again. A finished execution cannot be reassigned or resumed at all
(`reassign_execution_worker_keeping_origin` and `extend_review_rounds` refuse a terminal one), so there is no
released checkout to provision again; the claim guards the gap between the check and the removal.

[src: file: backend/src/api/orchestration.rs:4760-4772#cleanup_integrated_execution]
[src: file: backend/src/api/orchestration.rs:14394-14405#settle_cancelled_workspace]

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
[src: file: backend/src/core/worktree.rs:976-1012#lane_target_last_write]
[src: file: backend/src/api/disk.rs:151-162#lane_entry_state]

The periodic sweep is started at boot, after the boot reconciliation:
[src: file: backend/src/api/disk.rs:1207-1229#spawn_periodic_sweep]

## Foreign, ownerless and stalled (KT-1691)

Three families were listed but never removable. They are now offered **one entry at a time, with their age,
behind a confirmation, never by a sweep** (the default purge and the periodic sweep still take orphans only).
Each is re-judged at deletion time, a running Cargo build (its build lock) refuses any of them, and nothing of
a live execution is ever offered.

| Entry | When it is purgeable | What a purge removes |
|---|---|---|
| **`foreign`** build dir (`target/wt-*` with no `.kronn-lane.json` marker and no workspace lineage) | not written for `server.disk_sweep_idle_hours` (a build may be running in a fresh one), and no live execution claims its lane name | the directory. Kronn did not create it, so the panel warns to be sure it is not yours |
| **ownerless** worktree (`.kronn/worktrees/<name>` that **no** workspace row names, by checkout path, remembered path or directory name) and its `target/` | not written for the same idle window | the `target/` alone (regenerable), or the whole checkout through `git worktree remove` **without** `--force`: uncommitted or untracked work is refused, the branch stays |
| **`stalled`** build dir (an execution `Blocked` or `Interrupted`) | idle for `server.disk_stalled_build_days` (default **3**, minimum 1) *and* durable state shows nothing working for the execution: no pending or running dispatch, no unsettled commit lease, no attached session, no held worker lease | the build dir only. The worktree and branch stay, and the next resume rebuilds the lane (a seed clone plus its first build) |

A checkout a row names stays the business of the flow that owns it (integration, cancel, terminal release):
asking for its purge is refused. A `Working`, `AwaitingReview`... execution's lane is `active` whatever its age.
The removal of a stalled lane is recorded on its execution (`lane_target_reclaimed`).
[src: file: backend/src/api/disk.rs:158-215#lane_entry_state]
[src: file: backend/src/api/disk.rs:242-270#ownerless_entry_state]
[src: file: backend/src/db/orchestration.rs:577-620#stalled_execution_blocker]
[src: file: backend/src/db/orchestration.rs:643-660#workspace_row_names_path]
[src: file: backend/src/core/worktree.rs:507-570#remove_ownerless_worktree]

## The launch guard

Before provisioning a task worker (`provision_task_execution_inner`, phase A′) and before a **full audit**,
Kronn reads the free space of the volume holding the worktrees and `target/` (O(1), no walk). Below
**`server.disk_launch_min_free_gib`** (default **30**; `0` turns the guard off):

1. it runs the orphan sweep — the launching project first, then the others;
2. still under the threshold: it **refuses** the launch with the number, the setting and what the sweep
   gave back, **before creating anything**. A worker launch parks its execution `Blocked` (resumable once
   space is free) and posts a system message in the **principal room**.

[src: file: backend/src/api/disk.rs:1132-1200#ensure_launch_headroom]
[src: file: backend/src/api/orchestration.rs:10592-10615#post_disk_pressure_alert]

Until a config is published at boot the guard is off (`0`), so a process that never loaded one — a unit
test — is never refused by a threshold nobody chose.
[src: file: backend/src/core/disk_usage.rs:106-115#set_disk_policy]

The older hard floor still applies underneath: `server.disk_critical_gib` (default 5) refuses worktree
provisioning and builds; see [worktree-build-artifacts.md](worktree-build-artifacts.md#thresholds).

## Seeing it: API, MCP tool, UI

- `GET /api/maintenance/disk[?project_id=…]` — free space (smallest among the projects' volumes), the
  policy, `totals` by kind, and every `entries[]` largest first: `kind`, `path`, `bytes`
  (`bytes_are_partial` = a floor: the walk is bounded in entries and time), `idle_seconds`, `owner`
  (`task_ref`, `execution_id`, `status`), `state` and `purgeable`.
  [src: file: backend/src/api/disk.rs:614-629#build_disk_report]
  An agent's bridge token gets a **scoped view** (KT-1347): free space, `policy`, `totals` and `orphan_*`
  stay whole-instance aggregates, but `volumes` and `entries` (paths, lane names, task refs) are those of
  the token's own project only — none for a project-less launch — and the report carries `scoped: true`.
  Naming another project in `?project_id=` is refused (403). The operator and the UI keep the full report.
  [src: file: backend/src/api/disk.rs:668-677#scoped_for_bridge]
- `POST /api/maintenance/disk/purge` — no `paths`: purge **orphans only**. With `paths`: delete exactly
  those entries, and only with `confirm: true`; each is re-judged at deletion time, so a report never
  authorises a delete. **Human / UI only**: the route is deliberately not on `BRIDGE_ROUTES`, so a bridge
  token is refused (403). It deletes host files; even an own-project orphan purge was not worth giving to
  an agent, which can tell the user instead. [src: file: backend/src/api/disk.rs:1281-1293#run_purge]
- MCP tool **`disk_usage`** (`project_id`) — the scoped report above, read-only, bounded to the 25 largest
  entries. It never calls the purge; a leftover `purge_orphans: true` argument returns a note that purging
  is a human action in Settings > Disk. [src: file: backend/scripts/disc-introspection-mcp.py:7831-7863#call_disk_usage]
- **Settings → Disk** — measure on demand (never on page open), totals by kind, the table, **Purge
  orphans**, a per-entry delete behind a confirmation, and the two thresholds.

`state` is one of `orphan` (the default purge takes it), `reclaimable` (finished but recent, or its clean
worktree is kept: only a confirmed per-entry delete), `active` (a live execution claims it: **never
removed**), `kept` (dirty worktree, unreadable state; an *ownerless* worktree is `kept` too, and `purgeable`
once quiet), `foreign` (not Kronn's: no sweep ever touches it, a quiet one is `purgeable` by a confirmed
per-entry delete) and `stalled` (an execution Blocked or Interrupted whose build dir has been idle for
`server.disk_stalled_build_days`: confirmed per-entry delete only). `purgeable` is what the per-entry
Purge button reads.
[src: file: backend/src/core/disk_usage.rs:203-218#DiskState]

## Settings

| Key | Default | Meaning |
|---|---:|---|
| `server.disk_launch_min_free_gib` | 30 | free GiB a worker launch / full audit needs; sweep first, then refuse |
| `server.disk_sweep_idle_hours` | 6 | hours (minimum 1; `0` is read as 1) a lane must be unwritten before the sweeper may take it |
| `server.disk_stalled_build_days` | 3 | days (minimum 1; `0` is read as 1) a Blocked or Interrupted execution's build dir must be unwritten before it is offered for a purge (KT-1691) |
| `server.disk_warning_gib` / `disk_critical_gib` | 20 / 5 | KT-373 floor, unchanged |

The first three keys are editable in Settings → Disk or `POST /api/config/server`, and apply without a restart.

## Limits

- **Pre-existing lanes carry no marker.** Those that a managed workspace row still names are attributed
  and swept like any orphan. The others (no row, or a row of an unmanaged workspace) show as `foreign`: no
  sweep takes them, a quiet one can be deleted from the panel, one entry at a time.
- **Seed pruning is by configuration, not by crate.** Two feature sets of one crate stay, however far apart
  they were built, so the seed can hold a few variants a build no longer uses (they are only superseded by
  a rebuild of the same configuration). On the 894-unit seed of 2026-11-12 (one rustc) the rule removes 77
  units (415 `deps/` and `build/` entries, about 0.5 GB apparent of 13 GB), not the 326 a per-crate rule
  would have pruned. Alternating two dependency states in the main checkout (a branch switch back and
  forth) still costs a lane one rebuild per switch. A removed dependency is only recognised through the main
  checkout's `Cargo.lock`; `incremental/` and artifacts no fingerprint names are never pruned.
- **Real sizes exist on APFS only.** On another file system the panel shows apparent sizes, labelled as such.
  A real size is per entry: it never adds up lanes that share blocks with each other.
- A checkout over the evidence cap, or recently written, stays until a human acts or it goes idle; the
  evidence patches under `task-evidence/` are never purged by Kronn.
- Sizes are floors on a very large target: the report says `≥` rather than stall (1.69 million files took
  44 minutes to walk in the 2026-08-21 incident).
- The guard measures free space *before* the launch; it cannot stop a build that fills the disk later. The
  idle threshold and the launch threshold exist so that a full disk is refused early instead.
- Bench runs have no launch endpoint of their own: they go through the audit launch, which is guarded.

## Tests

Unit: ownership and age rules (`core/worktree.rs` idle tests, `api/disk_test.rs`: foreign, ownerless and
stalled states, apparent vs real totals), the guard's decision, the bounded measure and the real (unique)
size of an APFS clone (`core/disk_usage_test.rs`), the seed prune on real fingerprint JSON shapes: variants of a crate kept, an identical
identity pruned, a changed `rustc` pruned, an unparseable fingerprint kept, no ping-pong after a rebuild,
nothing pruned by age alone, removed dependencies, never while a build holds the lock (`core/seed_prune_test.rs`), and a lane that
never clones a stale seed artifact (`core/task_worker_lane_warm_test.rs`). End to end (`api/orchestration.rs`): an aged orphan is
swept and a fresh, foreign or live-execution lane never; a launch under pressure sweeps first, then is
refused, parks the execution and alerts the principal room. KT-1691 adds, on a real repository and database:
a Blocked or Interrupted execution's idle build dir purgeable and a live, attached or recent one never, a
foreign `wt-*` and an ownerless worktree (clean, dirty, its `target/`) purgeable per entry and a task's own
checkout never, and the real bytes a cloned lane reports and the guard sweeps.
