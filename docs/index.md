# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.13.2

- Automation variables can reference encrypted project values without copying
  secrets into templates. Launch previews are masked and audited, while each
  run resolves a fresh encrypted snapshot. See the
  [execution-variable architecture](architecture/execution-variables.md).
- Settings includes a local-icon Ko-fi link to support Kronn; the GitHub funding
  declaration is present in the repository, while GitHub-side button activation
  remains dependent on the default branch and Sponsorships configuration.
  [src: file: frontend/src/pages/SettingsPage.tsx:78-108]
  [src: file: .github/FUNDING.yml:1]
- A configured external API connection is an agent everywhere, not only in the
  router: mentionable, coloured, listed in the worker catalogue with the media
  it can generate, and delegable. Seven symptoms, one cause — see
  [external API connections](operations/external-api-connections.md) and the
  [static-table gotcha](gotchas/known-agents-static-tables.md).
- HTTP agents can generate images and videos. The tools existed for CLI agents
  through the MCP bridge, which an HTTP agent structurally cannot reach. See
  [HTTP-agent capabilities](architecture/http-agent-capabilities.md).
- A refused agent start says what it refused on, in both code paths that settle
  one. See [discussion agent routing](architecture/discussion-agent-routing.md).
- Multi-agent rooms survive a backend restart: a sibling's reply to the shared
  trigger no longer supersedes the siblings still waiting to run.
  [src: file: backend/src/db/agent_dispatch.rs]
- A human can decline an arbitration instead of answering it — a settled
  outcome that unblocks the lot without inventing a decision. See
  [`kronn-internal`](operations/mcp-servers/kronn-internal.md).
- HTTP agents author and run saved automations — listing Quick Execs, running
  one, and writing Quick APIs and Quick Execs — and read what each media model
  advertises instead of discovering its limits from a refusal. See
  [HTTP-agent capabilities](architecture/http-agent-capabilities.md) and
  [media generation](architecture/media-generation.md).
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.13.0 and
  [`releases/`](releases/) for older release history, including the 0.12.0
  collection-sidebar contract, named external connections and the project
  Docker tab.

## Layout

- **`architecture/`** — High-level diagrams and component overviews.
- **`operations/`** — Runbooks, on-call notes, deploy procedures.
- **`screenshots/`** — Project-specific docs.
- **`gotchas/`** — Traps that cost real debugging time, written so the next
  encounter is cheap. One per trap, named after the trap.
- **`tech-debt/`** — Known debts, planned removals, deprecation notes.
- **`releases/`** — Archived release notes kept out of the concise root
  changelog.

## Adding to the docs

- Drop a new markdown file into the matching subfolder; update this `index.md` if you create a new top-level folder.
- Cross-link with relative markdown links so the graph stays navigable in Obsidian / GitHub.
- Keep AI-loaded files (anything `AGENTS.md` references) free of secrets — Kronn enforces this on agent writes.
