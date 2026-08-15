# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.10.0

- Live Pages are sandboxed, versioned HTML reports backed by persisted named
  JSON datasets and linked to workflows or discussions.
- `CollectApiData`, `TransformData` and `PublishPageData` form a deterministic,
  zero-token path from saved Quick APIs or reusable shell-free Quick Execs (including CSV normalization) to an automatically refreshed
  Page.
- The Pages library supports search, favorites, multi-selection, archive and
  explicit deletion using the same interaction model as Discussions.
- Workflow and MCP authoring expose real data previews, visual mapping and Page
  creation for standalone, mock-backed or scheduled reports.
- Continual Learning is a default-off beta: typed agent proposals remain gated
  by evidence checks and explicit human validation before entering user or
  project learning documents.
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.10.0 and 0.9.7, and
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
