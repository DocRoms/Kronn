# Task workers: processes outlive the turn, Vite cannot write its config bundle

## What a worker starts dies with its dispatch (KT-1266)

A worker's shell tool starts a backend or a Vite in a process group of its own,
so signalling the agent's group when the run ends left them listening on the
lane's ports (the sandbox denies the worker the signal to stop them).

Every launch of a worker carries `KRONN_TASK_DISPATCH_ID=<dispatch job id>`, and
so does everything it starts. When the dispatch ends (completed, failed,
cancelled) or the watchdog finds its run gone, the processes carrying that id
and their descendants get SIGTERM, then SIGKILL, and the table is re-read for
what was started meanwhile. Only the ended dispatch's id is matched.
[src: file: backend/src/core/dispatch_reaper.rs:18]
[src: file: backend/src/core/child_env.rs:941]
[src: file: backend/src/api/discussions/runtime.rs:553]
[src: file: backend/src/api/orchestration.rs:768]

- macOS hides the environment of platform binaries (`/bin/sh`, `/bin/sleep`,
  `/usr/bin/*`), as `ps eww` does: such a process is found only as a descendant
  of a visible tagged one. A server run by a node, python or cargo binary is
  visible. A test that needs a tagged process must not use a system binary.
- The table is read with libproc and `KERN_PROCARGS2` on macOS and `/proc` on
  Linux: no `ps`, which the worker sandbox refuses to execute.
- Nothing sweeps at boot: a lane's own backend (started by a worker, with its
  own scratch database) would otherwise kill every worker of the host.

## Vite's config loader needs a writable `node_modules` (KT-1266)

Vite bundles `vite.config.ts` under `<nearest node_modules>/.vite-temp`, and
only handles `EACCES`: the sandbox answers `EPERM`, so `vite` and `vitest` died
on a `node_modules` that was a link to the main checkout's. The lane now makes
`frontend/node_modules` a real directory whose entries link to the main
checkout's (never `.vite-temp`, `.vite`, `.cache`), so the bundle lands in the
lane and no `--configLoader native` is needed. A whole-directory link from an
earlier launch is replaced; packages installed in the main checkout since are
linked at the next launch. The smoke check now also loads the config
(`vitest run --passWithNoTests <no match>`), which `--version` never did.
[src: file: backend/src/core/task_worker_env.rs:397]
[src: file: backend/src/core/task_worker_env.rs:90]
