# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.9.6

- Every discussion agent can use the shared plan: explicit room ids for CLIs,
  native planning tools for Ollama and LiteLLM, and human-gated proposals for Vibe.
- Previously unbounded context paths are capped and disclose truncation. Quick
  Exec handles deterministic checks without spending an agent turn.
- Discussion cost distinguishes Kronn replies from whole-session CLI telemetry;
  an unavailable measurement is shown as unknown, never as zero.
- macOS desktop packages own and start their embedded backend, expose actionable
  startup failures and tolerate missing optional Docs resources. Windows sidecars
  use the CPython-compatible UCRT runtime and are smoke-tested before Tauri builds.
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.9.6 and 0.9.5, and
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
