# Task workers: processes outlive the turn, Vite cannot write its config bundle

## What a worker starts dies with its dispatch (KT-1266)

A worker's shell tool starts a backend or a Vite in a process group of its own,
so signalling the agent's group when the run ends left them listening on the
lane's ports (the sandbox denies the worker the signal to stop them).

Every launch of a worker carries `KRONN_TASK_DISPATCH_ID=<dispatch job id>`, and
so does everything it starts. When the dispatch ends (completed, failed,
cancelled) or the watchdog finds its run gone, the processes carrying that id
get SIGTERM, then SIGKILL, and the table is re-read for what was started
meanwhile. Only the ended dispatch's id is matched.
[src: file: backend/src/core/dispatch_reaper.rs:29-30#DISPATCH_ENV]
[src: file: backend/src/core/child_env.rs:941-953#DISPATCH_ENV]
[src: file: backend/src/api/discussions/runtime.rs:559-561#reap_task_worker_processes]
[src: file: backend/src/api/orchestration.rs:811-813#reap_task_worker_processes]
[src: file: backend/src/api/discussions/runtime.rs:626-642#reap_task_worker_processes]
[src: file: backend/src/core/dispatch_reaper.rs:638-693#reap_dispatch]

What is taken, and what is not:

- **Tagged processes are always taken.** A process that copies the variable
  into something it launches for the operator (a direnv `.envrc` that exports
  it, a tool that persists its environment) makes that thing the worker's: it is
  stopped with the dispatch.
- **An untagged descendant is taken only in the session of the taken process it
  descends from** (`getsid`). That still catches a server that cleared its
  environment. A daemon the worker started (tmux, `ollama serve`, sccache, a
  gradle daemon) carries the tag, so it is stopped, and so are its children
  that inherited its environment (tmux panes do). Only a child that is both
  untagged and in a session of its own survives.
- **Some processes escape the reap on macOS.** `KERN_PROCARGS2` shows no
  environment for platform or hardened binaries, nor for a process that
  rewrote its argument area. Such a process is found only as a same-session
  descendant; once its tagged parent exits it belongs to launchd and is missed.
- **A pid is never signalled on a stale snapshot.** The start stamp
  (`agent_orphans::start_stamp`) is compared at kill time, and the tag or the
  session is read again right before each signal (SIGTERM and SIGKILL).
  A process whose stamp could not be read is never a target.
[src: file: backend/src/core/dispatch_reaper.rs:147-187#run_processes]
[src: file: backend/src/core/dispatch_reaper.rs:476-512#macos_procargs]
[src: file: backend/src/core/dispatch_reaper.rs:583-616#is_still_ours]

- macOS hides the environment of platform binaries (`/bin/sh`, `/bin/sleep`,
  `/usr/bin/*`), as `ps eww` does: such a process is found only as a descendant
  of a visible tagged one. A server run by a node, python or cargo binary is
  visible. A test that needs a tagged process must not use a system binary.
- The table is read with libproc and `KERN_PROCARGS2` on macOS and `/proc` on
  Linux: no `ps`, which the worker sandbox refuses to execute.
  [src: file: backend/src/core/dispatch_reaper.rs:365-428#process_table]
  [src: file: backend/src/core/dispatch_reaper.rs:430-474#process_table]
- On Linux the kernel refuses `/proc/<pid>/environ` of a zombie and of a
  process of the same user that is not dumpable (`(sd-pam)`, `ssh-agent`).
  A zombie is skipped; any other such process makes the reap unverified only
  when it is the backend's child or descends from a run process.
  [src: file: backend/src/core/dispatch_reaper.rs:303-332#hidden_kin]
[src: file: backend/src/core/dispatch_reaper_test.rs:195-199#start_python_under_shell]
- Nothing sweeps at boot: a lane's own backend (started by a worker, with its
  own scratch database) would otherwise kill every worker of the host.
- A daemon that also calls `setsid` *and* clears its environment cannot be told
  from a foreign one; it is left running.

## Vite's config loader needs a writable `node_modules` (KT-1266)

Vite bundles `vite.config.ts` under `<nearest node_modules>/.vite-temp`, and
only handles `EACCES`: the sandbox answers `EPERM`, so `vite` and `vitest` died
on a `node_modules` that was a link to the main checkout's. The lane now makes
`frontend/node_modules` a real directory whose entries link to the main
checkout's (never `.vite-temp`, `.vite`, `.cache`), so the bundle lands in the
lane and no `--configLoader native` is needed. A whole-directory link from an
earlier launch is replaced; packages installed in the main checkout since are
linked at the next launch. An entry the lane already holds as a real directory
is left alone and does not stop the entries after it.
[src: file: backend/src/core/task_worker_env.rs:359-397#link_frontend_deps]
[src: file: backend/src/core/task_worker_env.rs:399-434#layer_frontend_deps]
[src: file: backend/src/core/task_worker_env.rs:437-455#link_entries]

The smoke check also loads the config (`vitest run --passWithNoTests <no
match>`), which `--version` never did. That executes the worker-authored
`vite.config.ts`, so it runs through the same `SandboxedProbeRunner` as the
lane's first build (KT-1255): the deny-default profile, never the backend's own
rights. If vitest cannot load its config under that profile the frontend reads
as unavailable, with the command's output as the reason.
[src: file: backend/src/core/task_worker_env.rs:985-1045#check_frontend]
[src: file: backend/src/api/orchestration.rs:2256-2276#worker_probe_runner]
