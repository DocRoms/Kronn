# TD-20260715-ws-dead-after-restart-silent-send

- **ID**: TD-20260715-ws-dead-after-restart-silent-send
- **Status**: Resolved in 0.9.5 (2026-08-11)
- **Area**: Frontend / realtime (WebSocket lifecycle)
- **Problem (fact)**: After a backend restart, an already-open UI tab keeps a
  dead WebSocket and **a message posted from it never reaches the backend, with
  zero user feedback**. Incident #1 (2026-07-15): backend restarted at 13:29;
  the user re-posted in disc `38a0059b` from a pre-restart tab → the message was
  silently dropped (disc `message_count` stayed at 13, no backend log line), the
  UI showed no error, and the user concluded the agent was broken. An F5 was
  required to recover.
- **Original constraint**: surfaced mid-incident under a repo freeze; required
  a proper reconnect + send-acknowledgement design rather than a hotfix.
- **Impact**: UX/trust (user input silently lost; indistinguishable from the
  backend bug it accompanied) · support cost (a dead tab mimics a dead server).
- **Where (pointers)**:
  - `frontend/src/hooks/useWebSocket.ts` — client socket lifecycle (reconnect
    behaviour after server-side close).
  - `frontend/src/components/BackendStatus.tsx` — existing health surface a
    "connection lost" state could hook into.
- **Resolution**:
  - The WebSocket client reconnects with bounded exponential backoff, detects
    half-open connections through a pong deadline and ignores callbacks from
    stale sockets or unmounted components.
  - Discussions expose a reconnecting state and reload their list, active room
    and presence snapshot after every connection recovery.
  - The message stream emits `accepted` only after persistence. A failure before
    that receipt removes the optimistic row, restores the exact draft and
    surfaces the transport error.
  - The global backend status retries quickly while offline and rechecks on
    browser online/visibility events.
  - Hook, page, component and browser tests cover heartbeat failure, stale
    lifecycle callbacks, resynchronization, outage recovery and pre-receipt
    send failure.
- **Next step**: None — keep the regression tests as the release gate.
