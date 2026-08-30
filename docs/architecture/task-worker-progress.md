# Native task-worker progress

`TaskExecutionDetail.progress` is the canonical status projection for a native task worker. Its phases distinguish queueing, provider launch/wait, tool activity, delivery, and terminal outcomes. The projection also carries queue age, optional queue position, tri-state process liveness, the last reliable signal, and an explicit telemetry mode. [src: file: backend/src/models/orchestration.rs:1361-1403]

Queue state comes from the durable dispatch row. A claimed dispatch without `agent_started_at` remains `queued`; the post-semaphore boundary becomes `launching`, followed by `upstream_wait` immediately before the provider call. Queue position is omitted when scheduler admission constraints prevent a reliable rank. Missing continuous telemetry is represented as `unavailable` or `boundary_only`, never inferred as a stall. [src: file: backend/src/api/orchestration.rs:6294-6335]

Process liveness is attached by both execution-status surfaces only when the runtime registry has observed a successfully created provider runtime. Cancellation-token presence or absence is not treated as process evidence. [src: file: backend/src/api/orchestration.rs:6340-6368] [src: file: backend/src/api/orchestration.rs:8328-8368] [src: file: backend/src/lib.rs:204-208]

Native Claude stream tool boundaries update durable progress: tool start records `tool_activity`, and tool completion returns the phase to `upstream_wait`. [src: file: backend/src/api/discussions/streaming.rs:2415-2481]

The dispatcher persists provider launch and later progress boundaries in `agent_dispatch_jobs.last_progress_at`. [src: file: backend/src/db/agent_dispatch.rs:681-711]
