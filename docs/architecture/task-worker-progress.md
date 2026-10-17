# Native task-worker progress

`TaskExecutionDetail.progress` is the canonical status projection for a native task worker. Its phases distinguish queueing, provider launch/wait, tool activity, delivery, and terminal outcomes. The projection also carries queue age, optional queue position, tri-state process liveness, the last reliable signal, and an explicit telemetry mode. [src: file: backend/src/models/orchestration.rs:1578-1613#TaskExecutionProgressPhase]

Queue state comes from the durable dispatch row (a lane's first build, before any dispatch exists, reports `warming`). A claimed dispatch without `agent_started_at` remains `queued`; the post-semaphore boundary becomes `launching`, followed by `upstream_wait` immediately before the provider call. Queue position is currently always omitted: scheduler admission constraints prevent a reliable rank. Missing continuous telemetry is represented as `unavailable` or `boundary_only`, never inferred as a stall. [src: file: backend/src/api/orchestration.rs:9962-10062#execution_progress] [src: file: backend/src/api/discussions/streaming.rs:4325-4356#mark_agent_started] [src: file: backend/src/api/discussions/streaming.rs:4416-4433#mark_progress]

Process liveness is attached by both execution-status surfaces only when the runtime registry has observed a successfully created provider runtime. Cancellation-token presence or absence is not treated as process evidence. [src: file: backend/src/api/orchestration.rs:10064-10073#attach_runtime_liveness] [src: file: backend/src/api/orchestration.rs:10075-10094#get_execution_detail] [src: file: backend/src/api/orchestration.rs:13427-13437#attach_runtime_liveness] [src: file: backend/src/lib.rs:488-491#agent_runtime_registry]

Native Claude stream tool boundaries update durable progress: tool start records `tool_activity`, and tool completion returns the phase to `upstream_wait`. [src: file: backend/src/api/discussions/streaming.rs:4826-4911#mark_progress]

The dispatcher persists provider launch and later progress boundaries in `agent_dispatch_jobs.last_progress_at`. [src: file: backend/src/db/agent_dispatch.rs:838-891#mark_agent_started]
