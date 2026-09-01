# Changelog

All notable changes to Kronn are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release notes for 0.9.3 and earlier are available in the
[legacy archive](docs/releases/CHANGELOG-legacy.md).

---

## [Unreleased]

### Added

- Discussions now report their storage weight, split by what a cleanup could
  actually reclaim: attachment bytes held on disk, extracted document text, and
  message content. The sidebar shows a green / amber / red indicator whose
  detail panel breaks the three masses down and states how much is reclaimable
  without losing any conversation. The indicator is configurable from Settings
  (`[server.discussion_weight]`: `enabled`, `amber_bytes`, `red_bytes`) and
  disabling it removes the queries entirely, not just the badge. Weights are
  served by a bounded batch endpoint — it never scans every discussion.

## [0.12.0] - 2026-08-30

### Added

- Settings now manages any number of named OpenAI-compatible connections in
  one External API zone. LiteLLM, NVIDIA and OpenRouter have dedicated presets,
  while `Other` accepts another compatible endpoint; each connection has its
  own discussion mention alias, credential and Economy / Default / Reasoning
  model mapping. Existing LiteLLM and NVIDIA settings migrate into named
  connections.

- Project details include a Docker tab with Compose service status, bounded
  project and service lifecycle controls, published ports and hosts, host-file
  diagnostics, one-click URLs and recent logs. Project cards expose a running
  badge and a matching collection filter.

- Live Pages selected together can open in a standalone mosaic. Two- and
  three-Page layouts offer explicit arrangements, while larger selections use
  a responsive grid without merging the Pages' sandboxed runtimes.

- Full audits now finish with a deterministic documentation-optimization gate
  that measures the context loaded by each detected agent integration and blocks
  validation when mandatory documentation is oversized, broken or ambiguous.

- Delegated task details expose durable native progress phases, reliable signal
  timestamps and honest telemetry availability instead of inferring activity
  from an attached browser stream.

- Ollama model downloads expose streamed pull progress in Settings and can be
  cancelled while the exact pull is still running, with explicit success and
  failure outcomes instead of one opaque request.

### Changed

- Projects, Discussions, Planning, Automation, Pages and Plugins now share the
  same collection sidebar structure: compact header actions, search, separate
  filter and sort controls, Favorites / Recent / All groupings, row actions,
  keyboard navigation, responsive collapse behavior and shortcut footer.

- Project details separate Audit, Docs and Code into direct full-height tabs.
  Audit launch uses the shared agent selector and sixteen-step full-audit
  briefing, documentation health is explained in plain language, and telemetry
  coverage moved from project cards to Settings.

- External API connection cards use one compact visual hierarchy for endpoint,
  credential state and tier mappings, with inline create, edit, test and delete
  actions. Model mappings stay locked until the current endpoint and credential
  pass a connection test.

- Agent and project pickers use the same searchable, keyboard-accessible
  selector across discussions, Quick Prompts, Quick APIs, workflows and audit
  launch. Quick Prompt comparison has one unambiguous launch action, labels
  named external providers and lets users copy run identifiers.

- Compatible frontend, backend and desktop dependencies were refreshed for the
  release. The unused `backoff` dependency and its unmaintained `instant`
  transitive dependency were removed; the remaining allowed Rust maintenance
  advisory is inherited from the PDF extraction stack. The release-time CLI
  freshness registry was also refreshed against each vendor's stable channel.

- The repository-wide duplicated-line ceiling is ratcheted from 4% to 3%
  against a measured 2.62% candidate baseline, preventing gradual copy-paste
  growth as the codebase expands.

- Backend coverage floors are ratcheted to 83% for lines, functions and
  regions, with the security-sensitive key-management module floors tightened
  to their demonstrated 90–99% range.

### Fixed

- Delegated-task worktrees release shared edit locks deterministically, retain
  commit authority through bounded recovery and report native provider progress
  without presenting missing telemetry as a stalled or free session.

- The long-lived `kronn-internal` MCP bridge can reload when its loaded source
  changes through a versioned, owner-only and size/schema-bounded handoff,
  without trusting a mutable replacement artifact between verification and
  execution.

- External connection tests invalidate stale model selections, bound concurrent
  probes and verify an entered credential with an authenticated minimal request
  instead of trusting a public model catalogue. Migrated NVIDIA connections
  retain their executable default endpoint. OpenRouter uses a non-billable key
  validation endpoint, preserves the full key prefix and upgrades already
  receipted databases without violating foreign keys.

- Re-running a Quick Prompt comparison preserves each target's provider and
  reasoning tier instead of collapsing every result onto one agent, while
  resolving the model currently assigned to that tier.

- Audit/template installation now creates the shared `AGENTS.md` entry point
  unconditionally but emits Claude, Gemini, Cursor, Windsurf, Cline, Copilot,
  Kiro and Vibe instruction adapters only when the target repository already
  declares that integration or the user explicitly launches it for bootstrap
  or audit. Generated adapters are rendered without raw placeholders or
  example commands, and a bounded upgrade repairs only recognizable
  Kronn-managed template ranges while preserving user content.
  Localized briefing prompts now consistently reference the canonical `docs/`
  tree instead of the retired `ai/` path.

## [0.11.0] - 2026-08-22

### Added

- Planning tasks can now be delegated through a durable orchestration lifecycle:
  one selected worker receives a child discussion and isolated managed worktree,
  while the parent discussion follows progress, reviews typed delivery evidence,
  requests bounded changes, reassigns on provider failure and integrates only
  through validated fast-forward with target-SHA and backup-ref guards. See the
  [operator guide](docs/guides/task-orchestration.md).

- Quick Prompt comparisons now include reorderable result columns, model,
  duration and token metrics, independent human and blind AI quality ratings,
  rankings over weighted/AI/human quality, time or tokens, and a reasoning-agent
  path that opens a contextual discussion to improve the source prompt.

- Ollama, LiteLLM and NVIDIA HTTP agents can use Kronn's native tool catalogue,
  preserve honest partial evidence when a tool/context limit is reached and act
  as lower-cost or local fallbacks when hosted CLI providers are unavailable.

- Kronn can now tell you which of a plugin's endpoints keep failing. A plugin
  whose endpoints fail repeatedly carries a "spec to check" badge on its card,
  naming the endpoint and the status behind it. It reads the call log Kronn
  already keeps (`GET /api/api-call-logs/drift`), and separates an endpoint that
  has never answered — a spec that was wrong from the start — from one that also
  succeeds, where the endpoint exists and the call is malformed. The two need
  different fixes, so they read differently. The badge stays out of the way until
  failures accumulate: an alarm raised on healthy plugins is one nobody reads.

### Changed

- The Custom API builder must verify an endpoint before declaring it. It used to
  read the documentation and write down what that implied; documentation goes
  stale, and the resulting spec sent agents chasing endpoints that answer 404 or
  parameters the API ignores. It now tests each endpoint with a real call when it
  can, marks anything unverified as such instead of asserting it, and records the
  response shape and the parameter names that actually work.

### Fixed

- The orchestration bridge no longer makes every principal session pay for the
  full Ollama worker methodology and duplicated scope schemas in its MCP
  catalogue. Selection-time contracts stay visible; exact transport, bounded
  scope, validation and fallback examples now load through `tool_manual` only
  when a principal delegates. This restores the ratcheted catalogue budget while
  keeping spawned local workers on their dedicated two-tool surface.

- The `kronn-internal` bridge now fingerprints the script it actually loaded and
  refuses orchestration mutations whose optional security fields could otherwise
  disappear through a stale MCP schema. After upgrading to 0.11.0, reconnect
  every already-running `kronn-internal` MCP process once and verify
  `bridge_info` reports `stale: false`; a pre-0.11.0 bridge cannot protect the
  transition that made this guard available. Recovery and completion reads stay
  available while a bridge is stale, so an existing execution can be inspected,
  cancelled, reviewed or delivered without replaying its launch.

- Spawned host task workers no longer need write access to a linked worktree's
  shared Git objects or refs. Codex and Claude remain confined to the managed
  worktree and commit through an execution-bound Kronn tool before delivery.
  Git commit endpoints now also treat their explicit file list as authoritative:
  unrelated paths already staged in the index are left staged and cannot be
  included accidentally.

- Native `kronn start` no longer leaves the UI and API offline while another
  Cargo command owns the repository's shared build lock. The supervisor serves
  the last successfully-built backend during the wait and only swaps it when
  the completed build actually produced a different binary.

- API calls from General discussions now execute configurations explicitly
  enabled for General instead of advertising them through `mcp_list` and then
  rejecting them for having no project. The shared API executor also rejects
  unknown or missing path parameters before the network call and names the
  exact parameters expected, replacing opaque vendor 404s with an actionable
  broker error.

- Plugin configurations that are global to nothing, available to no general
  discussion, and linked to no project no longer look healthy while remaining
  invisible to every agent. The Plugins page flags the orphan scope, offers a
  one-click repair through General discussions, and prevents the UI from
  removing a configuration's last remaining scope.

- Live Pages workflow Sync now distinguishes its three outcomes at the action
  itself: a spinner while running, a green check on success, and a red cross
  with the returned error message on failure. A successful run no longer looks
  like a failed validation.

- The discussion plan's compact “+N more in progress” and “+N more ready”
  indicators are now controls: they reveal the remaining tasks in rank order
  and can collapse the list again. Their state resets when switching
  discussions.

- A tool result that had to be trimmed no longer lets an agent report a cut list
  as a whole one. Trimming a document loses text and it shows; trimming a
  collection changes its meaning — one entry out of forty-three reads as "there
  is one", which an agent then states as fact. Kronn now keeps every entry as a
  compact identifier record when that fits; otherwise it emits valid JSON for
  the retained prefix, says exactly how many items it kept out of the total, and
  warns that the count is not final. The Fastly service endpoint also tells
  agents to project `id`, `name`, and `version` instead of loading every
  service's full version history.

- A discussion whose agent is actually working now shows as working, even when
  this browser never opened its stream — a run started from the API, from
  another tab, or before a reload used to sit under the queued hourglass
  alongside the ones genuinely waiting. The sidebar reads the dispatch's real
  status instead of relying on a live-stream flag it may never have received.

- The SpeedCurve plugin describes its API as it actually behaves. Its spec named
  `site` and `since`/`until` where the API wants `site_id` and
  `start_timestamp`/`end_timestamp` — and an unknown filter is accepted and
  silently ignored, so agents analysed another site's data believing they had
  filtered. Four LUX endpoints that answer 404 were removed, pagination is
  documented, and the guidance no longer points at them.

- A model whose tool budget is spent is now made to answer rather than asked to.
  Refusing the call left the declarations in place, so it simply asked again —
  eleven more times, until the round cap. The declarations are withdrawn once a
  whole turn has been refused, which is what the "you used tools but wrote
  nothing" retry already did.
- The workflow detail pane no longer shows a second scrollbar inside the page's
  own. The panes declared `overflow-y: auto` with `overscroll-behavior: contain`
  while having nothing to scroll, which swallowed the wheel; the surrounding
  viewer is the single scroller again.

- A model circling the same tool is stopped after twelve calls to it, instead of
  running until the round cap. The existing guard only caught a call repeated
  argument for argument, so varying one parameter each time slipped past it —
  observed as 47 `api_call`s over 47 minutes, ending with nothing. Kronn now
  refuses the thirteenth and tells the model to answer from what it already has.
- Tool traces name the route an API call took — which plugin, which endpoint,
  which method — instead of a bare `api_call() → ok`. Query strings and bodies
  are still never shown: the route identifies a call, the values are the part
  worth protecting.

- A short question to a local agent no longer dies on its first tool call. The
  context window was sized from the prompt alone, so a one-line question got a
  near-floor window that the first tool result blew past — and since Ollama fixes
  that window when it loads the model, raising it afterwards bought nothing. A
  turn that declares tools now asks for the full ceiling up front, and oversized
  results are trimmed against the window actually granted rather than the one
  that was theoretically available.

- An agent working through tool calls no longer looks dead. A batch progress
  tick was clearing the per-discussion "running" state, which unmounted the
  streaming bubble mid-run — taking the live tool traces and the elapsed counter
  with it — and flipped the sidebar card back to the queued hourglass while the
  job was still running. A progress tick means the batch advanced, not that this
  discussion finished.
- The workflow detail pane scrolls with the wheel again. Its container had an
  automatic height, so the pane grew with its content instead of overflowing:
  nothing scrolled inside it and reaching the bottom meant dragging the page
  scrollbar.

### Changed

- An agent exploring a repository is no longer cut off after eight tool rounds.
  That ceiling was written for an agent calling one API — "list, then call, then
  maybe retry once" — and never revisited when HTTP agents gained file and git
  tools: finding files costs two or three rounds before the first read, so a
  triage that was genuinely working died with its report unwritten. The
  configured execution duration is now the real limit; the round cap remains as
  the anti-runaway backstop underneath it.
- Tool traces name what they did: `find_files({"pattern": "..."})` rather than
  `find_files()`. Only the workspace tools, whose arguments are paths and globs
  in your own repo — `api_call` and friends stay name-only, since their
  arguments can carry secrets. Eight identical-looking `find_files() → ok` lines
  said nothing about what was actually searched.

- A batch item now gets the execution ceiling the operator configured, instead
  of a fixed twenty minutes. A batch item is one agent run, so "maximum agent
  execution duration" applies to it too — a local model legitimately spending
  many tool rounds was being cut off well before that setting. It remains a
  guard, not a switch: the value stays clamped to 1–120 minutes.
- A run stopped by that ceiling no longer claims the user interrupted it. The
  cancel signal carries no reason, so the message states what happened and names
  both causes rather than asserting one it cannot know.

### Added

- Each agent now carries its own concurrency limit, set on its card in Settings.
  A local agent is capped because the machine is the limit — Ollama defaults to
  1, since it serves a single inference slot and a second run only queues and
  discards the KV cache the first one warmed; a CLI agent defaults to 5. Remote
  providers stay unlimited: LiteLLM and NVIDIA are endpoints someone else
  scales, and a cap there is about spend, not this machine. Admission is atomic,
  so a job whose agent is at its limit stays queued and no request is sent.

### Fixed

- An Ollama agent that read a large file or diff no longer fails with a bare
  "API server error". The context window was sized once from the system context
  and the user prompt, then never re-sized as the tool loop appended results, so
  Ollama truncated the history until the user turn itself was gone and rejected
  the request. The window is now re-sized from the messages actually being sent,
  and the truncation failure reports what happened instead of pointing at tool
  calling. A single result too big for the window at its widest is trimmed rather
  than dropped, and says how many bytes went missing so the model does not reason
  on a silent truncation.

- Local Ollama steps no longer pay for reasoning tokens they discard. Kronn sent
  qwen3 models the `/no_think` control token, which recent Ollama runtimes have
  made nearly inert; the request now also carries `think:false`, which they do
  honor. On a classification prompt the generated-token count drops from 240 to
  2 (`qwen3:8b`), 226 to 1 (`qwen3.6`) and 82 to 1 (`qwen3.8`) for the same
  answer. The flag is only ever sent to turn reasoning off, so a model Kronn
  makes no claim about keeps its own default.
- The agent freshness pill works again. Every latest-known-version constant had
  fallen behind — Ollama was pinned to 0.4.7 against a current 0.32.14 — so no
  agent was ever reported as out of date.

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
