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
or unowned targets. [src: file: backend/src/db/orchestration.rs:429-542#worktree_cleanup_liveness] [src: file: backend/src/core/worktree.rs:1263-1327#ensure_build_disk_headroom]

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

Two further settings (KT-1318) make a launch sweep orphaned lane build dirs and
then refuse below `server.disk_launch_min_free_gib` (default 30), and set how
long such a dir must be idle (`server.disk_sweep_idle_hours`, default 6); see
[disk-usage.md](disk-usage.md).

## Native development and qualification builds

The native backend supervisor exports `CARGO_TARGET_DIR` as
`<checkout>/target`; it does not inherit a target directory from another
checkout. Before its initial and watched `cargo build`, it checks free space on
that exact directory's filesystem and refuses without starting Cargo below the
critical limit. The guard never removes files. Its default is 5 GiB; when a
deployment has changed either server disk threshold, the operator must supply
the same approved values through `KRONN_DEV_BACKEND_DISK_WARNING_GIB` and
`KRONN_DEV_BACKEND_DISK_CRITICAL_GIB` rather than reading a host configuration
file from the shell. [src: file: scripts/dev-backend-supervisor.sh:19-22#CARGO_TARGET_DIR] [src: file: scripts/dev-backend-supervisor.sh:140-163#build_backend] [src: file: scripts/dev-backend-watch-command.sh:24-30#dev-backend-build-guard]
[src: file: scripts/dev-backend-build-guard.sh:8-48#KRONN_DEV_BACKEND_DISK_CRITICAL_GIB]

Kronn's versioned backend and desktop development profiles use line-table-only
debug information, disable incremental compilation, and explicitly retain
debug assertions and overflow checks. A developer needing full debug info may
use Cargo's explicit command-line override, for example
`cargo build --config 'profile.dev.debug=true'`; this is a local choice and is
not a global Cargo setting. [src: file: backend/Cargo.toml:168-179#profile.dev]
[src: file: desktop/src-tauri/Cargo.toml:49-55#profile.dev]

Qualification validation resolves a Cargo validation's effective target through
`cargo metadata` before Quick Exec is allowed to spawn its build. The metadata
call retains the validation's `--manifest-path`, `--config`, and
`--target-dir` inputs, runs offline with a bounded deadline, so root-launched
backend gates measure the same target filesystem as Cargo. A new target is
created only at Cargo's exact target path when its immediate parent is already a
real directory; an existing symlink, non-directory, or inspection failure is
refused. Non-Cargo
validations use their declared working directory. A critical-space refusal is
stored as a refused validation rather than a pass. [src: file: backend/src/core/worktree.rs:1263-1327#ensure_build_disk_headroom]
[src: file: backend/src/api/orchestration.rs:6891-6948#validation_build_path]
[src: file: backend/src/api/orchestration.rs:6950-7046#validation_metadata_command]
[src: file: backend/src/api/orchestration.rs:6739-6756#run_one_validation_with]
[src: file: backend/src/api/orchestration.rs:7175-7241#run_one_validation_checked_with_runner]

There is intentionally no automatic lifecycle cleanup for the interactive
target or unknown qualification caches. The only automatic reclamation remains
the existing managed-worktree path: ownership, terminal execution state,
attached sessions, and worker leases are all checked before deletion. [src: file: backend/src/core/worktree.rs:293-388#clean_worktree_build_artifacts]
[src: file: backend/src/db/orchestration.rs:429-542#worktree_cleanup_liveness]

For an interactive or qualification cache outside that managed ownership path,
the bounded policy is refusal and escalation: the build guard names its exact
target and configured critical threshold, while the maintenance inventory lists
managed candidates and their refusal reason. An operator may request a manual,
path-specific reclaim only after the inventory's ownership and terminal-state
checks; this document does not authorise a broad purge or deletion of unknown,
active, or leased targets. [src: file: backend/src/core/worktree.rs:1222-1257#ensure_disk_headroom_for] [src: file: scripts/dev-backend-build-guard.sh:41-44#critical_gib]
[src: file: backend/src/api/maintenance.rs:125-190#list_build_artifacts]
[src: file: backend/src/db/orchestration.rs:429-542#worktree_cleanup_liveness]

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
[src: file: docs/releases/0.13.0-rust-profile-benchmark.json:11-28#measurements]

The new binary also emitted a macOS linker warning that `__eh_frame` exceeded
the compact-unwind table's 16 MiB encoding limit and exception-handling
performance might be affected. It did not fail the build, but this record does
not classify that warning as resolved or claim warning-free linking. Strict
all-target Clippy on the candidate had separately passed; it is not a substitute
for the linked-binary result.
[src: file: docs/releases/0.13.0-rust-profile-benchmark.json:15-20#limitations]

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
temporary evidence directory. [src: file: docs/releases/0.13.0-rust-profile-benchmark.json:13#raw_evidence_directory]

This measurement completes the profile comparison, not release qualification.
The combined unfiltered backend, frontend, Python, shell, desktop and browser
gates remain separate requirements; backend tests must run against their own
data directory, never the real install. [src: file: docs/testing-quality.md:44-62#test-backend] [src: file: docs/testing-quality.md:102-104#ci-test.yml]
[src: file: docs/testing-quality.md:78-83#KRONN_DATA_DIR]

## Background priority of workers and validations (KT-1633)

A worker's builds and a validation's test run can keep every core busy for
minutes, and nothing used to lower their priority, so the owner's interactive
applications competed with them. Kronn now starts them in the background:

| Host | Mechanism |
|---|---|
| macOS | `/usr/sbin/taskpolicy -b <program> …` (background QoS: CPU, I/O, network) |
| Linux | `/usr/bin/nice -n 10 <program> …` (`/bin/nice` when that is where it lives) |
| Windows | the `BELOW_NORMAL_PRIORITY_CLASS` creation flag |

The launcher `exec`s the program, so the process id, the process group Kronn
kills on a timeout or a cancel, and the program's children (which inherit the
policy) are the same as without it. It sits in front of the OS sandbox, never
behind it.

**What runs in the background**

- A **task worker**'s CLI agent, on its first dispatch and on every re-dispatch
  after a reviewer's `request_changes` (a fix is the same worker again).
- The lane's **first build** (`cargo test --lib --no-run`, the warm-up), including
  under the OS sandbox: the priority travels on the probe command and the
  process runner applies it beneath the sandbox's own launcher.
- Every **validation** command of a task, confined or not.
- A workflow **Agent** step's CLI agent and a workflow **Exec** step's command
  (a heavy autoCode build runs there).

**What does not**

- The backend and the UI.
- Interactive discussions: the room principal's agent and any plain discussion
  turn. Reviews now run as read-only discussions that never build, so there is
  no separate review launch to lower.
- Quick Exec collectors the owner starts.
- The short bounded probes: `cargo metadata` (the validation target lookup),
  `cargo nextest --version` / `cargo fetch --offline` and the `vitest` smoke
  checks.
- Native ACP launches that carry no workflow-step or worker context.

**Turning it off.** Settings > Workers > « Run workers at background priority »
(`workers.background_priority` in `config.toml`, on by default). Saving it takes
effect at the next spawn, with no restart. `KRONN_BACKGROUND_PRIORITY=0`
(`false`, `off` and `no` work too, any case) in the backend's environment turns
it off as well: the mechanism runs only when both the setting and the
environment allow it.

**A host that refuses the launcher.** At boot, before any worker can be
dispatched, the backend runs the launcher once on `true`, on a blocking thread
(`check_launcher_at_boot`); a spawn only reads the stored answer and never waits
on a process. Only a definitive answer is kept. If the launcher is missing or
exits non-zero (a backend running inside an OS sandbox that forbids lowering a
priority gets `taskpolicy: setpriority(): Operation not permitted`), a warning
is logged once and workers and validations keep the backend's priority: a worker
at normal priority beats one that never starts. A transient failure (no process
slot, an interrupted call) is not stored: the boot probe retries, and until an
answer is known a spawn runs at normal priority while a re-check, at most every
30 seconds, runs on a thread of its own.

[src: file: backend/src/core/background_priority.rs:104-133#background_priority]
[src: file: backend/src/core/background_priority.rs:168-182#plan_here]
[src: file: backend/src/core/background_priority.rs:293-295#check_launcher_at_boot]
[src: file: backend/src/core/cmd.rs:359-370#async_cmd_with]
[src: file: backend/src/core/cmd.rs:395-404#sync_cmd_with]
[src: file: backend/src/core/task_worker_env.rs:849]
[src: file: backend/src/core/task_worker_lane_warm.rs:843]
[src: file: backend/src/agents/runner.rs:14711-14720#agent_launch_priority]
[src: file: backend/src/core/quick_exec.rs:516-540#build_command]
[src: file: backend/src/workflows/steps.rs:1411]
[src: file: backend/src/workflows/exec_step.rs:74-90#EXEC_STEP_PRIORITY]
[src: file: backend/src/core/config.rs:610-611]

## A failed lane warm-up is never silent (KT-1736)

On 2026-11-12 every lane's warm-up ended `first_build.outcome = "failed"` after
about 90 s (`task_execution_events`, `action = 'lane_warm_start'`), so each worker
met a cold lane (KT-1718 measured 41 min 43 s for its first
`cargo test --lib <filter>`). The stored reason was only the last 300 bytes of
`cargo test --no-run` — the end of the `cc` command line of the file that failed
(`…/aws-lc-sys-0.44.0/…/arm/aes/aes-xts-enc.S`) and cargo's
`warning: build failed` epilogue — never the compiler's own message, printed far
above. The warm-up ran to completion for the other jobs (hence the 90 s) and left
no `.kronn-warm.json`.

What was established, and what was not:

- The failure is specific to the warm-up's environment: the same crate builds
  from a clean lane in a worker's own shell (`cargo build -p aws-lc-sys`,
  1 min 04 s) and, replaying the warm-up's sealed environment without the OS
  sandbox, cargo does not even consider the crate stale on a lane seeded from the
  main checkout (no `dirty:` line for `aws-lc-sys` under
  `CARGO_LOG=cargo::core::compiler::fingerprint=info`; its build script only
  tracks `MACOSX_DEPLOYMENT_TARGET`, `CC`, `CFLAGS`, `AWS_LC_SYS_*`… and the
  seed's run recorded none of them). So the `.S` compile only runs, and fails,
  on a lane whose seed lacks a fresh `aws-lc-sys` build.
- The later lanes of the day end `timed_out`, not `failed`: the lane of this very
  task was seeded in 522 ms and its first build then ran 1 800 043 ms — the
  `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS` bound — without an `aws-lc-sys` error.
  `cargo test --no-run` rebuilt the worktree's own crate (a workspace crate is
  fingerprinted by its path, so a seed never covers it) for the lib test binary
  **and for every `tests/*.rs` binary**, which does not fit in 30 minutes on a
  loaded host. A targeted `cargo test --lib <filter>` needs the first of them
  only — see "What the warm-up builds" below.
- The failing file is an assembly source, and the cause is measured (13/11,
  unsandboxed shell, the worker's Seatbelt rules from `seatbelt_profile_with`,
  writable = one lane directory `W`, `TMPDIR=W/tmp`):

  | Replay of `cc -c …/arm/aes/aes-xts-enc.S -o W/b.o` | Result |
  |---|---|
  | worker profile, unchanged | `clang: error: unable to make temporary file: Operation not permitted` |
  | + `(subpath "/private/var/folders/<u>/T")` — the real path of `getconf DARWIN_USER_TEMP_DIR` | passes the temp stage (`/var/folders/…` without `/private` does **not** match) |
  | + `/private/tmp` | still fails |
  | worker profile, unchanged, `-save-temps=obj` | passes the temp stage |

  Apple clang compiles a `.S` as two jobs (preprocess to a temporary `.s`, then
  `-cc1as`). The first job's temporary file is created in the per-user Darwin temp
  directory, which the worker profile does not grant — a `.c` source has no such
  file, hence only the assembly sources fail. A first error line that names
  `unable to make temporary file` is therefore the signature of this cause.

**The fix: the temporary file stays in the lane.** The lane environment
(`WorkerLane::env`, so the warm-up, the sandboxed validations *and* the worker's
own shell) carries `CFLAGS=-save-temps=obj` on macOS: clang then writes its
intermediate files next to the object file, inside the lane's own target, and the
profile is **not** widened. The alternative — granting the real
`DARWIN_USER_TEMP_DIR` to the profile — would let worker-written code (a
`build.rs`, a test) write into a directory shared with every other process of the
owner's session, and was not taken.

- Checked with `aws-lc-sys` 0.44.0 and `libsqlite3-sys` rebuilt from an empty
  target with the flag (`CFLAGS=-save-temps=obj cargo build --offline --locked
  -p aws-lc-sys -p libsqlite3-sys`): exit 0 in 1 min 38 s, 629 saved `.s`/`.i`
  temps (`aes-xts-enc.s` among them) next to the objects. The crate's own probes
  (`is_flag_supported`, the `memcmp` check) are unaffected, so the flag is safe for
  the build scripts of the graph. That run was in a worker's sandbox, which does
  not reproduce the refusal, so it proves the flag breaks no build — not the
  refusal's absence; the refusal's absence under the profile is the replay row
  above.
- A lane whose seed was built without the flag recompiles its `cc` build scripts
  once (the `cc` crate tracks `CFLAGS` with `rerun-if-env-changed`): `aws-lc-sys`,
  `libsqlite3-sys`. That is the cost of the fix; `aws-lc-sys` was already being
  recompiled on the lanes that failed. Linux hosts are untouched: bubblewrap gives
  a private `/tmp`, so the variable is not set there.
- `CFLAGS` is a default: a command that sets its own (`probe.env`) wins.

**What the warm-up builds (decided by the owner, KT-1736).** The first build is
`cargo test --lib --no-run --locked --offline` — the dependency graph (the test
profile inherits `dev`, so its dependencies are the `dev` ones) and the crate's
own **lib test binary**: exactly what a worker's first targeted
`cargo test --lib <filter>` needs. It no longer builds every `tests/*.rs`
integration binary (each links the whole crate again; with them the warm-up ended
`timed_out` at the 30-minute bound on 12/11); the validations that run them build
them. A manifest with no library target (no `src/lib.rs`, no `[lib]`) keeps the
whole-package `cargo test --no-run`, since `--lib` would be an error there.

So the failure now explains itself the next time it happens:

- **The event keeps the first errors.** `ProcessProbeRunner` scans a probe's
  stderr while it streams and keeps its first 8 error lines (`error…`,
  `… : error`, `fatal error`, `error occurred`, `panicked at`; each clipped to 400
  characters around an ellipsis, so the file name at the end of a `cc` command
  survives) beside the last 300 bytes. A failed probe's reason reads
  `exited with exit status: 101: first errors: … | last output: …`. Every probe
  that goes through the process runner gets it, not only the warm-up.
- **The room says so.** When the first build fails or times out, the worker's
  room gets a system line before the worker is dispatched —
  `⚠️ préchauffage échoué : <cause>` (`⚠️ warm-up failed: <cause>` in an English
  room) followed by the fact that the worker starts on a cold lane. The cause is
  the recorded reason, clipped to 900 characters. A warm, cancelled or skipped
  first build posts nothing.

**Reproducing a warm-up by hand.** Replay the same command in the lane, under the
OS sandbox the warm-up uses
(`sandbox-exec -p <profile> cargo test --lib --no-run --locked --offline` from the
manifest directory, `TMPDIR=<lane target>/.kronn-tmp`, the lane's `CARGO_*` and
`CFLAGS` variables), then read the first error lines of the execution's
`lane_warm_start` event. A worker cannot do this itself: `sandbox-exec` does not
nest, so the replay has to come from an unsandboxed shell (the principal's, or the
owner's terminal). To test one compile alone, run the `cc` command of the failing
file under the profile with `-save-temps=obj` added or removed.

**Measuring.** The number that matters is *lane start → first targeted test
result*: the `lane_warm_start` event's `seed_ms`, `queued_ms` and
`first_build.duration_ms` give the warm-up's share (`timed_steps`), and the
worker's first `cargo test --lib <filter>` the rest. A warm lane is one whose
target holds `.kronn-warm.json`.

Measured on 2026-11-12 on the lane of KT-1736 itself (host load 15–25):

| Step | Duration |
|---|---|
| worktree creation / lane provision / seed clone | 2.8 s / 0.1 s / 0.5 s |
| first build (`cargo test --no-run`, before) | 1 800.0 s — the bound, `timed_out`, no marker |
| smoke check | 1.3 s |
| worker's own `cargo test --lib` build of the `kronn` lib test target, same seeded lane, afterwards | 22 min 19 s, then 8 s for 43 tests |

So on a seeded lane the first targeted result costs about 22 minutes of
compilation of the worktree's own crate, whichever process pays it. Before this
change that compile came *after* a warm-up that had either failed in 90 s
(`aws-lc-sys`, KT-1718: 41 min 43 s to the first test on a fully cold lane) or
spent its whole 30 minutes also building every integration binary. After it, the
warm-up is that same lib-test compile: the figure to expect is the ~22 minutes
above moved from the worker's turn into the warm-up, a worker whose first
`cargo test --lib <filter>` then takes seconds, and a `.kronn-warm.json` in the
target.

The principal confirmed the cause and the fix outside the worker sandbox:
`sandbox-exec` with a worker-like profile and `cargo build --offline --locked -p
aws-lc-sys` on a fresh target fails without `CFLAGS` (`clang: error: unable to
make temporary file: Operation not permitted`, exit 101) and succeeds with
`CFLAGS=-save-temps=obj` (exit 0, 20.81 s). The end-to-end measurement below is
a post-integration live check, run once the restarted Kronn carries this change.

**Not measured here:** a fresh lane's warm-up *under the Seatbelt profile* ending
`succeeded` with the marker written, and the resulting lane start → first test
result. A worker cannot run the profile (see above) and the lane of this task was
seeded before the change; the owner's Mac is where it is read: launch a task on a
project whose seed lacks a fresh `aws-lc-sys`, then read `lane_warm_start`
(`first_build.outcome`, `duration_ms`) and check the lane target for
`.kronn-warm.json`.

[src: file: backend/src/core/task_worker_env.rs:1141-1182#drain_output]
[src: file: backend/src/core/task_worker_env.rs:68-80#CLANG_KEEP_TEMPS_IN_OBJ_DIR]
[src: file: backend/src/core/task_worker_env.rs:148-154#compiler_temp_env_for]
[src: file: backend/src/core/task_worker_lane_warm.rs:729-911#warm_lane_cancellable]
[src: file: backend/src/core/task_worker_lane_warm.rs:925-932#first_build_args]
[src: file: backend/src/core/task_worker_lane_warm.rs:936-940#has_lib_target]
[src: file: backend/src/core/task_worker_sandbox.rs:318-369#seatbelt_profile_with]
[src: file: backend/src/api/orchestration.rs:10876-10907#warm_failure_notice]

## Long builds in a task worker (KT-1718)

A cold test-profile build of the backend (`cargo test --lib <filter>` compiles
the whole dependency graph and a 1.7 GB `libkronn.rlib`) took 41 min 43 s cold
on a loaded lane, against a 10-minute shell cap (table below). On 2026-11-12 a worker's filtered run hit the shell
tool's cap three times (each timeout killed `rustc`), started the build in the
background, handed the turn back, and the headless Claude process exited and
took the build with it: dispatch `worker_reported_failure`, execution
`Interrupted` (KT-1595, then KT-1692 and KT-1693). Three limits were stacked,
and the worker is told about all three:

| Limit | Value | Who owns it |
|---|---|---|
| Per shell command | Claude Code's own cap is 10 minutes (`BASH_MAX_TIMEOUT_MS`); Kronn now sets **60 min** | Settings > Workers, `workers.bash_max_timeout_min` (1 to 240) |
| Per shell command when it names no limit | Claude Code's own default is 2 minutes; Kronn now sets **15 min** | Settings > Workers, `workers.bash_default_timeout_min` (1 to the maximum) |
| Generation (the whole turn) | `server.agent_global_timeout_min`, 30 min by default, clamped to 1 to 240 | Settings > Server |

**Kronn sets the shell limits itself.** Every Claude task worker is launched
with `BASH_DEFAULT_TIMEOUT_MS` and `BASH_MAX_TIMEOUT_MS` taken from the two
settings, so a worker no longer depends on the shell Kronn was started from (the
variables used to be passed through only when present in Kronn's own
environment, which a normal install never has). The settings win over an
inherited value; they are published to the process by `config::save` and at boot,
so a saved change reaches the next spawn without a restart. A hand-edited value
is clamped at spawn and the default never exceeds the maximum. A command allowed
as long as the generation would be cut with it, so Settings > Workers shows the
per-command limits next to the generation limit and warns when the maximum
reaches it.

**The worker brief says how to run a long build.** Run it in the foreground and
pass the Bash tool an explicit `timeout` up to the cap; never cut it short and
never restart it in a loop; never end the turn while a command runs in the
background, because the process exits with the turn and takes the build with it
(a command that must be backgrounded is waited for in the same turn). The
persisted long validations stay Kronn's, run at integration.

**Lane warm-up.** Before the worker starts, the lane runs
`cargo test --lib --no-run --locked --offline` (the test profile, not only the
dev build; KT-1736: the lib test binary only, no `tests/*.rs` binary) and writes `.kronn-warm.json` only when it succeeds; a lane whose marker
matches the current `Cargo.lock` is not built again. A failed or timed-out
warm-up leaves no marker and the worker starts anyway, so a lane without
`.kronn-warm.json` is cold for the test profile whatever its seeded `debug/`
holds.

**Measured on the KT-1718 lane (2026-11-12, `CARGO_BUILD_JOBS=3`, background
priority, machine busy with other workers).** The lane had been seeded with a
`cargo build` target but carried no `.kronn-warm.json`, so its test profile was
cold. From the worker's first build command to the first targeted test result:

| Step | Wall time |
|---|---|
| Cold `cargo check --lib --tests` (the shell tool moved it to the background at its 600 s cap) | 10 min 08 s |
| Cold `cargo test --lib <filter>`: test-profile build, then the filtered run | 41 min 43 s build, 0.00 s run |
| Same lane, source untouched: `cargo test --lib <filter>` | 1.68 s build, 29.6 s for 50 tests |
| Same lane, after a source edit: `cargo test --no-run --locked --offline` (the warm-up command) | 1 min 07 s |
| Same lane, after merging the integration branch (about 80 files changed, backend and docs): rebuild of the test profile | 19 min 45 s |
| Same lane, immediately after: the citation gate test alone, binary already built | 25.6 s |

Before this change that first result was out of reach: the foreground cap (10
minutes) is a quarter of the cold build, so each attempt was killed, which is the
KT-1595 sequence. With a 60-minute cap the cold build completes inside one
foreground command (the measured run used a background command only because this
worker's own environment predates the setting), and on a warmed lane a filtered
`cargo test --lib` is well inside any cap. These are single observations under
load, not a performance guarantee.

[src: file: backend/src/models/setup.rs:131-145#bash_max_timeout_min]
[src: file: backend/src/models/setup.rs:204-211#effective_bash_timeouts_ms]
[src: file: backend/src/core/task_worker_env.rs:859-869#apply_bash_timeouts]
[src: file: backend/src/agents/runner.rs:15172-15174#apply_bash_timeouts]
[src: file: backend/src/core/config.rs:678-679#set_bash_timeouts]
[src: file: backend/src/core/child_env.rs:280-281#BASH_DEFAULT_TIMEOUT_MS]
[src: file: backend/src/api/orchestration.rs:11346-11356]
[src: file: backend/src/core/task_worker_lane_warm.rs:872]
[src: file: frontend/src/components/settings/WorkersSection.tsx:39#WorkersSection]

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

### Lane targets (`<repo>/target/wt-<lane>`) — KT-1256

A Claude task worker builds in a lane target outside its worktree, so removing
the worktree never removed it. Ownership is **not** the `wt-` name: humans and
other tooling build in `target/wt-0151-*` too. Kronn writes
`target/wt-<lane>/.kronn-lane.json` (lane, execution id, worktree) when it
provisions a lane, and only a lane carrying that marker is ever deleted.

| Lane | Outcome |
|---|---|
| No marker, malformed marker, marker whose lane differs from the directory | Reported as `unmarked`, never touched |
| Marked, execution unfinished / unknown, session or lease held | Kept (`claimed`) |
| Marked, execution finished, worktree present with uncommitted changes | Kept with the worktree |
| Marked, execution finished, worktree gone | Removed at boot |
| Marked, execution finished, worktree present and clean | Removed with the worktree, or by name through the reclaim route |

A running Cargo build (its lock) always refuses. The worktree is looked up under
the repository's own managed root from the marker's directory name, never at a
path the marker spells.

- `GET /api/maintenance/lane-targets?project_id=…` is the dry run: every
  `target/wt-*` with its state, reason, execution and measured size.
- `POST /api/maintenance/lane-targets/reclaim` takes `target_paths` and
  re-judges each; there is no "everything" verb.
- The boot report (`lane_target_report`, one line per lane target, plus the
  `lane_targets_unmarked` count) says what was removed and what was left.
- Each removal is audited on its execution as `lane_target_reclaimed` or
  `lane_target_refused`.

### The dry-run does not authorise

The dry-run is a read-only inventory. It **does not authorise** a deletion:
each target is re-judged against the durable execution state at the moment of
the `reclaim` call. A target that was `reclaimable` in the dry-run may be
refused at reclaim time if its execution is no longer terminal, and a target
that was refused in the dry-run is refused again at reclaim time. The durable
state is the only thing that may authorise a cleanup; the dry-run is only a
preview of it.
