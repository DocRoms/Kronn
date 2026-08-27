# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.11.0

- Planning tasks can run through a durable worker, child-discussion, isolated
  worktree, parent-review and guarded-integration lifecycle.
- Quick Prompt compare records model, time, tokens, human quality and blind AI
  quality, then opens a contextual reasoning-agent discussion to improve the prompt.
- Ollama, LiteLLM and NVIDIA HTTP agents use Kronn's native tools as local or
  lower-cost workers alongside CLI agents.
- Exact-session room routing, restart reconciliation and bounded tool-loop
  convergence keep multi-agent work observable instead of silently stalling.
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.11.0, the
  [task delegation guide](guides/task-orchestration.md), and
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
