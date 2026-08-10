# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.9.4

- LiteLLM joins Ollama and the CLI providers as a first-class agent. Both HTTP
  agents can call Kronn tools through native tool-calling frames.
- Vibe workspace-trust conflicts are detected when they would silently block
  a Kronn-managed MCP configuration.
- Settings now follow an Identity → Agents → Capabilities → Interface hierarchy,
  with the remaining controls grouped by project experience and system data.
- The UI ships four independent lazy-loaded locales: French, English, Spanish
  and Simplified Chinese. UI language remains separate from the default output
  language assigned to new discussions.

## Layout

- **`architecture/`** — High-level diagrams and component overviews.
- **`operations/`** — Runbooks, on-call notes, deploy procedures.
- **`screenshots/`** — Project-specific docs.
- **`tech-debt/`** — Known debts, planned removals, deprecation notes.

## Adding to the docs

- Drop a new markdown file into the matching subfolder; update this `index.md` if you create a new top-level folder.
- Cross-link with relative markdown links so the graph stays navigable in Obsidian / GitHub.
- Keep AI-loaded files (anything `AGENTS.md` references) free of secrets — Kronn enforces this on agent writes.
