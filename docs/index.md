# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.9.5

- New discussions containing several explicit agent aliases launch independent
  replies rather than silently entering debate mode. Collaboration remains an
  explicit, visible discussion policy, and generated alias prose cannot trigger
  a handoff without Kronn's internal delegation marker.
- Leading DeepSeek-style private-reasoning blocks returned through LiteLLM are
  filtered from the visible response while later literal tags remain intact.
- Open tabs recover after a backend restart or a half-open WebSocket: heartbeat
  detection, bounded reconnects and automatic discussion resynchronization are
  now one explicit reliability contract.
- Reconnecting discussions explain that drafts are preserved, while the global
  backend indicator retries quickly during an outage and disappears as soon as
  service recovers.
- Message sends are confirmed only by a durable persistence receipt. A failure
  before that point restores the draft and removes the optimistic transcript
  row instead of silently losing user input.
- See the concise [`CHANGELOG.md`](../CHANGELOG.md) for 0.9.5 and 0.9.4, and
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
