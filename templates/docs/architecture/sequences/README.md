# Sequence diagrams

One Mermaid `sequenceDiagram` per critical flow. Per-flow file isolation
keeps `docs/AGENTS.md` Tier 1 small — agents load a sequence only when
working on the related flow.

## Conventions

- One file per flow. Name them by the user-facing action :
  - `auth-login.md` — user authenticates and gets a session token
  - `request-lifecycle.md` — typical request path (client → API → DB → response)
  - `deploy-pipeline.md` — code commit → CI → staging → prod
  - `payment-checkout.md` — checkout flow if e-commerce
- Hard cap: **3 files maximum**. Quality > quantity — only diagram what's
  load-bearing for understanding the system.
- Each file: 2-3 sentence intro describing the scope + entry/exit conditions,
  then the Mermaid `sequenceDiagram` block.

## Template

See [TEMPLATE.md](TEMPLATE.md) for the per-file shape. Copy + rename when
adding a new flow.

## Updating

Sequence diagrams stale fast. When you change an endpoint signature, an
auth mechanism, or a queue topology, **also update the matching sequence
file** in the same PR. The audit pipeline will flag drift between the
diagram and the code on the next pass.
