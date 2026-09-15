# TD-20260504-stale-stream-watchdog

- **ID**: TD-20260504-stale-stream-watchdog
- **Area**: Frontend (Dashboard) / Backend (streaming)
- **Severity**: Medium — no data loss, but it made users pay twice
- **Status**: 🟢 Fixed 2026-09-15 (three faults); one deliberate limit remains

## The report

"Connexion perdue avec l'agent", repeatedly, followed by: *on crame du token
pour rien*. The user resent the message each time.

## The verdict, which reverses the premise

**The agent was not lost.** A reply runs in a spawned task the client cannot
stop, and it persists its answer either way:

- `backend/src/api/discussions/streaming.rs:2659` — *"Spawn background task —
  always saves to DB even if client disconnects"*
- `streaming.rs:721-724` — *"a dropped receiver does NOT cancel the agent — the
  message still gets persisted to DB"*
- `is_disconnected`: **zero occurrences in `backend/src`**. Nothing tests
  whether the client is still there.
- The only real cancellation is the registry (`CancelGuard::insert`,
  `streaming.rs:2655`), reached solely from `stop_agent` /
  `stop_agent_dispatch`.

So `abort()` on the client closes a socket the backend never reads. The run
finishes and writes its answer.

The cost was therefore never the abandoned run — Kronn's own retry is
idempotent (`runtime.rs:446-462`). It was **the resend the toast talked the
user into**, re-billing thousands of lines of context while the first reply
landed on its own.

## Three faults, stacked

**A — the abort did nothing.** `Dashboard.tsx` called `cleanupStream(discId)`
and then `abortControllers.current[discId]?.abort()`. `cleanupStream` deletes
that entry synchronously, so the abort read `undefined`. The correct order was
already used twice elsewhere (`DiscussionsPage.tsx:3237`, `:3261`); the watchdog
was the only site to have it backwards.

**B — two budgets, neither aware of the other.** The frontend gave up after 5
minutes; `NON_STREAMING_STALL_TIMEOUT` grants a silent agent **15**. Codex
`exec` writes nothing to stdout until the end, so every turn past five minutes
tripped the watchdog. Not a race — arithmetic.

**C — the decision ignored the one authority.** Staleness was judged from local
text deltas, so a run spending minutes in tool calls looked dead. Meanwhile
`getRunning()` — the server's own list — was already polled every 5 s in the
same component, kept apart from `sendingMap` on purpose so it would not trip
this very watchdog. The strong signal was isolated; the weak one decided alone.

## What shipped

- The abort fires before the reference is dropped, in `abortStaleStreams`, with
  a test asserting the observed order rather than the outcome.
- The threshold matches the backend ceiling, pinned by a test so the two cannot
  drift apart silently again.
- `detectStaleStreams` consults the server's running list first: silence is not
  death. It reads it through a ref — listing `runningDiscIds` as an effect
  dependency would rebuild the 30 s interval on every 5 s poll and the countdown
  would never finish.
- The toast says what is true: the stream was interrupted, the agent is still
  running server-side. Corrected in all four locales.

## Remaining limit, accepted

`noteStreamTick` still advances only on assistant **text**; tool and log events
do not feed it. Ticking on those was considered and not done: the server's
running list already answers the same question with authority, and a second
liveness source would have to agree with it. If `getRunning()` ever becomes
unavailable to the Dashboard, this limit returns and the tick should then be fed
from non-text events too.

Not addressed here: if a user genuinely wants to stop a run, `abort()` is the
wrong tool — `discussionsApi.stop(id)` / `stopDispatch` reach the cancel
registry. The watchdog does not attempt this, and should not: it fires on a
guess about a run it cannot see.
