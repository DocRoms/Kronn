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
  [src: file: backend/src/api/orchestration.rs:4947-4985#environment_cause]
- When the sandbox kept Docker closed, Kronn also posts an elevation card:
  refuse, allow for this discussion, or allow for the project. An answer that
  allows records the grant and resumes the integration on its own. Only a
  message Kronn posted counts, and no agent token can answer it.
  [src: file: backend/src/db/orchestration.rs:3005-3060#record_docker_elevation]
- A project's validations reach is a human setting, in the project's settings:
  sandbox only (default), Docker allowed (the sandbox plus the resolved Docker
  socket, nothing else), or full access (no sandbox, behind a warning).
  [src: file: backend/src/models/projects.rs:30-38#ValidationsAccess]
- The socket comes from `DOCKER_HOST` (Unix only), then `docker context
  inspect`, then the known OrbStack, Docker Desktop, Colima and `/var/run`
  paths. A TCP endpoint is refused.
  [src: file: backend/src/core/task_worker_sandbox.rs:482-498#host_docker_socket]

The Docker socket carries the host's rights: allow it only for a project whose
validations you trust.
