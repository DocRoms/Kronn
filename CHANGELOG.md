# Changelog

All notable changes to Kronn are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release notes for 0.9.3 and earlier are available in the
[legacy archive](docs/releases/CHANGELOG-legacy.md).

---

## [Unreleased]

## [0.9.6] - 2026-08-12

Kronn's own token cost, measured and then bounded. Every figure below is a byte
count from this machine; token counts are estimates and gate nothing.

Full release documentation, including what each measurement does NOT prove and the
rollback matrix: `docs/operations/token-economy-0.9.6.md`.

### Added

- Workflow step details now open as a compact inspector with a familiar
  Preview/Edit switch. Preview remains the default; Edit embeds the canonical
  step editor in place, keeps long forms scrollable, saves without walking the
  whole wizard, and preserves Cancel as a no-write action. A selected late step
  in a long workflow opens directly without rendering every sibling editor.
- `BatchQuickPrompt` receipts expose each child's queued, claimed,
  agent-started and settled timestamps, plus monotonic active time, calendar
  wall time and estimated machine suspension. The 10/15-minute objectives are
  explicit; a 20-minute active-time overrun fails with
  `LATENCY_BUDGET_EXCEEDED` and transactionally cancels/settles every child so
  no orphan agent can continue spending tokens.
- Planning dependencies can now be removed explicitly from the Planning UI,
  the REST API and the `kronn-internal` MCP through the narrow,
  retry-safe `task_remove_blocker` operation. It removes only the selected
  dependency edge, preserves task status and blocked reason, and records the
  acting agent when a relation actually changes.
- **Quick Exec** — runs a deterministic command (tests, typecheck, lint, PR and CI
  collection) and returns a bounded summary instead of a full log, with the streams
  kept as an artifact on disk. No shell, ever: an allowlist of binaries by bare
  name, a denylist consulted first so a shell added later is still refused, a
  literal argv, a cwd that must canonicalise inside a declared root, and an explicit
  stdin. `Passed` means exit 0 and nothing else — a timeout, a cancellation, a
  signal death and a binary that never spawned are four distinct states and none is
  a pass. A truncated stream says so on the summary's first line.
- **Review ledger** — a finding is keyed to a CAUSE, not to a comment, so five
  comments about one unwrapped error are one finding with five symptoms. The ledger
  is pinned to a head SHA, so a re-review replays only what the diff touched. A
  re-review of the two reference discussions costs 564 B per pass at the measured
  p90, against 40.9 MB for a cold pass over the whole thread.
- **Deterministic PR collection** — six templates fetch metadata, changed files,
  both comment streams, checks and reactions with no agent pass, leaving the payload
  in an artifact rather than in a context.
- **Context Architecture Audit** (`GET /api/projects/{id}/context-audit`) — what
  each of nine agent conventions actually loads in any monitored project, with
  sizes, duplication, dead references and redirect cycles. It proposes a tier split
  and never writes: a Critical section is never proposed for a move, however large.
- **RTK adoption state** (`GET /api/rtk/state`) — folds five RTK commands into one
  bounded panel. A source that cannot answer says why AND what to do; two currently
  cannot on this machine, and both are named rather than shown as empty.
- **Discussion token cost in the header** — two figures, never a total. A Kronn
  agent reports a cost per reply; a joined CLI reports a running total for its whole
  session, which also covers files read, tests run and work in other rooms. Adding
  them would double-count and misattribute.
- **CLI telemetry** — a joined CLI reports its own vendor counters. On one real
  session 4 308 007 075 tokens of traffic had been stored as `0`; unmeasured is now
  `NULL` and renders as "unknown", never as free.
- **Benchmarks and gates** — review-pass input (cold and warm), RTK compression
  floors with residual ranking, the instruction-file ratchet, and the MCP surface
  ratchet.
- **Planning parity for discussion agents** — CLI agents receive the exact
  discussion id on their first turn and can use it even when an MCP runtime
  binding is stale. Ollama and LiteLLM expose compact native Planning reads and
  writes executed inside Kronn, scoped to the current room and attributed to the
  triggering message. Vibe emits the existing human-gated proposal fence because
  its runtime deliberately runs without MCP.

### Changed

- `docs/AGENTS.md` went from 84 224 B to 13 471 B behind a ratchet that tightens on
  every gain and refuses growth.
- The manual "linked CLI session" form is gone. The binding is provenance — where a
  thread came from — established automatically at join and now shown read-only. The
  cross-agent memory API is untouched: it is what lets one agent pick up a
  discussion another started. Unlink survives alone, because a stale binding needs
  a human escape hatch.
- A silent room hands a waiting agent nothing to answer, asserted field by field.

### Fixed

- Databases opened by pre-rebase 0.9.6 builds now reconcile the former
  migration names with their final 113–119 identifiers before startup checks.
  Existing columns and tables are recognized instead of replaying their SQL
  and failing on a duplicate column.
- Three unbounded paths found by measuring rather than by reasoning: a debate
  context that sent 1 320 210 B to a model, `disc_load_other` returning whole
  discussions, and CLI sessions with no ceiling.
- Awareness backlogs are capped by bytes as well as by message count, with a
  starvation guard so one oversized message cannot block the queue behind it.
- Windows desktop builds use MSYS2's UCRT64 Pango runtime, matching Python.org
  CPython instead of mixing it with legacy MSVCRT DLLs. The docs exporter is
  now built and smoke-tested before the expensive Tauri compile.
- macOS applications are ad-hoc signed by Tauri before the DMG is assembled.
  CI verifies the uploaded image checksum and the strict signature of the app
  mounted from that image, preventing another internally invalid installer.
- The desktop shell no longer navigates to a dead random port or reuses an
  unverifiable CLI/dev/Docker listener. It acquires the shared data-directory
  lock before launch, renders a clear conflict or startup error in its embedded
  UI, and restarts the native process on Retry. Missing optional Docs-sidecar
  resources degrade without terminating the desktop shell. First boot now has
  a visible loading state and a realistic bounded startup window for database,
  keychain or antivirus initialization instead of failing after 15 seconds.


## [0.9.5] - 2026-08-11

### Changed

- Frontend lint debt is ratcheted down: all React dependency-array warnings are
  resolved, Oxlint is a zero-warning gate, and the stricter ESLint baseline is
  pinned in CI. TypeScript tooling and compatible Rust lockfile dependencies
  are refreshed to their current releases.
- Starting a discussion with several explicit `@aliases` now fans the same
  request out to independent agents. Kronn no longer turns that intent into an
  implicit debate or synthesis; agents collaborate only when the discussion
  setting allows it and one agent explicitly delegates to another.
- Multi-agent discussions explain their current collaboration policy before
  launch and link directly to the relevant discussion setting. Sibling replies
  receive bounded context so they can complement one another without repeating
  an unbounded transcript.
- Agent-to-agent delegation now uses an internal marker instead of interpreting
  ordinary generated `@alias` text as an instruction. The marker is removed
  before the response is displayed.

### Fixed

- Desktop releases once again reach the platform build matrix after dependency
  review. The security gate now names each failing backend, desktop or frontend
  audit explicitly and still completes the ordinary version-drift report before
  blocking a release; macOS packaging also opts into Tauri's deterministic
  headless DMG path explicitly. Desktop builds cache the active shared Cargo
  target and clear current plus legacy bundle paths before artifact upload.
- Duplicate root handoffs are ignored, while each explicitly targeted native
  agent still receives one durable dispatch and keeps its requested reasoning
  tier.
- LiteLLM responses no longer expose a leading private DeepSeek-style
  `<think>`/`<thinking>` block. Identically named tags later in the actual
  answer remain untouched.
- Open tabs recover cleanly after a backend restart or half-open network
  connection. The WebSocket client detects missing heartbeat acknowledgements,
  reconnects with bounded exponential backoff, ignores stale socket callbacks
  and resynchronizes the active discussion, sidebar and contact presence after
  reconnecting. A visible banner explains the temporary state without blocking
  draft editing.
- Backend availability is detected quickly without adding healthy-state noise.
  The global status retries every two seconds during an outage and probes
  immediately when the browser comes online or its tab becomes visible.
- A message is no longer considered sent before the backend confirms durable
  persistence. If the request fails before that receipt, Kronn removes the
  optimistic transcript row, restores the exact draft and surfaces the error.

## [0.9.4] - 2026-08-11

### Added

- LiteLLM is a first-class agent with encrypted endpoint credentials, live
  model discovery, independent Economy/Default/Reasoning assignments, cost
  estimates, retained configuration during VPN or proxy outages, and a model
  failure ledger with retry and removal actions.
- Ollama and LiteLLM can call Kronn's bounded native tools for MCP discovery,
  Quick APIs and Quick Prompts while credentials remain server-side.
- ccUsage discovery now supports native, Docker, macOS, Linux and legacy paths.
  Its redesigned panel filters by agent and model, separates detail and
  analysis tabs, compares token use and cost, and ranks the top two models in
  each category.
- Simplified Chinese joins French, English and Spanish as a fully separated,
  lazy-loaded interface locale. Interface language remains independent from
  the output-language context sent to non-CLI discussion agents.
- Workflow steps receive durable UUIDs distinct from editable aliases. Existing
  workflows are backfilled, imports get fresh identities and every relevant
  workflow surface exposes a consistent copy action.
- MCP planning tools accept an explicit discussion target, so agents can create
  plans and tasks in the intended room even when their runtime is not currently
  bound to it.

### Changed

- Settings now follow a clear product hierarchy: Identity, Agents, beta context
  features, Capabilities, Interface, Experience & projects, then System & data.
  Agent defaults, usage, cost, mention colours and reasoning tiers are grouped
  on the relevant agent cards with consistent contextual help and warnings.
- Skills, directives and agent profiles share one searchable capability view
  with filters for kind and Kronn/personal origin, preparing project-scoped
  import and export without conflating MCP servers with skills.
- Model and reasoning selection use the same compact picker in new discussions,
  chat aliases, Quick Prompts and workflow steps. Per-target tiers are persisted
  on each message and remain visible in the transcript instead of inheriting an
  unrelated agent's latest choice.
- New-discussion, chat and workflow prompt editors now share Markdown editing,
  preview tabs, syntax help, clickable examples, emoji completion and alias
  behaviour. Message editing uses the same responsive sizing conventions.
- The workflow wizard uses a larger workspace, sticky navigation, direct access
  to completed stages and individual steps, save/cancel actions from every
  stage, and clearer progressive disclosure for advanced step types.
- Multi-model sends reserve and order one response slot per requested agent, so
  a slow local model cannot make its placeholder disappear or place its answer
  under a later user message.
- Agent collaboration limits are expressed in user-facing terms, configurable
  globally and per discussion, with paid-agent safeguards and explicit
  transparency for unaffected CLI agents.
- Fastly and GitLab cards now document their current CLI authentication flows
  (`fastly auth login`, `glab auth login`) and visually separate credentials
  required by Kronn APIs from optional CLI-token backup fields.

### Fixed

- LiteLLM configuration and assigned tiers remain visible when the proxy or VPN
  is temporarily unavailable; models removed from the catalogue can be retired
  from the remembered failure list.
- GitLab repository discovery follows the authenticated `glab` flow instead of
  treating optional stored credentials as the only source of access.
- Kronn detects when Vibe workspace trust would silently reject the generated
  `.vibe/config.toml`, and reports the blocked directory in host sync, the agent
  card and `kronn doctor` without modifying Vibe's trust store.
- Native multi-model attribution, placeholders and turn ordering now follow the
  exact requested agent/model pair rather than falling back to the discussion
  agent or exposing native tool identities as respondents.
