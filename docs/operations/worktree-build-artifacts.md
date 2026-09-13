# Worktree build artefacts — ownership and cleanup policy

**Status:** in force (2026-08-22) · **Origin:** KT-373, after the 2026-08-21 disk-full incident

## What happened, and why the policy is shaped this way

The dev volume reached 100% with 753 MiB free. Seven Kronn worktrees were each
holding their own Rust `target/`, 7.5 to 24.6 GiB apiece. Nothing was
misbehaving: provisioning had simply never been asked to care about disk, and
`target/` is regenerable, so no component considered itself its owner.

Two details from the recovery drive everything below.

**A process scan does not prove a worktree is idle.** `Kronn-kt320/target` was
cleaned because no `cargo` or `rustc` was visible and no working path was open.
The worktree in fact belonged to a live agent that was between builds. Nothing
was lost — sources and Git state were untouched — but the classification was
wrong, and the same reasoning applied to a worktree mid-integration would be
destructive.

**Walking a full `target/` is itself expensive.** The main debug target held
1 689 865 stale files; deleting them one by one took over 44 minutes of APFS
I/O. Any check that runs on a failing path must not be the thing that walks it.

## Ownership

| Path | Owner | Cleanable |
|---|---|---|
| `<repo>/.kronn/worktrees/<name>/target/` | Kronn, via the task execution that provisioned the worktree | Yes, once the execution is terminal |
| `<repo>/target/` | The developer and their watcher | **Never automatically** — this is the interactive build |
| `CARGO_TARGET_DIR` under a temp dir | The run that set it | Yes, at the end of that run |
| Anything else | Not Kronn | Never |

"Managed" is not a filesystem pattern. A path qualifies only if
`assert_managed_task_worktree_path` accepts it: a direct child of
`<repo>/.kronn/worktrees`, with no symlink or reparse point anywhere between
the repo root and the target, canonicalised and re-checked against the managed
root.

## When a target becomes cleanable

A worktree's `target/` may be removed when **the durable execution state says
its execution is terminal** — `Done`, `Failed` or `Cancelled`
(`TaskExecutionStatus::is_terminal`, an exhaustive match, so a new status
cannot silently become cleanable).

The durable state is the **only** thing that may authorise a cleanup. A process
scan may still *refuse* one — a visible `cargo` is evidence of activity — but it
can never authorise it. This asymmetry is the direct lesson of the incident:
absence of a compiler is not evidence of absence of work.

Never cleaned, regardless of state: a worktree whose execution is missing from
the database (that is an inconsistency to report, not a directory to delete),
the repository's own `target/`, and any path the managed-path assertion refuses.

## What may be removed

Only `target/`, and only when it is a real directory. A `target` that is a
symlink is left alone: it points at storage this repository does not own.

Sources, Git state, `.git`, worktree metadata and any file outside `target/`
are never touched. Cleanup deletes work that a compiler can rebuild, and
nothing else.

## Cost rules

- Free space is read from the filesystem's own counter — O(1), no walk. This is
  what gates provisioning.
- An exact size is only computed when a human asks for an inventory, and the
  walk stops at a fixed entry budget, reporting a floor rather than stalling.
  A partial answer now beats an exact one after the disk fills.
- Nothing recursive ever runs on the provisioning path.

## Failure handling

A failure to resolve, lock, read or delete stops that target and surfaces a
diagnostic naming the path and the reason. It never proceeds to the next target
silently, and it never falls back to a broader deletion.

## Thresholds

`server.disk_warning_gib` (default 20) logs a warning and continues.
`server.disk_critical_gib` (default 5) refuses provisioning, naming the setting
in the error so the operator knows which knob to turn. A warning configured
below the critical mark is raised to it: a contradictory config must not read
as "merely warn" on a disk that is in fact below the refusal line.

## Native development and qualification builds

The native backend supervisor exports `CARGO_TARGET_DIR` as
`<checkout>/target`; it does not inherit a target directory from another
checkout. Before its initial and watched `cargo build`, it checks free space on
that exact directory's filesystem and refuses without starting Cargo below the
critical limit. The guard never removes files. Its default is 5 GiB; when a
deployment has changed `server.disk_critical_gib`, the operator must supply the
same approved value through `KRONN_DEV_BACKEND_DISK_CRITICAL_GIB` rather than
reading a host configuration file from the shell. [src: file: scripts/dev-backend-supervisor.sh:16-23]
[src: file: scripts/dev-backend-build-guard.sh:8-31]

Kronn's versioned backend and desktop development profiles use line-table-only
debug information, disable incremental compilation, and explicitly retain
debug assertions and overflow checks. A developer needing full debug info may
use Cargo's explicit command-line override, for example
`cargo build --config 'profile.dev.debug=true'`; this is a local choice and is
not a global Cargo setting. [src: file: backend/Cargo.toml:131-143]
[src: file: desktop/src-tauri/Cargo.toml:44-51]

Qualification validation resolves a Cargo validation's effective target through
`cargo metadata` before Quick Exec is allowed to spawn its build; an explicit
`--target-dir` is preserved in that lookup. Non-Cargo validations use their
declared working directory. An unresolved or non-directory target is refused
instead of measuring an unrelated parent filesystem; a critical-space refusal
is stored as a refused validation rather than a pass. [src: file: backend/src/core/worktree.rs:491-518]
[src: file: backend/src/api/orchestration.rs:2480-2552]

There is intentionally no automatic lifecycle cleanup for the interactive
target or unknown qualification caches. The only automatic reclamation remains
the existing managed-worktree path: ownership, terminal execution state,
attached sessions, and worker leases are all checked before deletion. [src: file: backend/src/core/worktree.rs:299-370]
[src: file: backend/src/db/orchestration.rs:439-531]

## Maintenance command

Two routes, delivered by KT-373, let an operator inspect and reclaim the
build artefacts described above without writing a script.

### `GET /api/maintenance/build-artifacts?project_id=…` — the dry-run

Lists every candidate target for the given project. For each target the
response carries:

- the **path** (`<repo>/.kronn/worktrees/<name>/target/`);
- the **state** — `reclaimable`, or the reason it is refused (execution not
  terminal, path not managed, `target` is a symlink, execution missing from the
  database, …);
- the **age** of the target;
- the **estimated reclaimable space**.

**Refused targets are listed, with their reason.** A refusal is not a silent
skip: the operator sees every target and why it was not reclaimed, so a
"nothing to do" answer is auditable rather than a guess.

### `POST /api/maintenance/build-artifacts/reclaim` — the action

Acts **only on the `target_paths` named by the caller**. There is no "clean
everything" verb: the caller must enumerate the exact paths to reclaim, and
each one is re-judged against the durable state at the moment of deletion.

### The dry-run does not authorise

The dry-run is a read-only inventory. It **does not authorise** a deletion:
each target is re-judged against the durable execution state at the moment of
the `reclaim` call. A target that was `reclaimable` in the dry-run may be
refused at reclaim time if its execution is no longer terminal, and a target
that was refused in the dry-run is refused again at reclaim time. The durable
state is the only thing that may authorise a cleanup; the dry-run is only a
preview of it.
