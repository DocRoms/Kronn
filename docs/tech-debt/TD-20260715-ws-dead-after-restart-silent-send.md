# TD-20260715-ws-dead-after-restart-silent-send

- **ID**: TD-20260715-ws-dead-after-restart-silent-send
- **Area**: Frontend / realtime (WebSocket lifecycle)
- **Problem (fact)**: After a backend restart, an already-open UI tab keeps a
  dead WebSocket and **a message posted from it never reaches the backend, with
  zero user feedback**. Incident #1 (2026-07-15): backend restarted at 13:29;
  the user re-posted in disc `38a0059b` from a pre-restart tab → the message was
  silently dropped (disc `message_count` stayed at 13, no backend log line), the
  UI showed no error, and the user concluded the agent was broken. An F5 was
  required to recover.
- **Why we can't fix now (constraint)**: surfaced mid-incident under a repo
  freeze; needs a proper reconnect + send-acknowledgement design, not a
  hotfix.
- **Impact**: UX/trust (user input silently lost; indistinguishable from the
  backend bug it accompanied) · support cost (a dead tab mimics a dead server).
- **Where (pointers)**:
  - `frontend/src/hooks/useWebSocket.ts` — client socket lifecycle (reconnect
    behaviour after server-side close).
  - `frontend/src/components/BackendStatus.tsx` — existing health surface a
    "connection lost" state could hook into.
- **Suggested direction (non-binding)**:
  - Auto-reconnect with backoff on socket close/error; visibly degrade (banner
    "connexion perdue — reconnexion…") instead of staying silently dead.
  - Never silently drop a send: if the transport is down, queue-and-retry or
    fail loudly next to the input.
  - An end-to-end ack (message accepted = it exists in DB) so "posted" in the
    UI always means "persisted".
- **Next step**: create ticket.
