# Provider quota re-arm

Kronn blocks new delegations for a provider after a real quota-exhaustion recovery marker on an escalated execution. Escalations attached to Planning tasks in `done` or `archived` no longer count as active blockers. [src: file: backend/src/db/orchestration.rs:1884-1900]

A human confirmation in Config > Agents can acknowledge that one provider's quota is available again. The action records an idempotent provider-scoped audit event and watermark; it does not restart, cancel, or change any historical execution or recovery row. A later quota marker for that provider is newer than the watermark and blocks launches again. [src: file: backend/src/db/orchestration.rs:1903-1925]

The re-arm endpoint is a browser route and is not part of the task-worker MCP surface. [src: file: backend/src/lib.rs:1853-1875]
