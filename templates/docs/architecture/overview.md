# Architecture

> **TEMPLATE FILE.** If sections below contain `{{...}}`, say `NOT_FOUND` — never invent services, ports, or patterns.

> Folder structure: [repo-map](../repo-map.md).

## Services

<!-- Fill after audit: list all services/apps with port, tech, role -->
{{SERVICES}}

## Key patterns

<!-- Fill after audit: architectural patterns used (MVC, hexagonal, event-driven, etc.) -->
{{PATTERNS}}

## Code organisation

<!-- Fill after audit: 1-2 sentences on how the codebase is laid out (by feature, by layer, by service…) -->
{{SEPARATION_DESCRIPTION}}

## Architecture diagram

<!--
  Mermaid `flowchart` rendering the services above + main data flow + external systems.
  Renders natively in GitHub, GitLab, Obsidian, VS Code preview — no extra tooling.
  For multi-tier projects, simulate C4 layers via named `subgraph` blocks
  (`subgraph Person`, `subgraph System`, …). The audit prompt expects this section
  to ship with a complete diagram, never a `{{...}}` placeholder.
-->
{{ARCHITECTURE_MERMAID}}

## Data flow

<!-- Fill after audit: 2-3 sentences explaining how data moves through the system -->
{{DATA_FLOW_DESCRIPTION}}

## Sequence diagrams

<!--
  Up to **3** sequence diagrams covering the most critical flows. Each lives in its
  own file under `sequences/` so it stays Tier 3 in `docs/AGENTS.md` — agents load
  them on demand only when working on the related flow, keeping per-turn token cost low.
-->

See [`sequences/`](sequences/) for per-flow Mermaid `sequenceDiagram` files.

## Legacy / planned migrations

<!-- Fill after audit: areas being migrated or deprecated -->
{{MIGRATIONS}}
