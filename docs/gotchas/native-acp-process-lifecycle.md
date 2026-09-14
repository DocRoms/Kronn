# Native ACP process lifecycle

`AcpJsonRpcTransport` owns two things with one lifetime: the native ACP child
process, and the task draining its stdout. `kill_on_drop` covers the child.
Nothing covered the task, and a bare `JoinHandle` DETACHES on drop rather than
cancelling — so `DispatcherOwner` holds it and aborts on drop, which is what
makes an abandoned transport or a cancelled shutdown stop the drain too.
[src: file: backend/src/acp.rs:465-523]
[src: file: backend/src/acp.rs:581]

The owner keeps that handle inside itself across every await. Taking it into a
local first defeats the guard exactly where it is needed: a `shutdown` cancelled
mid-await would drop the local — detaching — while `Drop` found nothing left to
abort.
[src: file: backend/src/acp.rs:482-514]

Reaping the child does NOT close its stdout when a descendant inherited the
pipe, so the join is bounded and then aborted, and the abort is awaited —
`abort` only requests cancellation. That cancellation is logged and not returned
as an error: the task IS stopped and nothing leaks, while
`acp_discovery::discover_with_transport` maps any shutdown error onto its
outcome, so failing there would turn a successful catalogue discovery into a
provider error. A PANIC is a different thing and is still reported, exactly like
the nominal join — the non-fatal policy covers the cancellation we asked for,
not every `JoinError`. The claim stops at the drain; a descendant holding the
inherited pipe is not Kronn's to terminate.
[src: file: backend/src/acp.rs:1178-1210]
[src: file: backend/src/acp.rs:458]

The Unix-only regression fixtures observe a real subprocess over a socket the
TEST owns, and read no PID at all. A python3 stdlib fixture connects, announces
`READY`, then selects on that socket and on stdin; EOF on the socket is its
autonomous exit, so a red run cleans up against the BROKEN implementation
instead of relying on the `kill_on_drop` the fix introduces. The fixture
deliberately survives stdin EOF the way a real agent does — otherwise dropping
the transport would end it for a reason unrelated to process ownership, and the
regression would pass without the fix.

A FIFO read through `tokio::fs` was tried first and rejected: it is delegated to
`spawn_blocking`, where a future timeout does not cancel the blocking call, so a
red run could pin a blocking-pool thread for the rest of the suite. The timeouts
that remain are anti-hang bounds, never waits — every one of them resolves on a
socket event.
[src: file: backend/src/acp.rs:2317-2458]

The in-process tests pin the ownership rules without a subprocess and without
timing: the drained task holds a `oneshot` sender, so the receiver resolving IS
the event that the task was dropped. The cancellation test polls the `shutdown`
future once before dropping it — an unpolled `async fn` has not run its body, so
dropping it would exercise nothing and pass for the wrong reason.
[src: file: backend/src/acp.rs:2211-2302]

Before the runner hands ownership to its turn task, every startup failure must
finish the ACP host — nine error returns, none of them propagating with `?`.
Model catalogue discovery follows the same rule for negotiation and
session-creation failures, and no longer discards its own cleanup error.
[src: file: backend/src/agents/runner.rs:3576-3858]
[src: file: backend/src/agents/runner.rs:3875-3880]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:51-92]
