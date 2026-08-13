# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.9.7

- Session resume and shared-worktree ownership now fail closed instead of
  silently binding or replaying work in stale contexts.
- LiteLLM and Ollama workflow steps receive bounded, project-scoped native API,
  Quick API and read-only Planning tools with secret-free execution receipts.
- Plugin imports explicitly assign Global/project scope, while project cards
  expose Context Audit drift and honest human-attestation provenance.
- Desktop releases smoke-test document generation and require a complete
  Windows, macOS Intel, macOS ARM and Linux installer matrix.
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.9.7 and 0.9.6, and
  [`releases/`](releases/) for older release history.

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
