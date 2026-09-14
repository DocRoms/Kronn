# Native ACP process lifecycle

`AcpJsonRpcTransport` owns both the native ACP child process and the task that
drains its stdout. Process abandonment is protected with `kill_on_drop`; an
explicit shutdown serializes process termination, reaping, and dispatcher join
so repeated calls are safe and cleanup errors remain visible.

Reaping the child does NOT close its stdout when a descendant inherited the
pipe, so the dispatcher join is bounded and then aborted — dropping a
`JoinHandle` detaches without cancelling. That abort is logged, not returned as
an error: the child is reaped and the task cancelled, so nothing leaks, and
failing there would turn a successful model-catalogue discovery into a provider
error.
[src: file: backend/src/acp.rs:458-466]
[src: file: backend/src/acp.rs:513-517]
[src: file: backend/src/acp.rs:1113-1157]

The Unix-only regression fixtures use a subprocess they own, publish its exact
PID, and install a failure-path guard that can only signal that PID. They cover
rejected negotiation abandonment and repeated shutdown/reaping.
[src: file: backend/src/acp.rs:2154-2307]

Before the runner hands ownership to its turn task, every startup failure must
finish the ACP host. Model catalogue discovery follows the same rule for
negotiation and session-creation failures.
[src: file: backend/src/agents/runner.rs:3599-3745]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:52-90]
