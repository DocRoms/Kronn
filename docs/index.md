# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.12.0

- Projects, Discussions, Planning, Automation, Pages and Plugins share one
  accessible collection-sidebar interaction contract. [src: file: docs/conventions/collection-shell.md:1-15]
- Settings manages named LiteLLM, NVIDIA, OpenRouter and other OpenAI-compatible
  connections, including bounded connection tests, searchable model catalogues
  and model tiers. [src: file: docs/operations/external-api-connections.md:1-45]
- Project Audit is a direct detail section, Docs and Code expand through the
  available detail body, and selected Live Pages can open in an isolated
  mosaic. [src: file: frontend/src/components/ProjectCard.tsx:2619-2628]
  [src: file: frontend/src/pages/Dashboard.css:3131-3179]
  [src: file: docs/architecture/live-pages.md:44-56]
- Project details also expose Compose service state, lifecycle controls,
  published endpoints, host diagnostics and bounded recent logs in a dedicated
  Docker tab. [src: file: frontend/src/components/ProjectCard.tsx:2425-2431]
  [src: file: frontend/src/components/ProjectDockerPanel.tsx:300-345]
  [src: file: frontend/src/components/ProjectDockerPanel.tsx:349-410]
- Delegated tasks expose durable native progress, while full audits apply a
  deterministic documentation-optimization gate before validation. [src: file: docs/architecture/task-worker-progress.md:1-11]
  [src: file: docs/operations/documentary-optimization.md:1-8]
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.12.0, the
  [external API connection guide](operations/external-api-connections.md), the
  [task delegation guide](guides/task-orchestration.md), and [`releases/`](releases/)
  for older release history.

## In development: 0.13.0

- Automation variables can reference encrypted project values without copying
  secrets into templates. Launch previews are masked and audited, while each
  run resolves a fresh encrypted snapshot. See the
  [execution-variable architecture](architecture/execution-variables.md).

## Layout

- **`architecture/`** — High-level diagrams and component overviews.
- **`operations/`** — Runbooks, on-call notes, deploy procedures.
- **`screenshots/`** — Project-specific docs.
- **`tech-debt/`** — Known debts, planned removals, deprecation notes.
- **`releases/`** — Archived release notes kept out of the concise root
  changelog.

## Adding to the docs

- Drop a new markdown file into the matching subfolder; update this `index.md` if you create a new top-level folder.
- Cross-link with relative markdown links so the graph stays navigable in Obsidian / GitHub.
- Keep AI-loaded files (anything `AGENTS.md` references) free of secrets — Kronn enforces this on agent writes.
