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

### Recurrence on 2026-09-13 — KT-638

During 0.13.0 qualification, the protected KT-635 candidate was not applied:
its complete library run recorded 5,946 passes and 131 failures, each a
worktree-provisioning refusal below the 5 GiB critical threshold. The saved
failure log therefore identified disk pressure rather than independent product
regressions; the threshold was not lowered. [src: user: 2026-09-13: KT-638 incident record]

The authorised recovery paused owned build activity and reclaimed only named,
regenerable targets after ownership, live-use, path and Git-state checks. It
preserved sources, worktrees, Git state, databases, host settings and the
running backend; the later maintenance inventory rechecked eligibility before
reclaiming its one named managed target. The final recorded free-space reading
was 1,449,400,604 KiB. This recovery is evidence of a manual exception, not
authority for automatic deletion of interactive or unknown targets. [src: user: 2026-09-13: kt638-old-targets-maintenance-013]

The recurrence also exposed that non-terminal old executions and unregistered
manual worktrees do not become reclaimable merely because a planning task is
done. KT-638 keeps prevention open: it does not lower disk thresholds, ignore
tests, scan recursively on a failing request, or authorise deletion of active
or unowned targets. [src: file: backend/src/core/worktree.rs:451-558]

The retained maintenance record distinguishes each authorised batch. These are
dated physical free-space observations, not quotas or reusable deletion grants:

| Batch | Available space afterwards | Preserved boundary |
| --- | ---: | --- |
| Principal qualification incremental cache | 14,919,256 KiB | Main backend/watcher and worker targets untouched |
| Remaining principal debug artifacts | 91,250,360 KiB | Sources, Git, independent logs and active workers retained |
| Entire main `target/debug`, then backend-only offline rebuild | 517,517,132 KiB | Running executable backed up; original backend process stayed online |
| Eligible KT-486 maintenance API target | 523,800,688 KiB | Other inventory entries refused/untouched |
| Explicitly authorised 27 old targets | 1,449,400,604 KiB | Fixed inventory; live-use and unchanged Git state verified per target |

The main-cache batch rebuilt only the backend with command-local
`CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0`, in 1 min 29 s; its rebuilt
debug directory reported 1.85 GiB. The final 27-target batch freed about
882.73 GiB, without rebuilding or restarting a service. Twenty targets were
under the managed root (six escalated, fourteen without recorded ownership),
and seven belonged to external worktrees. Human confirmation of inactivity
authorised that manual exception; automatic eligibility was not widened.
Sources, worktrees and non-integrated work were not removed. Such duplication
can affect other Rust projects too, but Kronn's approved versioned profile
change does not alter their Cargo settings.
[src: user: 2026-09-13: kt638-old-targets-maintenance-013]
[src: user: 2026-09-13: kt638-kronn-only-profile-approval-013]

Continuous learning was disabled, so no proposal became an accepted automatic
memory. KT-638, this document, and room
`85513703-3169-451d-88e3-e03fbaac34b1` retain the incident record. KT-639 covers
storage visibility and sorting/filtering separately and was explicitly deferred
from 0.13.0. None of these records authorises another purge.
[src: user: 2026-09-13: kt639-release-scope-013]

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
deployment has changed either server disk threshold, the operator must supply
the same approved values through `KRONN_DEV_BACKEND_DISK_WARNING_GIB` and
`KRONN_DEV_BACKEND_DISK_CRITICAL_GIB` rather than reading a host configuration
file from the shell. [src: file: scripts/dev-backend-supervisor.sh:16-23]
[src: file: scripts/dev-backend-build-guard.sh:8-49]

Kronn's versioned backend and desktop development profiles use line-table-only
debug information, disable incremental compilation, and explicitly retain
debug assertions and overflow checks. A developer needing full debug info may
use Cargo's explicit command-line override, for example
`cargo build --config 'profile.dev.debug=true'`; this is a local choice and is
not a global Cargo setting. [src: file: backend/Cargo.toml:131-143]
[src: file: desktop/src-tauri/Cargo.toml:44-51]

Qualification validation resolves a Cargo validation's effective target through
`cargo metadata` before Quick Exec is allowed to spawn its build. The metadata
call retains the validation's `--manifest-path`, `--config`, and
`--target-dir` inputs, runs offline with a bounded deadline, so root-launched
backend gates measure the same target filesystem as Cargo. A new target is
created only at Cargo's exact target path when its immediate parent is already a
real directory; an existing symlink, non-directory, or inspection failure is
refused. Non-Cargo
validations use their declared working directory. A critical-space refusal is
stored as a refused validation rather than a pass. [src: file: backend/src/core/worktree.rs:491-558]
[src: file: backend/src/api/orchestration.rs:2480-2585]

There is intentionally no automatic lifecycle cleanup for the interactive
target or unknown qualification caches. The only automatic reclamation remains
the existing managed-worktree path: ownership, terminal execution state,
attached sessions, and worker leases are all checked before deletion. [src: file: backend/src/core/worktree.rs:299-370]
[src: file: backend/src/db/orchestration.rs:439-531]

For an interactive or qualification cache outside that managed ownership path,
the bounded policy is refusal and escalation: the build guard names its exact
target and configured critical threshold, while the maintenance inventory lists
managed candidates and their refusal reason. An operator may request a manual,
path-specific reclaim only after the inventory's ownership and terminal-state
checks; this document does not authorise a broad purge or deletion of unknown,
active, or leased targets. [src: file: backend/src/core/worktree.rs:451-558]
[src: file: backend/src/api/orchestration.rs:2588-2680]
[src: file: backend/src/db/orchestration.rs:439-531]

## KT-638 profile benchmark record

The principal ran both profiles on September 13, 2026, at the same clean
commit `624df2894d57e83e491f5fd76c29e8e51fd55d5d` (backend tree
`a542a367b8a2015ff88da0de4b651305a86bc8f8`). Both used Cargo 1.98.0 and
rustc 1.98.0, two jobs, offline locked dependencies, and only the `kronn`
backend binary. Debug assertions and overflow checks remained enabled in both.
Each profile had its own initially absent target directory; no cache was
deleted and no backend was started. [src: commit: 624df2894d57e83e491f5fd76c29e8e51fd55d5d]

| Phase | Historical debug=2, incremental=true | New line-tables-only, incremental=false |
| --- | ---: | ---: |
| Cold: wall time / allocated target | 254.182 s / 6,097,596 KiB | 217.998 s / 3,434,628 KiB |
| Warm, unchanged source | 0.457 s / 6,097,596 KiB | 0.447 s / 3,434,628 KiB |
| Touch only `main.rs`, then rebuild | 1.689 s / 6,134,920 KiB | 2.352 s / 3,443,268 KiB |

All six commands exited zero, with identical tracked source before and after;
the touch phases restored the original source timestamps. In this scenario,
the final target decreased from 5.851 GiB to 3.284 GiB (43.87% less), while the
small source-touch rebuild was 0.663 s slower. These are single local
observations, not CI SLO samples or a general performance guarantee. Cold means
an empty Cargo target, not a purged OS/registry cache; independent qualification
work could run in the background. Desktop builds and the complete test-target
footprint were not part of this size comparison.
[src: file: docs/releases/0.13.0-rust-profile-benchmark.json:1]

The new binary also emitted a macOS linker warning that `__eh_frame` exceeded
the compact-unwind table's 16 MiB encoding limit and exception-handling
performance might be affected. It did not fail the build, but this record does
not classify that warning as resolved or claim warning-free linking. Strict
all-target Clippy on the candidate had separately passed; it is not a substitute
for the linked-binary result.
[src: file: docs/releases/0.13.0-rust-profile-benchmark.json:1]

To reproduce the profile inputs with new owned targets:

```text
CARGO_TARGET_DIR=<owned-old-target> CARGO_INCREMENTAL=1 CARGO_PROFILE_DEV_DEBUG=2 CARGO_PROFILE_DEV_INCREMENTAL=true CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=true CARGO_PROFILE_DEV_OVERFLOW_CHECKS=true cargo build --manifest-path backend/Cargo.toml --offline --locked --bin kronn -j 2
CARGO_TARGET_DIR=<owned-new-target> CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_DEV_INCREMENTAL=false CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=true CARGO_PROFILE_DEV_OVERFLOW_CHECKS=true cargo build --manifest-path backend/Cargo.toml --offline --locked --bin kronn -j 2
```

The local helper, six immutable JSON receipts, and separate raw stdout/stderr
logs are retained in `/private/tmp/kt638-profile-benchmark.utIscv`. Relevant
compiler/profile overrides were removed from the child environment before the
listed values were installed; system home variables were never changed. The
versioned receipt below preserves the compact results independently of that
temporary evidence directory. [src: file: docs/releases/0.13.0-rust-profile-benchmark.json:1]

This measurement completes the profile comparison, not release qualification.
The combined unfiltered backend, frontend, Python, shell, desktop and browser
gates remain separate requirements; host-sync test confinement must be proven
before running the full backend suite. [src: file: docs/testing-quality.md:35-61]

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
