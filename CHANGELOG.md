# Changelog

All notable changes to Kronn are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release notes for 0.9.3 and earlier are available in the
[legacy archive](docs/releases/CHANGELOG-legacy.md).

---

## [Unreleased]

## [0.10.0] - 2026-08-14

Kronn can now turn scheduled API collection into a readable, continuously
updated HTML report without sending mechanical data work through an agent.

### Added

- **Live Pages** provide versioned HTML reports backed by named JSON datasets.
  Pages render in a credential-free sandboxed iframe, retain data and HTML
  history independently, and can be linked to workflows or discussions. MCP
  agents may also create standalone Pages without first joining a Discussion;
  the current or an explicit Discussion remains an optional provenance link.
- The Pages library reuses the Discussion navigation model with search,
  favorites, multi-selection, archive and explicit deletion. Its navigation
  entry appears only after the capability has been activated. Each Page also
  exposes a compact header dropdown for its three latest successful workflow
  refreshes, with publication time, data revision and dataset-level `modified`
  / `unchanged` deltas, per-dataset retained JSON size, and direct navigation
  to the exact originating run. A linked workflow can also be relaunched from
  this dropdown without leaving the Page. The HTML studio adds line numbers, syntax
  highlighting, revision history, side-by-side comparison and restore-to-draft.
- Three deterministic workflow steps cover the complete data path:
  `CollectApiData` runs saved Quick APIs and saved shell-free, allowlisted CLI
  collectors concurrently under stable aliases, `TransformData` selects and aggregates typed JSON with bounded
  JSONPath recipes, and `PublishPageData` atomically updates one or more Page
  datasets. All three consume zero model tokens.
- **Quick Execs** are now reusable Automation resources with create/edit/test,
  project binding, declared variables, safe literal argv, bounded execution and
  JSON/CSV/text/lines normalization. Agents receive symmetric `qe_list`,
  `qe_create_draft`, `qe_update` and `qe_run` MCP tools, and Workflow Architect
  composes them through `CollectApiData.sources[].quick_exec_id`.
- Workflow authors can test a complete collection, reuse the previous step's
  real sample, map fields from an interactive JSON tree, preview the transformed
  result, and add a Transform or Page step directly from the collector.
- Page headers export a browser-rendered capture of the materialized report
  (runtime dataset content, authored CSS, SVG and canvas charts) to PDF or
  fixed-layout DOCX, preserving CSS that the document sidecar cannot interpret. Dataset totals
  open an inline viewer with a dataset selector, tabular preview, retained size,
  and CSV/JSON export; the refresh dropdown remains a second entry point.
- Workflow export bundle v3 now carries and remaps referenced Quick Prompts,
  Quick APIs, Quick Execs, Page templates/contracts and transitive sub-workflows. Credentials
  and retained Page observations remain local by design.
- Quick APIs and workflow API steps support vendor-neutral, run-anchored time
  expressions with signed offsets, IANA timezones, minute/hour/day flooring and
  RFC 3339, local ISO, date or Unix formats. Parallel collectors and resumed
  runs reuse one durable timestamp, so rolling cron windows cannot split across
  an hour boundary.
- The Kronn MCP and Workflow Architect expose Page discovery/authoring and the
  full data-pipeline schema, allowing an agent to create standalone or
  mock-backed Pages and then wire them to real workflow data.
- **Continual Learning** ships as an explicit, default-off beta. Once enabled,
  agents may propose durable facts, preferences or pitfalls through a typed MCP
  tool, but evidence checks and a human decision are required before Kronn
  writes to the dedicated user or project learning document. Pending candidates
  remain visible from a global badge and stale evidence is rechecked on a
  bounded background sweep.
- Every Discussion header now opens a searchable **Assets** inventory for all
  shared or agent-generated files in that room. Images keep the in-app
  carousel, filters separate images/files/pending uploads, large histories load
  forty cards at a time, disk-backed files can be downloaded, and each attached
  asset links back to its source message. The Assets and modified-files header
  actions stay hidden while their respective counts are zero.

### Changed

- Quick Prompt Compare now selects explicit agent + model-tier targets with
  the same picker used across Kronn, including comparisons of two tiers of the
  same agent. A launch opens a dedicated in-app comparison workspace with rich
  Markdown columns, live run states and links to each durable child discussion;
  any existing batch can reopen the workspace from its actions menu.
- The public-site screenshot gallery now opens in an accessible in-page
  carousel with keyboard navigation while preserving modified-click new-tab
  behavior. Its reproducible demo seed includes public-safe repository content
  plus an Automation showcase and isolates MCP sync from the real user home.
- The Automation library now follows the same sidebar + detail interaction as
  Discussions and Pages. Workflows, Quick APIs, Quick Prompts and Quick Execs
  share one ordered, searchable, project-filterable navigation surface instead
  of four unrelated tab layouts. Selecting a Quick API, Prompt or Exec opens its
  editor immediately in the detail pane; launch, compare, batch, history, ID,
  export and delete controls remain available in a compact command header. Long
  Quick Exec invocations stay on one ellipsized, inspectable line so AWS queries
  and structured arguments cannot make that header consume the viewport. The
  library labels them as `Quick Execs (CLI)` and their Test action now uses the
  same primary CTA treatment as Quick Prompt launches. The guided tour now
  introduces the unified library, follows the sidebar order and includes Quick
  Execs instead of describing the former tab layout.
- The primary navigation now follows the product flow: Projects, Discussions,
  Planning, Automation, Pages, Plugins, then Config. Pages therefore sits next
  to the workflows that publish it, while infrastructure remains grouped last.
- `CollectApiData` now surfaces a failed Quick Exec's stderr in its summary,
  gives expired AWS SSO sessions an actionable `aws sso login --profile …`
  diagnostic, and fails an entirely empty collection even when every source is
  optional. Optional failures remain `PARTIAL` only when another source
  actually produced data.
- Page destinations are shared resources rather than workflow-owned children:
  several workflows can publish different datasets to the same Page, while
  both Workflow and Discussion links remain visible from the Page library.
- Workflow previews now label data steps with their human-readable type and
  useful source/field counts. A `PublishPageData` preview links directly to its
  target Page.
- Disabling Continual Learning stops capture and removes its injected project
  pointer without deleting previously validated learnings; existing pending
  candidates can still be reviewed so the queue can be drained safely.

### Fixed

- Discussion runs now distinguish the configurable inactivity watchdog from a
  separately configurable absolute execution duration (1–120 minutes). The
  former hidden 30-minute constant no longer terminates healthy, actively
  streaming agents after operators explicitly allow longer runs.
- Joined CLI agents can publish local images and files with `disc_append`.
  Kronn uploads them through the authenticated context-file path, pins only
  those files to the exact message, and refreshes the open discussion live.
  Clicking a thumbnail opens an in-app gallery with previous/next navigation,
  keyboard controls, an image counter, and an explicit new-tab action.
- Historical message attachments no longer consume the 20-file composer
  staging limit, returning to an already-open Discussion refreshes a stale
  attachment cache, and a failed multi-file MCP append compensates every upload
  from that batch instead of leaving hidden pending files behind.
- Time-series retention preserves append order when several points share the
  same observation timestamp instead of using random UUID order as a tie-breaker.
- Quick API POST bodies are sent as their typed JSON value instead of being
  serialized twice. Manual Quick API calls and workflow `ApiCall` steps now use
  the same body contract, including audit-log attribution.
- Global Quick APIs can be tested from a collector even when the workflow has
  no project, while project-scoped APIs still fail closed without their project.
- Transform previews now preserve the selected JSONPath result instead of
  presenting a misleading nested projection.

## [0.9.7] - 2026-08-13

This release is a reliability sweep across discussions, workflows, project
audits, plugins and desktop packaging. GitHub Copilot CLI issue #150 remains in
the backlog because that provider is no longer available in the test
environment; it is not presented as fixed.

### Added

- LiteLLM and Ollama Agent workflow steps can use a bounded set of Kronn-native
  tools for configured APIs, Quick APIs and read-only Planning. Tool execution
  stays project-scoped, secrets remain server-side and run details retain only
  the tool name and outcome—not arguments or credentials. A completed HTTP
  Agent step with no recorded call says so explicitly and points operators to
  a tool-capable model or deterministic `ApiCall` step when external data was
  expected.
- Plugin imports now end with an explicit assignment table. Every imported
  configuration starts with **Global** selected for convenience, but Kronn
  applies that scope only after confirmation; operators can instead select one
  or more projects.
- Context Audit snapshots make documentation drift visible on project cards,
  and existing documentation can be explicitly attested without pretending an
  AI audit ran.
- Desktop CI now smoke-tests PDF and DOCX generation before Tauri packaging,
  preserves platform diagnostics and requires non-empty Windows, macOS Intel,
  macOS ARM and Linux installers before a release can proceed.

### Changed

- All discussion agents now receive the same compact rich-output contract.
  Native and CLI agents can intentionally produce Mermaid diagrams, sandboxed
  HTML previews with PDF/DOCX actions, or CSV/XLSX/PPTX export cards without
  relying on the document-generation skill having been auto-selected. Mermaid
  diagrams now expose shared 50–250% zoom controls in inline and fullscreen
  views, with scrollable overflow for dense graphs.
- HTTP discussion agents are told that configured REST APIs are Kronn-native
  tools, not MCP servers, and are directed through API/Quick API discovery
  before inventing an unavailable vendor integration.
- Plugin cards expose registry/configuration drift and retired registry entries
  instead of silently rewriting encrypted configuration. Microsoft Graph's
  Docker CLI-token path again uses the authenticated Azure CLI when available.
- Workflow documentation now describes the exact, limited overlap with OpenAI
  Symphony: four workspace-hook names are shared, but Kronn does not import
  `WORKFLOW.md` and does not implement Liquid templates.
- Serious/critical axe findings on the five main pages now have a zero baseline;
  future browser failures attach the precise violation targets as diagnostics.

### Fixed

- Native discussion-agent startup failures are no longer left as silent,
  indefinitely pending targets. Catalogue/model errors and temporary provider
  outages now share one compact diagnostic card; the latter can retry only the
  failed agent against the original user turn without replaying successful
  sibling agents. Legacy structured LiteLLM 404 messages remain readable.
- Resuming a CLI session can no longer rotate credentials into a different
  discussion. Peer traffic receipts are cursor-based and content-free, so an
  agent can detect unseen work without rereading the transcript.
- Shared and inherited workflow worktrees retain durable ownership while child
  jobs need them. Restart reconciliation cancels stale workflow children,
  refuses unsafe fire-and-forget fan-out and purges only managed terminal
  worktrees.
- Unknown workflow template variables, malformed filters and unclosed
  placeholders fail before a step performs an API call, command, notification,
  gate or agent invocation.
- Project audit badges distinguish installed templates, bootstrap evidence,
  completed audits, human attestations and validation without allowing legacy
  backfill markers to overwrite a newer state.

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
