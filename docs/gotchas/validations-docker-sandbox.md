# Integration validations that need Docker

## The lane sandbox keeps Docker closed (KT-1433)

Before integrating an approved delivery, Kronn runs the plan's validations
(`make iso-deps`, `cargo test`…) in the worker's lane sandbox: writes only in
the lane, no outbound connection, Unix sockets included. A validation that goes
through Docker (OrbStack, Docker Desktop, Colima) therefore fails with
`permission denied while trying to connect to the docker API`, even with
Docker running. `failed to connect … no such file or directory` means Docker
itself is stopped.

## What Kronn does

- A validation that fails because of its environment (Docker unreachable or
  closed, a launch the sandbox refused) is not sent back to the worker: the
  execution is held with a card naming the fix, and its approval stays.
  [src: file: backend/src/api/orchestration.rs:8093-8153#environment_cause]
- When the sandbox kept Docker closed, Kronn also posts an elevation card:
  refuse, allow for this discussion, or allow for the project. An answer that
  allows records the grant and resumes the integration on its own. Only a
  message Kronn posted counts, and no agent token can answer it.
  [src: file: backend/src/db/orchestration.rs:4344-4399#record_docker_elevation]
- A project's validations reach is a human setting, in the project's settings:
  sandbox only (default), Docker allowed (the sandbox plus the resolved Docker
  socket, nothing else), or full access (no sandbox, behind a warning).
  [src: file: backend/src/models/projects.rs:166-174#ValidationsAccess]
- The reach changes the sandbox, never where a validation builds (KT-2024):
  at every reach the command gets the environment of its lane (its build and
  data directories, ports and build budget), whatever target directory the
  Kronn process itself was started with. What exists only because the sandbox
  refuses something (the offline switch, caches moved out of the home) is
  left out of a validation that runs without it.
  [src: file: backend/src/api/orchestration.rs:8432-8455#launch_for_reach]
- The socket comes from `DOCKER_HOST` (Unix only), then `docker context
  inspect`, then the known OrbStack, Docker Desktop, Colima and `/var/run`
  paths. A TCP endpoint is refused.
  [src: file: backend/src/core/task_worker_sandbox.rs:611-627#host_docker_socket]

The Docker socket carries the host's rights: allow it only for a project whose
validations you trust.

## macOS: no loopback connection, not even to a server the validation started (KT-2036)

On macOS a validation cannot connect to `127.0.0.1`, including to a server it
started itself (a wiremock, a supertest app, a Python stub). The Seatbelt
profile denies every outbound connection, loopback included; it only lets a
loopback port be bound and accepted on.
[src: file: backend/src/core/task_worker_sandbox.rs:284-304#seatbelt_profile]
The reason: the Kronn API, Ollama and a local database all listen on loopback,
and Seatbelt cannot limit loopback to the ports the validation bound itself.
The boundary stays (owner decision, 2026-11-23). Inside the profile a Python
`urllib` call to a loopback server fails with `[Errno 1] Operation not
permitted`.

A test that talks to a local server therefore passes in the worker's run, on
Linux and in CI, then fails at integration on macOS. Such a test belongs in CI
or in the worker's run, not in a macOS validation gate. On Linux, bubblewrap
gives the sandbox its own network namespace (`--unshare-net`), with a private
loopback, so the same test passes there.
[src: file: backend/src/core/task_worker_sandbox.rs:382-424#bubblewrap_args_hiding]

Kronn's own autoCode dry runs, which call a loopback stub, probe a loopback
connect once and skip with an explicit message where it is refused.
[src: file: backend/src/workflows/autocode_issue_template_test.rs:447-466#dry_run_skipped]
