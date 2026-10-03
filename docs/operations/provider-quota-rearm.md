# Provider quota re-arm

Kronn blocks new delegations for a provider after a real quota-exhaustion recovery marker on an escalated execution. Escalations attached to Planning tasks in `done` or `archived` no longer count as active blockers. [src: file: backend/src/db/orchestration.rs:1857-1960]

A human confirmation in Config > Agents can acknowledge that one provider's quota is available again. The action records an idempotent provider-scoped audit event and watermark; it does not restart, cancel, or change any historical execution or recovery row. A later quota marker for that provider is newer than the watermark and blocks launches again. [src: file: backend/src/db/orchestration.rs:2032-2070]

The re-arm endpoint is a browser route and is not part of the task-worker MCP surface. [src: file: backend/src/lib.rs:1994-2003]

## Reassigning to the same provider is a re-arm (KT-838)

One exhausted quota often stops several executions at once, each escalation taking the next quota generation of that provider. Reassigning an `Escalated` execution to the very provider whose quota stopped it (its recovery marker is `quota_exhausted:<provider>`) is the human's "the quota is back" signal for the whole outage: the reassignment re-arms the provider in its own transaction, so every earlier generation stops blocking, whichever execution is reassigned first. The audit event has `actor_kind = human` and `actor_id = reassign:<execution id>`. Reassigning to a different provider says nothing about the first one and re-arms nothing. A new real quota failure takes a newer generation and blocks the provider again. [src: file: backend/src/api/orchestration.rs:8742-8795]

## Announced reset time

When the provider's refusal names a reset time in a zone (Claude Code: `resets 4:20pm (Europe/Paris)`, or with a day: `resets Oct 3, 4pm (Europe/Paris)`), it is resolved to a UTC instant and stored on the provider's quota generation row (`provider_quota_generations.reset_at`). Settings > Agents shows it beside the re-arm button as "Rearmable at 16:20" in the browser's local time. It is display only: nothing re-arms on it (KT-593). A refusal without a zone gets no instant, since the backend's own zone may not be the user's. A later refusal that names no time keeps a still-future announcement of the same outage and drops a passed one. [src: file: backend/src/api/discussions/orchestration.rs:1540-1591]
