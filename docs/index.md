# Project documentation

This folder is the project's living knowledge base, shared by humans and AI agents alike.

## Entry points

- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.
- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.

## Current release: 0.14.1

- Workflows chain and scale: a `TriggerWorkflow` step launches another workflow
  with mapped variables, `concurrency_key` counts the limit per rendered key, and
  `workspace_config.base_ref` starts an isolated run from a fetched commit. See
  the [architecture overview](architecture/overview.md).
- A workflow Agent step with `room_id` is its room's principal without an invite
  token, at every launch and resume. See
  [kronn-internal](operations/mcp-servers/kronn-internal.md).
- `ApiCall` can return an image or another file (`api_response: Binary`) that an
  Artifact displays without opening its sandbox to the network. See
  [Live Pages](architecture/live-pages.md) and
  [de-agentified ApiCall](operations/deagent-apicall.md).
- Anthropic prompt caching through LiteLLM is on by default;
  `KRONN_LITELLM_PROMPT_CACHE=0` turns it off. See
  [token economy](operations/token-economy-0.9.6.md).
- The native-files sync removes only what it wrote (`.kronn/native-files.json`),
  and boot reclaims the clean worktree of an interrupted run after
  `server.interrupted_worktree_ttl_days`. See the
  [0.14.1 release notes](../CHANGELOG.md), including the dark-theme
  accessibility pass and its validation limits.

## Earlier releases

- HTTP agents can discover, launch and edit Quick Prompts, and author disabled
  workflow drafts. See [HTTP-agent capabilities](architecture/http-agent-capabilities.md)
  and the retained [native Quick Prompt campaign](research/native-qp-litellm-ollama-2026-09-22.md).
- Media generation can select the sole connection for a modality when none is
  supplied. The [seven-model check](research/media-without-connection-id-2026-09-22.md)
  records queued jobs, with no image-generation workers running.
- Text, JSON and log attachments open in the discussion carousel, with a bounded
  preview and the original download. See [UI structure](architecture/ui-structure.md).
- Empty optional workflow inputs remain in execution snapshots; required and
  undeclared variables retain validation. See
  [execution variables](architecture/execution-variables.md).
- Live Pages follow Kronn's theme and no longer reset local state when polled
  data is unchanged. See the [0.14.0 release notes](../CHANGELOG.md) for these
  changes, setup improvements and validation limits.

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
