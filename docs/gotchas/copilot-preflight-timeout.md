# Copilot preflight timeout proof

KT-625 was found by the unfiltered KT-621 backend run, not by a live Copilot
request. The old test started a shell with a 100 ms deadline, then waited up to
one second for that shell to write its PID. Under load the deadline could kill
the child before its first instruction, so the PID file never appeared. The
observed failure was `the slow preflight must have started: Elapsed(())`, before
the termination assertion. Increasing the deadline or adding a retry would not
establish process ownership.

The regression now obtains the PID directly from the spawned `Child`. Its
stdin is held open so the fixture cannot finish normally; a paused Tokio clock
advances the deadline without relying on child startup timing. The assertion
requires the process to be absent immediately on return, with `ESRCH`, without
the former 50 ms sleep. A separate regression exercises the full invocation
path and pins its unchanged four-second production deadline.
[src: file: backend/src/agents/runner_test.rs:6022-6125]

The preflight keeps ownership of `Child` outside the timed output collection.
Timeout explicitly awaits `Child::kill`, which also waits/reaps in Tokio;
`kill_on_drop` remains the cancellation fallback. Stdout, stderr, exit status
and spawn errors are covered separately. The timer budget still includes
spawning; this does not alter account classification, authentication or worker
reassignment policy.
[src: file: backend/src/agents/runner.rs:8202-8295]
[Tokio Child lifecycle](https://docs.rs/tokio/latest/tokio/process/struct.Child.html#method.kill)

These are local shell fixtures, not a real agent or account check. The release
checklist records failed and successful runs separately; a failed library suite
does not imply the later integration suites ran.
