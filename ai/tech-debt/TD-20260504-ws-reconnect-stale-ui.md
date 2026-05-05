# TD-20260504-ws-reconnect-stale-ui

- **ID**: TD-20260504-ws-reconnect-stale-ui
- **Area**: Backend (WS handler) + Frontend (streaming state)
- **Severity**: Medium

## Problem (fact)

After a Docker / backend restart while a discussion has an active stream, the frontend remains stuck in "agent en cours" forever.

Two compounding causes:

### 1. Backend WS handshake too strict on reconnect

`backend/src/api/ws.rs` rejects the first WS frame if it's not `Presence`. After a backend restart, the frontend's reconnect logic sends `Ping` first (heartbeat resumed before the initial `Presence` is re-emitted), and the backend logs:
```
WARN kronn::api::ws: WS: first message must be Presence, got Ping { timestamp: ... }
```
…on every reconnect attempt (~every 30 s), permanently. The frontend never recovers a usable WS channel for that tab.

### 2. Frontend has no stale-streaming watchdog

The `isStreaming` flag in the discussion state is cleared **only** by an SSE/WS event. If the channel dies (lost network, suspend, backend restart, container OOM), the spinner spins forever. There is no:
- "no chunk for >N min" timeout that auto-clears the flag
- HTTP fallback that re-fetches the latest message and reconciles state on WS failure

## Reproduction (verified 2026-05-04, EW-7277 incident)

1. Launch a QP, agent enters Phase 2 streaming
2. Reply "vas-y", agent starts Phase 3 streaming
3. Suspend the laptop (Docker pauses)
4. Wait ≥30 min, resume the laptop
5. Docker resumes → backend restarts (`agent_detect sweep` log signature)
6. Frontend keeps showing the agent as "en cours" indefinitely
7. The agent message **was actually completed and persisted** before the restart (`messages.timestamp` ~ pre-restart, `partial_response IS NULL`) — the data is fine, the UI is wrong.

DB sanity check used for triage:
```sql
SELECT partial_response IS NOT NULL AS has_partial,
       partial_response_started_at,
       (SELECT COUNT(*) FROM workflow_runs WHERE status IN ('Running','Pending') AND id = discussions.workflow_run_id) AS run_active
FROM discussions WHERE id = '<disc_id>';
```
Both columns NULL/0 ⇒ stream finished, frontend is just wrong.

## Why we can't fix now (constraint)

Not a hard blocker — refresh (F5) recovers a clean UI in <1s. But the UX is "agent semble bloqué 2h" → support friction, false bug reports, fear of clicking "Stop" and losing real output.

## Impact

- UX: every laptop suspend / Docker restart while streaming = stuck UI, mandatory refresh
- Trust: user can't tell "agent stuck" from "UI stale" without DB introspection
- Telemetry: WARN log spam (`first message must be Presence, got Ping`) drowns real signal — every 30 s per orphaned tab

## Where (pointers)

- Backend: `backend/src/api/ws.rs` (handshake check that requires `WsMessage::Presence` as first frame).
- Frontend WS client: search for `WsMessage` send / `Presence` send → likely `frontend/src/lib/ws.ts` or similar.
- Frontend streaming state: discussion store / `useDiscussion` hook → look for `isStreaming` setter; that's the missing watchdog site.
- Discussion partial-response recovery already exists for the data side (`partial_response` + `PartialResponseRecovered` broadcast, see `index.md` §9). The stale-streaming watchdog is the **UI complement** — bypass the WS dependency when the channel itself is broken.

## Suggested direction (non-binding)

**Backend fix (quick win):**
- Either accept `Ping` as a valid first frame post-handshake (heartbeat is benign), and send a `RequestPresence` reply if not yet associated.
- Or send an explicit `Error("expected Presence")` so the frontend's WS client knows to send `Presence` instead of retrying naked Pings.

**Frontend fix (defensive):**
- Add a `streamingWatchdog`: a per-discussion timer reset on every chunk; on timeout (5 min), clear `isStreaming` + trigger an HTTP refetch of the latest messages. Show a soft toast: *"Reconnexion perdue — voici l'état le plus récent"*.
- On WS reconnect failure (3 retries), force-refetch all open discussions' messages via REST and reconcile.

Both are independent — ship the cheaper one first if needed.

## Next step

Create ticket. Linked to the broader resilience work (Tauri sleep/wake, mobile network changes, prolonged Docker pauses).
