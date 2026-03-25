# MCP context — draw.io

**Server:** `drawio-mcp` (npx)
**Source:** [jgraph/drawio-mcp](https://github.com/jgraph/drawio-mcp) — official, maintained by the draw.io team (jgraph)
**Auth:** None required

## What it does

Creates and edits draw.io diagrams programmatically: flowcharts, UML, architecture diagrams, sequence diagrams, ERDs, network diagrams. Outputs `.drawio` XML files.

## Project rules

- Output diagrams to `docs/diagrams/` (create directory if needed).
- Use `.drawio` extension (not `.xml`).
- Name files descriptively: `architecture-overview.drawio`, `auth-flow.drawio`.
- When updating an existing diagram, read it first to preserve layout and styling.

## Common use cases in Kronn

- Architecture diagrams for audited projects (`ai/` context enrichment).
- Workflow visualization (multi-step pipelines).
- Data flow diagrams for discussions.
