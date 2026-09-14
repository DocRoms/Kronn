# Native ACP process lifecycle

`AcpJsonRpcTransport` owns both the native ACP child process and the task that
drains its stdout. Process abandonment is protected with `kill_on_drop`; an
explicit shutdown serializes process termination, reaping, and dispatcher join
so repeated calls are safe and cleanup errors remain visible.
[src: file: backend/src/acp.rs:444-460]
[src: file: backend/src/acp.rs:509-512]
[src: file: backend/src/acp.rs:1109-1137]

The Unix-only regression fixtures use a subprocess they own, publish its exact
PID, and install a failure-path guard that can only signal that PID. They cover
rejected negotiation abandonment and repeated shutdown/reaping.
[src: file: backend/src/acp.rs:2132-2254]

Before the runner hands ownership to its turn task, every startup failure must
finish the ACP host. Model catalogue discovery follows the same rule for
negotiation and session-creation failures.
[src: file: backend/src/agents/runner.rs:3599-3732]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:52-90]
