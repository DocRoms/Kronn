# Native ACP process lifecycle

`AcpJsonRpcTransport` owns two things with one lifetime: the native ACP child
process, and the task draining its stdout. `kill_on_drop` covers the child.
Nothing covered the task, and a bare `JoinHandle` DETACHES on drop rather than
cancelling — so `DispatcherOwner` holds it and aborts on drop, which is what
makes an abandoned transport or a cancelled shutdown stop the drain too.
[src: file: backend/src/acp.rs:695-758#DispatcherOwner]
[src: file: backend/src/acp.rs:760-768#AcpProcess]

The owner keeps that handle inside itself across every await. Taking it into a
local first defeats the guard exactly where it is needed: a `shutdown` cancelled
mid-await would drop the local — detaching — while `Drop` found nothing left to
abort.
[src: file: backend/src/acp.rs:707-749#finish]

Reaping the child does NOT close its stdout when a descendant inherited the
pipe, so the join is bounded and then aborted, and the abort is awaited —
`abort` only requests cancellation. That cancellation is logged and not returned
as an error: the task IS stopped and nothing leaks, while
`acp_discovery::discover_with_transport` maps any shutdown error onto its
outcome, so failing there would turn a successful catalogue discovery into a
provider error. A PANIC is a different thing and is still reported, exactly like
the nominal join — the non-fatal policy covers the cancellation we asked for,
not every `JoinError`. The claim stops at the drain: on Unix `shutdown` first
kills the process group the child leads, so only a descendant outside that group
can still hold the inherited pipe, and that one is not Kronn's to terminate.
[src: file: backend/src/acp.rs:707-749#finish]
[src: file: backend/src/acp.rs:1795-1829#shutdown]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:80-120#discover_with_transport]

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
[src: file: backend/src/acp.rs:4160-4177#FixtureLink]
[src: file: backend/src/acp.rs:4197-4232#command]
[src: file: backend/src/acp.rs:4179-4182#FIXTURE_GUARD]

The in-process tests pin the ownership rules without a subprocess and without
timing, including that a PANICKING drain is reported as an error rather than
folded into the non-fatal cancellation policy: the drained task holds a `oneshot` sender, so the receiver resolving IS
the event that the task was dropped. The cancellation test polls the `shutdown`
future once before dropping it — an unpolled `async fn` has not run its body, so
dropping it would exercise nothing and pass for the wrong reason.
[src: file: backend/src/acp.rs:4042-4059#dropping_the_dispatcher_owner_cancels_the_drain]
[src: file: backend/src/acp.rs:4105-4127#a_panicking_drain_is_reported_as_an_error]
[src: file: backend/src/acp.rs:4129-4158#cancelling_a_shutdown_in_flight_still_cancels_the_drain]

Before the runner hands ownership to its turn task, every startup failure must
finish the ACP host: each error return goes through `acp_start_failure`, none
propagates with `?`.
Model catalogue discovery follows the same rule for negotiation and
session-creation failures, and no longer discards its own cleanup error. A
scripted transport counts its shutdowns, so "did this path clean up?" is
asserted on all three outcomes rather than read off the source — and a failing
cleanup never replaces the failure that caused it.
[src: file: backend/src/agents/runner.rs:5829-5838#acp_start_failure]
[src: file: backend/src/agents/runner.rs:5328-5339#acp_start_failure]
[src: file: backend/src/agents/runner.rs:5545-5553#acp_start_failure]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:80-120#discover_with_transport]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:287-298#ScriptedTransport]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:382-422#discovery_shuts_the_transport_down_when_initialize_fails]
[src: file: backend/src/core/model_catalog/acp_discovery.rs:424-450#a_failing_cleanup_never_hides_the_failure_that_caused_it]
