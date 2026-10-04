# Changelog

All notable changes to Kronn are documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release notes for 0.9.3 and earlier are available in the
[legacy archive](docs/releases/CHANGELOG-legacy.md).

---

## [Unreleased]

### Added

- Full access is now a visible, accessible switch on each agent card, with a
  risk dialog before it is enabled, and the setup wizard has an Access step
  that offers it agent by agent, off by default (KT-975).
- The plugin catalogue offers MySQL / MariaDB (KT-996, #217), through the
  community server `@benborla29/mcp-server-mysql`: host, user and password,
  with an optional port and database, read-only unless a write flag is set on
  the server.
- A skill can declare variables in the Claude Code format: an `arguments`
  list used as `$name` in its body, an optional `argument-hint`, and Kronn's
  label, default and control for each one as JSON under
  `metadata.kronn-variables`. Kronn reads, validates and writes them back
  unchanged, and such a skill shows a "Variabilisé" badge in every skill list,
  its hover naming the placeholders (KT-906).

- The project card says when the branch has lost its audit evidence while
  another commit still carries `docs/.kronn.json`, names that commit and its
  branches, and restores the file on request (KT-993). A Full audit completed
  on this Kronn instance now counts as audited when the branch has no evidence
  file, and the audit timeline lists the audits the file records (another
  instance, an attestation, legacy evidence) with their date and provenance
  instead of staying empty. The timeline loads in one request instead of up to
  thirteen.

### Fixed

- The desktop release pipeline now fails before building when the pushed tag
  differs from `VERSION` or a version marker is stale, builds the release job
  from the requested tag on a manual run, and expects exactly the installers
  Tauri is configured to produce (`.exe`, `.dmg`, `.deb`) instead of accepting
  an `.msi` or `.AppImage` in their place; the release notes give the real
  `.deb` file name. The release commit is resolved once and the checkouts,
  the dependency review and the full CI gate (label-gated jobs included) run
  on it, and the release waits for that gate (KT-1023).

- Opening a project shows the last known Git and Dependencies results at once,
  with their date, and refreshes them behind a discreet indicator. A failed
  refresh keeps the previous result instead of blanking the block, and the
  Git block renders local status before the pull-request lookup completes
  (KT-989).
- Plugins page: the toolbar button that rescans the projects' `.mcp.json` files
  is labelled as such instead of "Sync", the preview names how many projects
  are affected, and the report after applying lists created, merged, rewritten
  and removed counts (KT-833).
- Turning on full access for GitHub Copilot CLI failed with "Agent does not
  support access flags", and Vibe and Kiro accepted a flag they ignore. The
  backend now accepts exactly the five agents that honor it (KT-975).
- A guarded action (save, create, toggle) now always runs with the current
  props and translations instead of the ones from the first render, and a
  browser with disabled or full storage no longer crashes the Discussions,
  Dashboard, Workflows or Settings pages when they remember a preference
  (KT-1022).

- Listing projects no longer writes `docs/.kronn.json` into a checkout that
  only carries legacy audit markers (KT-993): the status is computed in memory
  and the file is written by audits and validations only, atomically.



- A validation no longer asks again about a TD already decided (KT-938): the
  decision a card writes on the TD sheet (confirmed, rejected, accepted
  decision) is read back, so the full validation after a resume asks only
  about new or undecided TDs and names the partial validation it follows. A
  deferred TD is asked again, since its decision was postponed. Re-audits stop
  carrying rejected TDs and accepted trade-offs in the index, and no longer
  report them as missed. A TD whose status sits only in its YAML front matter
  is read and updated too.



- An audit of an already documented repository no longer fails on dead links
  in documents Kronn does not own (KT-1020): moved `docs/legacy/` documents and
  root instruction files with human content outside Kronn's block are
  reported, not blocking, and the repair step never rewrites them.



- Audits no longer receive the `kronn-internal` and Memory MCP servers
  (KT-935): the `.mcp.json` filter and the ACP broker apply the same exclusion,
  so an OpenCode, Claude or Codex audit gets neither Kronn's write tools nor a
  knowledge graph shared across projects, and the refusal is logged instead of
  appearing as a registry error. Discussions on the audited project keep
  their tools.



- The documentation template and audit are more consistent (KT-934): a
  validation is refused while a document Kronn owns still carries a
  placeholder or a `TODO:` marker, the final review skips the `TEMPLATE.md`
  gabarits like the document gate does, and no root redirector is written
  toward a missing `docs/AGENTS.md`.



- A resumed audit keeps the tokens, duration and cost of the steps it inherits
  and names the run that spent them, refuses to start when it cannot record
  them, and warns when the sources moved since the run it continues (KT-1021):
  each audit run now records its commit, branch and source fingerprint. Token
  figures count cached prompt tokens the same way for every agent, and the
  timeline shows a step whose agent reported nothing as unknown instead of
  hiding it.
- Natively, a Claude discussion kept none of its project's MCP servers once
  one of them carried a credential in its environment (KT-1003): the whole
  `.mcp.json` was refused. Each authorized server now stays on its own. Its
  environment values reach Claude as `${KRONN_MCP_…}` references in the
  command line and as values in the Claude process's own environment, never in
  argv, ACP payloads or events. A credential passed in `args` still leaves its
  server out.
- Security hardening (KT-1009). The WebSocket no longer streams every live
  event to any private-network peer: without the API token only loopback (or,
  under Docker, the gateway's local clients) counts as the local UI, and
  `X-Real-IP` is honoured only from the Docker gateway (`X-Forwarded-For`
  never). The container no longer writes the GitHub token into `.netrc` for
  gitlab.com (a `GITLAB_TOKEN` feeds that entry, and a stale one is removed).
  Saving an OpenAI key rewrites `~/.codex/auth.json` atomically, owner-only,
  and keeps a ChatGPT login's mode. Key and recovery temp files are created
  owner-only and never follow a planted symlink, scheduled backups are created
  0600, masked values no longer panic on accented or emoji edges, the agent
  read guard also refuses Kronn's key, config, database, `.mcp.json` and MCP
  backups, and exports drop the failure webhook URL.
- A plugin bundle can no longer change the command a trusted plugin runs
  (KT-1010): custom arguments in a bundle replace the plugin's whole command
  line, so an import now drops them, and says so, unless the importer gives
  explicit consent (`accept_args_override`). Custom arguments, which may hold
  tokens, now travel only in the encrypted bundle, and a manual import can no
  longer claim a `mcp-` or `api-` id reserved for the built-in catalogue.
- Native skill and agent files are safer to write (KT-1011): nothing is
  written through a symlinked folder such as a `.claude/skills` pointing
  outside the repository, every front-matter value is quoted so a newline in
  a skill description can no longer add a key like `allowed-tools`,
  concurrent runs on one project no longer drop each other's entries from the
  ownership ledger, and "Migrate to .agents/skills" no longer offers the
  catalogue copies Kronn synced itself as repository skills.
- A tracker issue title or a step output can no longer run as code in an
  Exec step (KT-1017). Saving a workflow now refuses a template placeholder
  inside an interpreter's inline script (`bash -c`, `python3 -c`, `node -e`…),
  where it would be parsed as code, and says how to write it safely: pass the
  value as a later argument (`["-c", "echo \"$1\"", "_", "{{issue.title}}"]`),
  or, in a shell script, use the new `{{value|sh}}` filter, which renders one
  single-quoted word. `{{run.id}}` and `{{time.now…}}` stay allowed. Saved
  workflows keep running unchanged until they are next edited.

- A task delegated to Gemini, Copilot, Kiro or OpenCode as a launched worker
  ran with the discussion's full access and without its delivery context, so
  it could act beyond the worker scope and never deliver (KT-1012). These
  native ACP agents are now refused as launched workers with a clear reason,
  at preparation and at launch; one worker policy now decides every route, and
  an exact joined CLI session of the same agent stays eligible.
- Agent streams are sturdier (KT-1014). An accented letter or emoji split
  across two network chunks of an HTTP model's reply (Ollama, LiteLLM,
  OpenRouter, NVIDIA, Custom) is decoded intact instead of becoming two `�`,
  which could also corrupt a file path in a tool call. A non-UTF-8 byte on a
  CLI agent's output costs one replacement character instead of stopping the
  reader and leaving the agent blocked until the watchdog; a native ACP agent
  keeps its session. After a Claude or Codex turn exits normally, the
  processes it left behind (stdio MCP servers, background commands) are
  stopped with it instead of piling up.
- When Ollama refused an unreadable tool call and Kronn replayed the request,
  the text the refused attempt had already streamed (a preamble before the
  call) appeared twice in the reply and in the saved message (KT-944). On an
  Ollama turn that offers tools, the first 512 bytes of an attempt are now held
  until the attempt ends: a replayed attempt's preamble is dropped, a
  successful one is shown, and a longer answer streams live as before.
- An audit step on a hosted large-context model (OpenRouter, LiteLLM, NVIDIA,
  Custom) could explore for hours, resending its whole history each round,
  because context pressure was never measured on that wire and the write
  window only opened at round 242 of 250 (KT-998). Pressure is now measured on
  a bounded step budget (the smaller of the model's window and 128 000
  tokens), for audits and HTTP workers alike, and an audit step writes its
  deliverable once it has spent 1 500 000 input tokens. Both budgets can be
  set with `KRONN_HTTP_STEP_CTX_BUDGET` and
  `KRONN_HTTP_AUDIT_STEP_INPUT_BUDGET`.
- The cost an agent reports itself is no longer lost (KT-997, backend part).
  Claude Code names it `total_cost_usd`, which Kronn did not read, and the
  default Claude route dropped it anyway; OpenRouter's `usage.cost` was never
  read. Both now reach the run's usage as integer micro-USD, summed per
  response, and stay unknown (never zero) when not reported. Showing it per
  audit step comes later.

- A workflow run now keeps the project it was launched in (KT-1015). A global
  workflow launched from a project lost that project after a gate approval or a
  resume and continued with no working directory, and its worktree was never
  cleaned at startup. A sub-workflow pinned to another project now runs in that
  project's repository instead of the parent's worktree; as a foreach child it
  is refused, since it would have to share the parent's worktree.
- One failing workflow no longer cancels the scheduler's tick (KT-1016): a
  missing variable or an unreachable tracker stopped the loop, and every cron
  occurrence of the workflows after it was lost for good. A tracker issue is
  now marked processed only once its run is admitted, so an issue refused by
  the concurrency limit or the preflight is polled again instead of never
  running.
- Workflow run lifecycle holes (KT-1018). Approving a gate whose worktree had
  disappeared continued the run in the main checkout; it now fails with the
  reason instead. A run paused on a gate keeps its concurrency slot, and a gate
  approval or an interrupted resume is refused while the limit is reached. A
  run that stops on an error removes its worktree right away (keeping a branch
  that holds unintegrated commits) instead of waiting for the next start, and
  a failing or hung `before_remove` hook no longer keeps the worktree. Adding a
  Gate to a workflow another one uses as a sub-workflow is refused, and a
  workflow imported from a repository's `kronn/` folder goes through the
  editor's save rules. Isolated workflow worktrees get the project's agent
  configs (`.mcp.json` and the others), so an Agent step keeps the project's
  MCP servers and its strict MCP config instead of the host's.
- Workflow worktrees no longer pile up as `prunable` entries (KT-985). A
  worktree a step created inside its run's worktree (`.kronn/pr-N`) is now
  removed with it, and at startup Kronn drops the stale entries of its own
  worktrees under `.kronn/` in every project, leaving the user's other
  worktrees alone. A run still keeps a branch that holds commits no base has,
  whether it succeeded, failed or was cancelled.

### Changed

- Plugins page: one export and one import flow, the plugin bundle, where each
  plugin's scope and CLI exposure are chosen on import. The per-plugin JSON
  export and the paste-a-spec import are gone, and a plugin is deleted from
  its detail sheet only, behind its two-step confirmation (KT-833).
- The release notes are now generated from the installers a release really
  carries (KT-1004): one direct download link per attached file, the version's
  CHANGELOG section above the install table, and the AppImage advice only when
  an AppImage is attached. A missing platform or CHANGELOG section fails the
  release job instead of publishing a draft that names absent files.

## [0.14.2] - 2026-10-03

### Added

- The project card's Audit tab is now a timeline (KT-977): briefing,
  template, the audit's steps grouped as the pipeline runs them (documentation
  core, specialised audits, consolidation), validation, validated. Every step
  shows, before it runs, the file it writes and what it covers (from the new
  read-only `GET /api/audit/steps`, built from the chain the pipeline runs),
  then its status, a readable reason when it failed or was interrupted, its
  duration and when it last ran. A resumed run is shown whole: its steps are
  merged with the run it continued. A failed group resumes from its own button,
  and the timeline says that the consolidation reruns after it. The agent panel
  groups installed CLIs (recommended), HTTP agents and local models, shows the
  model of each level (⚡ Eco, 🎯 Standard, 🧠 Advanced, the recommended one)
  and warns about lower levels, local slowness and data sent to an HTTP
  provider. The briefing is filled in and saved from the timeline without
  opening a discussion, and a note explains what changed in the audit.

- Any HTTP agent can now run an audit, at the user's choice (KT-980): NVIDIA
  and named connections such as OpenRouter join Ollama and LiteLLM. Full and
  partial audit requests take an optional `connection_id`; the audit uses that
  connection's endpoint, key and model for the chosen level, and its
  validation discussion keeps the connection. A `Custom` agent without a
  connection is refused by name. Vibe, which has no file tools, still cannot
  audit.

- The Automation page lists the skills. "Skills" is a fifth type in the
  sidebar Filter, with its count, next to Workflows, Quick APIs, Quick Prompts
  and Quick Execs, and covers the built-in skills as well as the ones you
  wrote. A skill sits under every project that lists it among its default
  skills, and under "No project" when none does; the flat lists (Favorites,
  Recent) show it once. Opening one shows its sheet in the main column: name,
  description, category, the projects that use it, and its `SKILL.md` — rendered
  as Markdown without raw HTML (the same rendering as a repository resource) or
  as Source. Name and description are searchable, like the other types. The
  sheet is for reading: no variable, no launch. A skill is still edited in
  Config › Skills, which the sheet links to; a skill you wrote can be deleted
  from its row or its sheet, a built-in one cannot. A skill has no pin on the
  server, so its favorites are kept in this browser. Merging skills with Quick
  Prompts stays KT-906 (KT-914).

- Skills are real Agent Skills, written where agents look for them. Writing a
  skill into the repository now produces `.agents/skills/<slug>/SKILL.md` in the
  standard format (valid `name` and `description`, Kronn's own fields under
  `metadata`), never `kronn/skills/`, which keeps only what has no native home.
  A repository that already has `kronn/skills/<slug>` is offered the move, with
  its lock entry, identity and approvals following the skill and no duplicate
  left behind. The Skills tab gets "Migrate everything to .agents/skills" when
  skills sit in `.claude/skills`, `.gemini/skills`, `.codex/skills`,
  `.github/skills`, `.opencode/skill(s)`, `.cursor/skills` or `kronn/skills`:
  a recap lists every move (source → target) and every conflict (same slug,
  different contents — you pick the version, nothing is overwritten
  silently) before anything is written, and nothing is committed
  (KT-903).

- Each Agent step attempt exposes the CLI session it ran in and what it cost:
  `agent_provenance.attempts[].session_id` is the id Claude Code reports on its
  `init` line, the name of its transcript, and `cost_usd` is priced like a
  discussion reply (KT-894) — `null` with `cost_unknown_reason` when the
  counters or the rate are missing. Callers no longer have to find transcripts
  by the worktree name or a token added to the prompt. `task_exec_status` lists
  the worker's CLI sessions, rework attempts and relaunches included
  (`worker_sessions`, with their cost), and its compact view names the latest.

- The project's repository resources (`GET /api/projects/:id/repository-resources`)
  now describe both sides of every item. Its `status` is one of
  `repository_only`, `kronn_only`, `up_to_date`, `repository_newer`,
  `kronn_newer`, `conflict`, `approval_required` (or `native_skill` for a skill
  found outside `kronn/`). Each item carries the repository date (last commit's
  author and date, else the file's mtime), the Kronn date and `aligned_at`; its
  `required_secrets` with whether Kronn's stored configs hold each name; its
  ADR-005 level; and `write_preview`, every path a publish would write,
  `docs/AGENTS.md` and the router skill included. The listing also reports
  `can_write_repository` with its reason and `uncommitted_managed_paths`. What
  differs is not in the listing: `GET .../repository-resources/comparison?kind=&id=`
  returns, for the three states where the sides differ, a unified diff per file
  (`file_diffs`, the artifact's HTML included) and, for workflows, Quick APIs
  and Quick Execs, a field-by-field diff (`field_diff`). The Compare sheet asks
  for it when it opens and shows a loading state meanwhile.
- The sheet of a resource in the "AI & automation" tab (skill, Quick Prompt,
  Quick Exec, Quick API, workflow, artifact) now shows the resource itself, under
  its details: a "Repository / Kronn / Diff" picker over the file as the
  repository holds it, as Kronn holds it (masked, as a publish would write it) or
  what differs, using the same diff as the Compare sheet. A skill's `SKILL.md` is
  rendered as Markdown, with a "Rendered / Source" switch; every other kind is
  shown as text. A mode with nothing to show is off with the reason on hover
  ("not in the repository", "not in Kronn", "identical"). The sheet opens on
  Repository when only the repository has it, on Kronn when only Kronn has it or
  when the two are identical, and on Diff when they differ, saying which side is
  newer when only one moved. The text comes from
  `GET .../repository-resources/comparison`, which now answers for every
  resource, not only the ones that differ: `files` lists each file with its
  `repository` and `kronn` text (either absent when that side has no file, cut
  and flagged `truncated` past 512 KiB). Both sides are masked the same way,
  the repository's too — a secret typed by hand into a file in Git never
  reaches the texts, the diffs or the field view — while a `secret://NAME`
  reference stays readable. The listing still carries no content (KT-913).
- The skills the Kronn catalog provides that a project does not have yet
  ("Available in Kronn, not in this project": built-in ones and the ones a user
  wrote) open their sheet like every other row: the `SKILL.md` as Kronn holds
  it, rendered or as source, with the repository mode off ("not in the
  repository") and "Attach to the project" unchanged (KT-913).
- A native skill outside `kronn/` can be used in Kronn without `kronn.lock`
  (`POST .../repository-resources/skills/use`: a read-only reference to its
  path) or copied into Kronn as a managed skill
  (`POST .../repository-resources/skills/copy`, which writes nothing into the
  repository and replaces an edited copy only with `overwrite_kronn_changes`).
  The same skill under several skill folders is grouped by slug, every path
  kept, and flagged when the copies differ.
- The Automation sub-tab of a project's "AI & automation" tab filters by type
  (All / QP / QA / QE / Workflow), each chip with its count. It stacks with the
  location filter and the search, and every chip counts what choosing it would
  show given the others.
- The global Automation page filters by state (All / Favorites / Active /
  Inactive; only a disabled workflow is inactive) next to the type and project
  filters, and sorts its sidebar by name, last modification or type (favorites
  stay first). On a screen narrower than 640 px, the project tab's filters fold
  behind one "Filters (n)" button.
- `task_exec_update_validations({task_execution_id, validations, reason})`: the
  principal replaces the validations of an existing execution without
  relaunching it. The set replaces the current one, is held to the launch rules,
  and is journaled (`validations_replaced`) with the actor, the reason and the
  previous set; the room, the worktree, the attempts and the earlier validation
  results are untouched. It is refused while an integration runs, once the
  execution is terminal, and for a campaign run's shared gates.
- `task_exec_reassign({task_execution_id, validations, reason})`: the principal
  replaces the validations of an existing execution without relaunching it, with
  `validations` in place of `worker` (one change per call). The set replaces the
  current one, is held to the launch rules, and is journaled
  (`validations_replaced`) with the actor, the reason and the previous set; the
  room, the worktree, the attempts and the earlier validation results are
  untouched. It is refused while an integration runs, once the execution is
  terminal, and for a campaign run's shared gates. It is a change of an existing
  tool rather than a new one so the MCP catalogue does not grow.
- `task_exec_prepare` accepts the `validations` it will launch with and answers
  `launchable: false` (reason `invalid_validations`) for one that could never run.
- `tool_manual({tool: "task_exec_prepare"})` now states how a validation runs: one
  allowlisted binary and literal arguments, no shell, from the root of the
  worktree, with `pnpm --dir …` / `cargo … --manifest-path … --target-dir …` as
  the forms that replace `cd … &&` and `VAR=…`.
- Resources of a project's repository listing now say what they are linked to
  (KT-905). Each item of `GET /api/projects/:id/repository-resources` carries
  `uses` (what a workflow's steps or an Artifact's action blocks reference:
  Quick Prompts, Quick APIs, Quick Execs, sub-workflows, Artifacts) and
  `used_by`, as `{ kind, id, slug, name, missing }`; a reference to something
  the project does not hold is kept and flagged `missing`. A row shows a
  discreet "3 linked" count and its sheet lists "Uses" and "Used by", each
  entry opening the linked resource. Ticking a resource ticks everything it
  needs (recursively, loops included) with a "3 linked items added" line; a
  resource a ticked one still needs cannot be unticked ("required by
  nightly-triage"); "Align all" follows the same rule. Writing or loading a
  single resource announces its dependencies and includes them by default;
  leaving them out warns that it will only partly work.
- An audit, Full or partial, can run on Ollama or LiteLLM (KT-924). Both were
  refused on the ground that an HTTP agent has no filesystem, which stopped being
  true when Kronn gave them native file tools: the audit now hands the agent
  those tools, scoped to the project. Every path is canonicalised against the
  project and refused when it leaves it (`..`, an absolute path, a symlink), there
  is no shell, and a truncated read says so. The audit's agent is offered only
  `read_file`, `list_files`, `find_files`, `search_text`, `git_status`,
  `git_diff`, `git_log` and the four write tools — no `web_fetch`, no
  `git_commit`, no plan or REST-plugin tools. The model and endpoint come from the
  tiers and endpoints set in Settings → Agents, like a discussion or a workflow
  step. A step whose agent writes nothing is still recorded as failed, whether it
  left its file missing, still the template, or untouched. Stop reaches an HTTP
  agent's request and tool loop, which a process kill cannot. The refusal that
  remains (Vibe, NVIDIA, Custom) now names the agents that are accepted, and the
  audit's agent picker offers Ollama and LiteLLM. NVIDIA and Custom stay refused:
  sending a whole repository to a hosted service is a decision the audit does not
  take for you.

- An installed Ollama model can be updated from its card, and a Mac is offered
  the builds made for it (KT-930). In Config › Agents › Local models › Ollama,
  the download block now lists the installed models, each with an **Update**
  button, except a model confirmed up to date: it asks Ollama to pull that exact
  tag again, with the download's own progress, Cancel and error messages. A badge beside it says **Update
  available** when the official Ollama library's copy of that tag is no longer
  the one you hold, **Up to date** when it is, and **Not checked** whenever Kronn
  could not confirm either way (the registry did not answer, or the model is not
  from the official library): never "up to date" on a guess. The comparison
  downloads nothing: the SHA-256 of the registry's manifest for the tag is the
  digest Ollama reports locally. Answers are cached for hours; Refresh, and a
  finished update, check again instead. The block's folded summary counts the
  updates waiting. On a Mac with Apple Silicon running Ollama 0.34 or later, the `-mlx`
  builds (`gemma4:12b-mlx`, `qwen3.8:27b-mlx`) come first in the suggestions,
  marked "Optimized for Mac". The backend decides (`mlx_capable` and `version` on
  `GET /api/ollama/health`, from the host and the server's own version), never
  the browser's user agent; on any other machine the list is unchanged.

### Changed

- The Ollama card's "Download a model" block is folded once a model is installed
  (KT-930). It was always open, and took the card over even with a full model
  library. It stays open for a first use, remembers your choice in this browser
  and, folded, says how many suggestions it holds and how many downloads are
  running; a download in flight stays visible, with its Cancel. The suggestions
  are now one portable model per hardware tier — `qwen3.5:4b`, `qwen3:8b`,
  `qwen3:30b-a3b` — in place of the former six, each with the repository run that
  shows it exists, and all five are checked against the Ollama library (see
  `docs/operations/ollama-local-models.md`). Each suggestion shows its real
  size, read off the library's manifest rather than typed into Kronn; when the
  library does not answer, no size is shown instead of an invented one. The
  card draws from your local Ollama first and fills in what the library says
  when it arrives (answers are cached for hours, and the only thing sent is a
  library tag name).

- No prompt Kronn sends to a model lists your other Kronn projects any more
  (KT-926). The steps of a Full or partial audit, and every discussion (the
  audit's briefing and validation included), orchestration round and workflow
  Agent step, used to carry the name and absolute path of every other project
  registered in Kronn; the audit steps also asked the agent to read their
  `docs/AGENTS.md` and to write a `## Suggested companion repos` section into
  the audited repository's own `docs/AGENTS.md`. That prompt goes to the model
  provider, which may be a remote service with no relation to those projects,
  and the section landed in a versioned file; for an HTTP agent, whose file
  tools cannot leave the project since KT-924, the instruction to read another
  repository contradicted the tools. The only other repositories a prompt names
  are the ones you linked to the project yourself ("Linked repos" on the
  project): every audit step names them inline, and discussions and workflow
  Agent steps keep reading them from the project's `docs/linked-repos.md`, as
  before. Nothing else about the audit changes: it still describes the
  repository from what the repository says. A `## Suggested companion repos`
  section an earlier audit left in a `docs/AGENTS.md` is not removed by this
  change.
- The Plugins sidebar no longer carries a per-project tree, so each plugin is
  listed once (a global plugin used to appear once per project, and again
  under "No project"). It is a flat list: Favorites and "Recently tested"
  (both collapsible, and taken out of the full list) then "All plugins", each
  row keeping its scope chips ("All projects · 3 projects"). Project, Health
  (error / to check / ready) and Local sync (available in local CLIs / not
  synced) join Type in the filter panel; the filter icon stays lit while one is
  set, and "Clear filters" resets them all. The Project filter drives the
  overview panel, whose summary and "Test all" / "Test the project" button
  follow it. A plugin's scope is now edited only from the Access tab of its
  sheet.
- The global Automation page's search, Filter and Sort are back in the sidebar,
  under the title, as on Plugins and Discussions. The Filter panel unfolds
  under the search with a full-width select each for Type (with counts), State
  and Project, and "Clear filters"; the icon stays lit while one is set. The
  filter bar KT-904 had put above the list is removed, and `/` still reaches
  the search.
- The Automation sidebar is regrouped and lighter (KT-916). Under the search, a
  "Group by" control — Type, Project or None, Type by default and remembered in
  this browser — splits the list into collapsible groups (a coloured dot, the
  name and the count), or keeps it flat. The panel of three selects and the
  separate Sort button are gone: the filters are chips on a line that wraps —
  a type chip that reads "All" until you pick one (its list shows each type with
  its count), "Pinned", "Active" and "Recent" toggles that stack with it and with
  the search, and "+ Project", which turns into a removable `project ×` chip.
  "Clear filters" appears only while a chip is set. Rows are 44 px: name, then
  the trigger, the step count and the state of the last run for a workflow; the
  pin and the ⋯ menu still show on hover. The Favorites and Recent sections are
  replaced by the "Pinned" and "Recent" chips ("Recent" keeps the last 20
  automations you opened, latest first). The order is set from the header's ⋯
  menu: name, last modified, last opened, and reverse. A skill still sits under
  every project that lists it. The type list is now in the order Workflows,
  Quick Prompts, Quick APIs, Quick Execs, Skills, and the "Inactive" state
  filter is gone (an inactive workflow is shown by its ○ icon; "Active" hides
  it).
- `not_published`, `repository_modified` and `kronn_modified` are now
  `kronn_only`, `repository_newer` and `kronn_newer`; a resource that exists
  only in the repository is `repository_only`. A resource that exists on both
  sides but was never aligned is `up_to_date` when identical and `conflict`
  otherwise. `approval_required` is a status of its own, and only for the
  kinds Kronn can execute.
- Importing a repository resource no longer silently replaces a Kronn copy
  that holds edits the repository lacks: the import is refused unless the
  request sets `overwrite_kronn_changes` (the "keep repository version" choice
  on a conflict does).
- The lock records `N2` rather than `N0` for a workflow or a Quick API, and
  `N1` for prompts, skills and Quick Execs.
- The router skill `.agents/skills/kronn/SKILL.md`, written on every
  publication, now briefs an agent that opens the repository without Kronn:
  what `kronn/` is (index, lock, levels N0/N1/N2), how to run a Quick Prompt, a
  Quick Exec (secrets passed by name through the environment, never written)
  and a skill by hand, what only Kronn can run (workflows, Quick APIs, living
  artifacts), why and where to install Kronn (links from the README) and to
  read a Quick Exec before running it. The file carries its model version
  (`metadata.version`); a copy a human edited is still refused, not replaced.
- Claude Code sessions launched by Kronn no longer load the workstation's
  automatic memory (`MEMORY.md`): `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` is set for
  workflow Agent steps, `task_exec` principals and workers, about 8.8k fewer
  tokens on the first call and on every re-read. A native discussion turn is off
  by default too; start the backend with `KRONN_CLAUDE_AUTO_MEMORY=1` to keep it
  there. See [Claude auto-memory](docs/operations/claude-auto-memory.md).
- The repository resources of a project are masked ahead of the first listing
  instead of on it. A background task renders the workflows, Quick Prompts,
  Quick APIs, Quick Execs and Artifacts of every project a couple of seconds
  after the backend starts, and again once they have stopped being edited
  (from the API, an agent tool, an import or a workflow run alike, with a
  debounce), filling the memory Kronn already keeps for that (capped at
  48 MiB, and the task stops when it is nearly full). It never delays the
  startup or a request — it reads the database in short slices, masks off its
  lock, breathes between two resources and stops with the backend — so the
  first listing after a restart reads from memory like the following ones.
  Reads copy less: a rendering shares its document and files with that memory,
  the workflow references, the approval fingerprint and the field diff no longer
  copy the JSON they read, each resource and each repository file is read once
  per listing, and only the project's own rows are read. Nothing changes in the
  answers, byte for byte, nor in what is masked (KT-915).

### Fixed

- Testing a LiteLLM connection no longer fails with "Could not persist the
  saved connection's model catalog" (KT-939). A LiteLLM proxy lists a model once
  per deployment; the second copy broke the catalogue's unique key and the whole
  refresh rolled back, leaving the model pickers empty. A model listed twice is
  now stored once.

- A Full audit in which a step fails no longer invalidates the whole run.
  Until now one failed step — often for an outside reason, such as the Mac going
  to sleep — left the run Interrupted with no validation discussion, even when
  the other fifteen documents existed. The run still ends Interrupted (only a
  complete run can be validated and earn the Validated badge), but it now gets a
  validation discussion for the steps that succeeded, which names the steps to
  redo and tells the agent not to validate their documents — unless the step
  that failed is the one that writes `docs/AGENTS.md`, the entry point every
  other document hangs off: without it there is nothing to validate, so the run
  stays Interrupted with no discussion and the Resume button still names that
  step to redo. The documentary check that refuses a run with an invented link
  still applies. The progress is now the exact number of steps that succeeded,
  the ones after a failure included — a run that lost step 5 of 16 reads 15/16,
  no longer 4 or 16. The Resume button names the steps it will run ("Resume —
  redo step(s) 5"), and a resume runs only those: a resumed run now records the
  steps it carried over, so resuming it again no longer replays the whole
  chain, and a step that was started but never finished is no longer mistaken
  for a successful one (KT-931).

- The project card follows an audit it did not launch, or one still running
  after a page reload (KT-994). The card used to miss it: the launch button
  stayed active (a second audit could start over the first), the running step
  showed as interrupted and the agent panel stayed editable. During an audit
  the panel now shows who runs it (agent, tier, model) and cannot change, the
  button reads "Audit running…", and the briefing waits for the end. An HTTP
  agent's activity shows as it happens (`read_file · package.json (12)`), as a
  CLI's did. Each step shows its tokens. A partial audit marks the step it
  runs, keeps the other results and counts once finished.

- A native Ollama audit can write its documents again (KT-967). Five Kronn
  defects kept a local model from producing an audit step. The main one: the
  output budget (`num_predict`) was sized once, on the small first-turn
  window (about 1,900 tokens), and never followed the window as it grew, so a
  `write_file` holding a whole document was cut off and, since Ollama only
  emits a completed tool call, nothing was written. The model seemed to
  announce its write and stop. The output budget now follows the window. In
  an audit, forced convergence also keeps the write tools, a repeated read
  stays refused without withdrawing `read_file`, the last eight rounds (or
  the last quarter of the context) keep only the writers, and a step that
  wrote its document is no longer failed for reaching the round ceiling. On
  the same audit step, recorded raw, qwen3.8:27b-mlx and qwen3.6:35b-mlx both
  went from failing to writing a complete, cited document.
  A full audit on qwen3.6:35b-mlx then completed 7 of its 16 steps. Most of
  the failures traced back to Kronn, and these are fixed:
  - Two tool calls that Ollama sent in parallel were merged into one call with
    no arguments. The model got "missing required field `path`" and copied the
    empty call from its own history. Each call now keeps its own arguments.
  - Filling `docs/AGENTS.md`, the model also wrote four later steps' documents,
    and those steps found nothing to rewrite. An audit step now writes only its
    own document (the consolidation step reviews them all).
  - The date reached the model through three exact phrasings, and step 8
    matched none, so its TD files were dated 2025. Every step prompt now
    states today's date, and a `{{DATE}}` left alone in a finished document is
    filled by Kronn.
  - Diagnostics now say what to fix: a forgotten field is named instead of the
    file being called an untouched template, a cited directory (with or
    without a line) is reported as a directory rather than a missing path, a
    comment written inside a citation marker is told to move outside it, and
    a link written from the repository root names the link to write from the
    document's folder.
  - Between two attempts Kronn rewrote a step's document itself, so the
    receipt the model held went stale, and the final write window had already
    withdrawn `read_file`: the model could no longer write its own document.
    A step now rewrites its own document without a receipt; every other file
    keeps the requirement.
  - The security step looked for keys by extension only (`*.pem`, `id_rsa*`).
    It now also treats the file beside an `X.pub`, and anything under `.ssh/`,
    as a key: committed keys were found in 0 of 3 runs before, 3 of 3 after.
    During an audit, `read_file` refuses a file holding a private key, so the
    key never reaches the model, which on a hosted model would mean leaving the
    machine.

- Codex, launched by Kronn in a discussion or a room, can use Kronn's own
  tools again (KT-953). Kronn starts Codex non-interactively, so a tool left on
  "ask" was refused ("MCP tool call requires approval, but approval policy is
  never"): Codex could not read a message, post one, attach a file, generate a
  media or create a task — even with full access. Kronn now approves its own
  tools, and only them, in the configuration it hands Codex at launch (so it
  no longer depends on the sync of `~/.codex/config.toml`) and in the Kronn
  entry of that file; your other MCP servers keep Codex's own setting. Claude Code and Copilot CLI
  without full access hit the same wall for a fresh install: they now get
  Kronn's tools, and nothing else, explicitly allowed (Claude:
  `--allowedTools=mcp__kronn-internal`; Copilot: `--allow-tool=kronn-internal`).
  Full access and task workers keep their current permissions.
  Native MCP posts no longer start an extra answer from their own discussion
  agent merely because the CLI supplies a runtime session id. Joined peers
  and explicitly addressed responders keep their routing.
  Native ACP sessions also receive the owned Kronn bridge without a project;
  the project-server filter no longer removes it. Its scoped tool permissions
  cover reads and writes without requiring full access.

- Native tool history now keeps reported names, redacted argument excerpts
  and observed completion/error status, correlating updates by call id.
  Repeated calls remain distinct; missing metadata is shown as unknown.
  Command output, MCP results and patch bodies are not copied into the trace
  (KT-953).

- A link an agent writes to a file on its machine now leads somewhere
  (KT-954). Agents often hand over a file by its path
  (`[the GIF](/private/var/…/loop.gif)`); the link resolved against Kronn's own
  address and opened nothing. When the agent's message is saved, Kronn now
  attaches the files it links that sit in the agent's own folder or in the
  project. Known credential filenames and directories (including `.env`,
  private keys and `.ssh`) and Kronn's data directory are excluded; each
  message can add at most eight files and read 64 MiB in total. Persisted links
  identify the exact attachment, including when two files share a name. Images,
  videos and text open in the attachment viewer; other files download under
  their original names. Unavailable paths show a reason instead of a dead
  link. Project-relative paths open the project viewer at the requested line;
  attached Markdown images render inline. A discussion without a project runs its agent in its own
  folder (`~/.kronn/discussions/<id>`) instead of the system's shared temp
  folder. If that folder cannot be prepared, the turn stops with a visible
  error instead of falling back to shared temporary files. This working
  directory and the attachment filters do not sandbox
  the CLI's filesystem access.

- HTTP audits can write more than twelve findings and their index within a
  dedicated bounded budget. A missing artifact after a tool ceiling now keeps
  that cause in its persisted warning (KT-951). Full audits correct verifiable
  bundled citations, retain originals, and retry remaining documentary blockers
  at most twice. Resume includes previously successful steps whose documents
  failed the final gate; invented paths and invalid lines still prevent
  validation, and human sections remain protected (KT-952).

- An HTTP audit interrupted by a provider error now keeps that cause in the
  step recap alongside any missing-output warning. A 429 identifies a rate
  limit or exhausted quota and no longer suggests incompatible tools. Provider
  bodies stay out of this diagnostic and partial files are preserved. This
  failure does not automatically retry the request or replay tool effects
  (KT-955).

- A Full audit gives the model the exact error when its dimension-coverage
  table is incomplete, using the existing limit of two corrective attempts.
  Resume recomputes that feedback from the saved index. The table must still
  pass every check; provider failures do not trigger this correction, and
  earlier TD files and human-owned sections are preserved (KT-956).

- HTTP audits and discussions can read fresh repository content after writing
  it. Previously only orchestration workers invalidated cached observations;
  an audit could receive a pre-edit file or lose its reader as a repeated call.
  Successful mutations now invalidate those observations and restore readers
  withdrawn for repetition, while call ceilings, error circuits and cached
  write effects remain enforced. HTTP token counters also reach the audit's
  per-step telemetry, summing each provider response once and preserving
  unknown usage instead of showing zero (KT-948).

- Images attached to a discussion now reach HTTP vision models as image input:
  OpenAI-compatible connections receive data-URL parts and Ollama receives
  native image arrays. The model is explicitly told when an image cannot be
  seen (unknown capability, unreadable file, unsupported format or request
  limit), instead of receiving only a path and guessing its content. The model
  catalogue exposes an independent Vision capability and imports image-input
  declarations separately from image generation. Large uploads are downscaled
  for transmission without changing the original. A discussion's own attached
  files are readable outside the workspace, while attachment edits and reads
  of unrelated outside files remain refused (KT-946).

- Agent instruction templates now defer language and test requirements to the
  project's parameters. New adapter files no longer copy an English language
  default, and re-auditing an existing managed block removes its unconditional
  English rule while preserving user content. The testing checklist follows
  the configured test policy, and links to Project parameters reach a real
  Markdown heading.

- The context a local Ollama model is given on a Mac is computed from the model
  and no longer cut from the installed RAM (KT-943). The ceiling used to be one
  slice per RAM size — 65,536 tokens on a 64 GB Mac, for every model — which is
  four times too prudent for `qwen3.8:27b-mlx`: only 16 of its 64 layers cache
  anything (the others keep a constant state), so a token costs its cache 64 KiB,
  and 262,144 tokens take 16 GB next to its 18.2 GB of weights, inside the
  roughly 48 GB macOS lets the GPU use. With nothing configured, the ceiling is
  now the smaller of the model's own window and what the GPU budget
  (`iogpu.wired_limit_mb` when set, else 75 % of memory) leaves once the weights
  and a safety margin (10 %, at least 2 GB) are taken, over the cost of a token
  of cache. That cost comes from the model itself — `/api/show` for a GGUF
  model, the `config.json` in Ollama's store for an MLX one, whose `/api/show`
  does not say — counting only the layers with full attention, and follows
  `OLLAMA_KV_CACHE_TYPE` (q8_0, q4_0) when the server sets it. That model now
  runs at its whole 262,144 on 64 GB, about 76,000 on 32 GB, and a dense 70B at
  about 19,000 on 64 GB. When anything is missing — the attention shape, the
  weights, a cache type Kronn cannot price, a model with a sliding window — the
  RAM slice applies exactly as before, and so does it off Apple Silicon. A
  Settings override and `KRONN_OLLAMA_NUM_CTX_CAP` still win, and the ceiling of
  a native MLX task worker stays at 32,768. The model list says where the figure
  comes from — "model estimate (weights + cache)" next to "model window",
  "machine memory" and the others — and a run held below the model's window
  names it in its notice. See
  [Ollama local models](docs/operations/ollama-local-models.md#the-ceiling-is-computed-per-model-kt-943).

- A native backend that relocates its data directory (`KRONN_DATA_DIR`) keeps
  listening on its configured host instead of `0.0.0.0` (KT-936). The variable
  used to stand in for "running in Docker", so such a backend was refused at
  boot by the LAN guard unless `KRONN_HOST=127.0.0.1` was set. Only a real
  container binds every interface now.

- The model list of Config shows the models OpenCode declares in a project
  too, and a launch runs the model you chose (KT-928). OpenCode builds its
  model list per working directory — its user-level config plus the
  `opencode.json` of that directory — while Kronn read it from a neutral one,
  so a provider only a project declares, a local Ollama for instance, was in
  every run of that project and in no selector. Kronn now also asks OpenCode
  from each registered project that has its own `opencode.json(c)` (or
  `.opencode/opencode.json(c)`; the files are only checked for, never read),
  and adds what comes back: a model only a project offers says so in its
  description, nothing is listed that OpenCode did not report, and a project
  that fails to answer is left out without failing the refresh. The reader also
  accepts a model list grouped by provider, which it used to drop whole. On
  the launch side, a model that the session does not list was silently
  replaced by OpenCode's own default while the discussion kept showing the one
  you picked; that launch is now refused with a message naming the model, and
  nothing is sent to the agent. A value of another session option (an effort
  level, a mode) can no longer be taken for a model.
- An audit on OpenCode reads a versioned environment template and no longer
  aborts when a read is refused (KT-927). OpenCode guards `*.env.*` by pattern
  and asks Kronn about a read without saying which file, so `.env.dist`, like
  `.env.example`, `.env.sample` and `.env.template`, was refused together with
  the real `.env` — and OpenCode ends its whole turn when a question is
  refused, so one template read aborted the step. Kronn now starts OpenCode
  with a read policy: the templates are readable, `.env`, `.env.local` and the
  other real environment files stay refused, and a refusal reaches the agent as
  the tool's answer instead of ending the turn. For the other ACP agents the
  broker refuses a read of a real secret file, or of a link to one, even with
  full access, and allows the templates. A project that already passes its own
  `OPENCODE_CONFIG_CONTENT` keeps it. See
  [auditing with an ACP agent](docs/architecture/audit-acp-agents.md).

- "Cancel" stops an audit on an ACP agent in seconds, and the partial audit can
  be cancelled at all (KT-927). `cancel-audit` killed the process it had on
  record, which for OpenCode — and for Claude and Codex through their adapters —
  is a lifeline that does no work: the audit believed the step over while the
  agent kept reading and writing, for about eight minutes on an OpenCode run. The
  stop now cancels the agent's ACP session and shuts its process down, with
  what it started. The partial audit had no cancellation whatsoever, for any
  agent: Cancel set a flag nothing read and the refresh ran to its end; it now
  stops between steps and during one, ends as Cancelled, and leaves the baseline
  alone.

- The steps of an audit on an ACP agent record the tokens they consumed
  (KT-927). Only Claude's own stream was read, so every step of OpenCode — or of
  any ACP agent, Claude and Codex included — counted 0 tokens, per step and per
  run. The recap now keeps input, output and cache read/written apart, as the
  runtime reports them (migration 205), for the Full and the partial audit. A
  runtime that reports nothing gives an unknown figure (`—`), never 0; the run
  total is unknown until a step has reported.
- A local model can now read a large API response in parts instead of losing it
  (KT-929). The `api_call` tool of the HTTP agents (Ollama, LiteLLM, NVIDIA)
  takes an `extract`, a JSONPath applied to the response, the same one workflows
  and the CLI bridge already took; only what it selects comes back. Until now a
  response too big for the window was shortened with the advice to "ask for a
  part you have not seen", and the tool had no way to ask. The model asked again
  for the same thing, was refused as a repeat, and the turn ended half done: a
  SpeedCurve answer with LCP, FCP, INP and TTFB missing. A shortened API result
  now says what it holds (its keys, with the paging shown as values, the length of
  its main list, the keys of one element down to its nested objects) and gives
  paths that select something on that very response, one of them for a whole
  element. A call that differs from the previous one only by its `extract` or a
  query parameter is a new question and runs; only an identical call is answered
  as a repeat. A result that was already an extract is told to select less
  instead. Measured on a 198 KB answer of a hundred runs in a 32K window: the
  first result is shortened to 8 KB, one targeted call returns the four metrics of
  all runs in 2 KB, and the conversation stays at 19,319 of 32,768 tokens. The
  window itself is unchanged: a discussion already runs `qwen3.8:27b-mlx` at
  65,536 tokens on a 64 GB Mac, and the 32K ceiling applies only to a native MLX
  worker (see `docs/operations/ollama-local-models.md`).

- A room that a CLI peer joined without being identified opens again
  (KT-925). The bridge joins such a peer as `Unknown`; the discussion detail,
  its default targets, native replies and room imports then failed with
  "unknown persisted agent type", and the room never rendered. That peer is
  now skipped as a typed target and logged. The bridge also recognises
  OpenCode, which was joining as `Unknown`, and a discussion whose first load
  fails says why, with a retry, instead of loading forever.

- Frontend dependencies: `brace-expansion` is pinned to 5.0.12, which fixes
  GHSA-qhr7-859c-m2p7 and two related advisories (denial of service by
  recursion, quadratic expansion). It is only used by the lint tooling.

- Backend tests: the workflow test that reads a running step's tool call no
  longer depends on a Claude CLI being installed on the machine. The agent
  preflight now accepts an agent served by a test ACP route, so the test
  passes on a CI runner without `claude` on the PATH. Nothing changes outside
  tests.

- The Automation sidebar folds every group, the first one included (KT-921).
  The group that held the open automation was forced open, and the first group
  (Workflows) almost always does: clicking its header did nothing. A group now
  folds and unfolds like the others, whatever it holds, by type or by project;
  the open automation stays open in the main column. A search or a multiple
  selection still lays the groups open. With "None" there is no group, hence
  nothing to fold.

- The Skills type lists the skills a project actually uses, wherever they come
  from (KT-921). A native skill of a repository that you "used in Kronn" from
  the AI & automation tab (`block-migration` in `.agents/skills/`, say) had no
  place on the Automation page, which only read the Kronn catalog and the
  projects' default skills. It now sits under its project, with where it lives
  ("Repository · .agents/skills"), and its sheet shows the `SKILL.md` read from
  the repository — masked and rendered as safe Markdown like any repository
  file, and read only for a skill the project really uses. So does a skill Kronn
  published into a repository (`kronn.lock`). Such a skill is edited in its
  repository: the sheet offers neither the Config link nor a delete. New routes:
  `GET /api/projects/used-skills` (all projects at once, two small reads each)
  and `GET /api/projects/:id/repository-resources/skills/content`.

- The Automation list no longer shows the whole skills catalog (KT-921). In
  "All" and in Skills, only the skills used by at least one project are listed
  — attached, referenced from a repository, or published into it. The others
  wait at the bottom, folded, under "Voir les skills disponibles (N)", in the
  three groupings. A search still finds them and lays the section open; the
  type and project counters and the library total count what the list shows.

- The skills of a discussion are grouped, searchable, and include the ones a
  repository holds (KT-923). The skill picker of "New discussion" and of a
  discussion's settings listed the whole catalog flat, without the native
  skills of the project's repository (`block-migration` in `.agents/skills/`
  could not be picked). Both now share one picker with the Automation page's
  logic: "Utilisés par ce projet" first (attached, referenced from the
  repository or published, with "Dépôt · .agents/skills" for a repository
  skill), then the skills already ticked, then the rest of the catalog by
  category behind "Voir les skills disponibles (N)". A search on the name or
  the description lays out what matches; a discussion with no project shows the
  catalog by category. Nothing is fixed in the front: the lists are read from
  `GET /api/skills` and `GET /api/projects/used-skills` each time the picker
  opens, the section follows the project chosen in the form at once, and a skill
  used, published or attached since — or gone from the repository — shows or
  disappears on the next opening, without reloading the page. A ticked skill
  that no longer exists stays visible, flagged, so it can be removed. A
  repository skill is stored as `repository:<project>:<slug>` in the
  discussion's `skill_ids`, and the agent really receives it: at each send the
  backend reads its `SKILL.md` from the project's repository, for a path the
  project uses only, never through a symbolic link or out of the repository,
  masked like any repository text and cut at 64 KiB, then injects it in a
  "Repository Skills" block of the prompt (compact for the agents that take
  compact skills). A skill it cannot load — another project's, no longer used,
  file gone — is said at the head of the reply, never dropped silently. A
  `SKILL.md` edited between two messages is read again by the second. Only the
  `SKILL.md` is injected; skills with extra files stay KT-919.

- The AI & automation tab of a project with many automations no longer takes
  seconds to load (about 5.2 s in front_euronews, the same on every read).
  Each read rendered every automation and masked its secrets again, and built
  the diffs of every differing one. The listing now carries no diff, only the
  status; the diffs come from `.../repository-resources/comparison` when
  the Compare sheet opens. The masked rendering of a resource is kept in memory
  under a fingerprint of the content it was made from, so the same content is
  never masked twice, and text that cannot hold a secret no longer goes through
  the masking regexes (a check on the literal every match must contain, with a
  property test that the output is unchanged). Masking is otherwise the same:
  the same patterns, the same non-leak tests. The links between resources
  (`uses`, `used_by`, the "N linked" badge, the transfer announcements) stay in
  the listing and do not depend on the diffs: on 120 resources, 40 of them
  workflows of 15 steps, they cost about 2 ms when nothing is published and
  about 9 ms when every resource has its repository file, out of about 100 ms.
- An approved delivery the integration sends back (a validation that goes red, a
  merge conflict) no longer stays in `ChangesRequested` with nobody working it
  and nobody told. The send-back now re-activates the worker with the failing
  command, its exit code and its output (a joined CLI is re-offered the next
  attempt, a native worker is redispatched) and posts a notice, with the same
  evidence, to the principal that approved. The principal can also run the
  integration again on the same approved delivery with `task_exec_resume` when
  the failure did not come from it (a flaky test, the environment): no new
  delivery, the validations already green for that candidate are not re-run, and
  it is refused once the worker committed or delivered again.
- `task_exec_launch` no longer accepts a validation Quick Exec can never run
  (`cd frontend && npx tsc -b`, `CARGO_TARGET_DIR=… cargo test`, a pipe, a
  binary off the allowlist). It was accepted, the worker delivered, the review
  approved, and the integration then refused the command (`` `cd` is not in the
  Quick Exec allowlist ``) and sent an approved task back to `ChangesRequested`.
  The launch, the campaign policy and the preflight now refuse it up front, with
  the form that runs.
- The brief of a worker with a shell no longer tells it to "run the validations".
  It runs the targeted tests; the long validations the principal persisted are
  played by Kronn at integration, and the worker commits and delivers in the same
  turn without waiting on a background command. A full `cargo test` started in the
  background outlived the 600 s shell limit, the worker handed the turn back to
  wait for it and ended without delivering (`worker_completed_without_delivery`,
  twice on KT-847).
- The merge commit Kronn creates when it integrates a task branch now carries
  the `Signed-off-by` of the configured git identity, so a repository that
  enforces the DCO no longer turns the release PR red on it.
- `task_exec_commit` (and `git_commit` for native workers) can finish a merge in
  progress: when a worker integrates the target branch into its own, Kronn
  commits the merge with both parents and the sign-off instead of failing on
  git's "cannot do a partial commit during a merge". It refuses, without
  touching the merge state, while a conflict is unresolved or a path outside the
  merge and `files` is staged. The worker brief now says how to integrate the
  target branch and never to erase `MERGE_HEAD`: a single-parent commit made
  the target's files look added on both sides at integration.
- One exhausted provider quota that escalated several executions no longer
  keeps each of them from going back to that provider: reassigning one of them
  to the same provider now counts as the human "the quota is back" signal for
  the whole outage and re-arms it, so the others stop blocking whichever is
  reassigned first. A new real quota failure blocks the provider again.
- When the provider's refusal announces a reset time (Claude Code's
  `resets 4:20pm (Europe/Paris)`), Settings → Agents now shows "Rearmable at
  16:20" beside the re-arm button. It is a hint only; nothing re-arms
  automatically. See [Provider quota re-arm](docs/operations/provider-quota-rearm.md).
- The cost shown for an agent reply is no longer a guess. It was the reported
  token total split 60/40 into input and output at one fixed rate per agent, so
  a Codex run that was 98.6% cache reads (25.2M tokens, about $13.5 at API
  rates) showed $111. The cost is now computed from the four counters the
  runtime reports (input not served from cache, cache reads, cache writes,
  output) at the rates of the model that served the reply, and Codex's
  `cached_input_tokens` is now read. When the counters or the model's rate are
  missing, the cost stays unknown, and the execution card says why (a total
  only, a model with no confirmed rate). Rates cover the GPT-4.1 to GPT-5.6 and
  Claude 3.5 to Fable 5.1 families, checked against LiteLLM's price list; a
  model outside them is unpriced until its rate is confirmed. Replies
  already stored keep their earlier cost.
- The in-app token total no longer hides the cache. The discussion header and
  the execution card show the real input, the cache reads and the output apart
  once every reply reported them, and flag a split that covers only some
  replies. The `session_budget` signal reports the four traffic counters and
  the cache share beside its `traffic_tokens` axis; the axis itself still counts
  cache reads, as it was calibrated to.
- A model that goes silent now fails the run instead of freezing it, and
  stopping a run frees the model (KT-932). On 30/09 a laptop slept in the middle
  of a generation: Ollama was idle when it woke, but OpenCode kept an open,
  mute connection to it and the step stayed frozen until the process was killed
  by hand — the 600 s limit on event streams only covers what Kronn sends to the
  browser. ACP agents and the native HTTP agents (Ollama, LiteLLM, NVIDIA,
  external API) now carry an inactivity watchdog on the model's own output.
  Progress restarts it — every chunk of an HTTP stream, every frame of an ACP
  agent, a reasoning chunk that shows nothing included — so a slow model that
  keeps talking is never cut; a silence of the full delay fails the run with a
  message saying what stopped, for how long, how far it had got and what Kronn
  did about it, and cancels the generation. The delay is the discussion's
  **Agent inactivity timeout** (Config › Server) or the step's
  `stall_timeout_secs`, and never less than 15 minutes in a discussion, so the
  first token of a large model loaded cold gets through; anywhere else it is 15
  minutes. A stalled step is recognised as a stall by its `on_timeout` routing,
  as before. A request that is not streamed (constrained JSON) has nothing to
  watch while it runs and keeps only its own timeout. Stopping an agent — Stop,
  a cancel, a kill — closes its connection to the model, which is what makes
  Ollama stop generating instead of finishing for nobody while the next request
  queues behind it: the HTTP stream is dropped, and an ACP agent is cancelled
  and shut down with its whole process group. Both are covered by tests against
  a simulated Ollama that watches the connection. A long but silent ACP tool
  call — OpenCode running `cargo test` or a build — is not mistaken for a dead
  model: while a tool call is open the watchdog measures silence against a
  bound of its own, eight times the model's delay, and hands the clock back to
  the model the instant a terminal update closes the call; a tool call that
  never closes is still cut, by its own bound, with a reason naming the tool.
  See [Agent timeouts](docs/architecture/overview.md).
- A tool call the model wrote badly no longer ends an Ollama turn, and the
  failure no longer blames the context window (KT-942). Ollama reads the tool
  call itself — Qwen writes it as XML — and when the text is malformed it ends
  the stream with an error such as `XML syntax error on line 13: element
  <parameter> closed by </function>`; Kronn took that for fatal and stopped the
  turn, with the note about the context window (« Kronn is running it at 65536 —
  raise it with `KRONN_OLLAMA_NUM_CTX_CAP` ») ahead of it, pointing at a setting
  that had nothing to do with it. The same request is now sent again, at most
  twice: generation is stochastic and a second try usually yields a valid call.
  Nothing is added to the prompt. Nothing ran for the refused call, so the
  replay is safe after earlier tool rounds too — the tools already run are not
  run again — and each replay is written to the run's retry trace. If the model
  keeps writing an unreadable call, the turn still fails, visibly, and the
  message now says the model produced an unreadable tool call instead of
  leading with the context note. Covered by tests against a simulated Ollama
  that refuses the call once, after a tool round, and every time.

- A LiteLLM model the proxy lists but cannot serve is now caught by the test,
  marked in the catalogue, and explained in the discussion (KT-941). On 01/10
  the proxy listed 96 models and Kronn showed all 96 as available, yet every
  `vertex_ai/…` one answered a 404 ("Publisher model … was not found or your
  project does not have access") and one a 401 ("Not allowed … due to tags
  configuration"): a connection whose default was `vertex_ai/claude-sonnet-5`
  failed every turn, and the error was the proxy's raw, nested and escaped
  JSON, which named no model and said nothing to do. **Test** now sends a
  one-token call to the model chosen for each tier — economy, default,
  reasoning — and shows, under each selector and on the connection card,
  whether that model answers or is not found, refused by the proxy, failing or
  too slow. The connection stays usable when one tier fails: the pickers keep
  the catalogue so another model can be chosen. A bare 401 on every model is
  still reported as a rejected key, not as three refused models, and neither
  the upstream body nor the key is ever in the result. A model that answers a
  real call with a 404, or a 401/403 naming the proxy's own allow-list, is
  marked unavailable in the model catalogue with its reason (not found, access
  denied) and its HTTP code — whether it was found by Test or by a turn that
  failed — and the selectors show it as unavailable with that reason. Being
  listed again does not clear it; a successful call does (the next Test of that
  model, or Retry on the LiteLLM card). A flagged model is refused up front,
  naming it, instead of re-sending a call that cannot succeed, and Kronn never
  runs another model in its place. The discussion now reads "Model “X” is not
  accessible through this proxy (HTTP 404: not found or access denied). Choose
  another model in Config › Agents › LiteLLM." (in the discussion's language),
  with the proxy's raw answer under *Technical details*. That sentence had never
  appeared for a real error: Kronn read the status of `LiteLLM error 404: …` as
  `404:` and found none, so every provider error skipped the model diagnostics
  and surfaced as the raw body; the status is now read correctly, for every HTTP
  agent.

- Releases carry their desktop installers again (KT-970). 0.12.0 to 0.14.1
  were published with no asset: the build workflow only ran on `v*` tags,
  while this repository tags `0.14.1`, so the update banner led desktop users
  to empty release pages. The workflow now runs on both forms, creates a
  draft release with the four installers, and fails unless the published
  release carries one for every platform. It can also attach installers to
  an existing release, built from that release's tag. In the desktop app, the
  update banner only offers a version whose installer exists for your
  platform. See `docs/operations/releasing.md`.

- A question card written correctly is no longer refused as "not valid JSON"
  (#223). Claude Code's text blocks were joined with no separator, so the next
  block — a turn a Stop hook relaunched, or the text after a tool call — was
  glued to the closing fence of a `kronn-question`, which then never closed.
  Each new block now starts on a line of its own, in the stored reply as in
  the live stream, in discussions, orchestration, workflows and the ACP
  adapter. A fence that is really left open is now named as such rather than
  as invalid JSON.

- A discussion no longer says its connection was interrupted while the agent
  is fine (#220). The "Realtime connection interrupted — reconnecting" banner
  showed on every WebSocket close, even one that reconnected within a second;
  it now waits until the reconnect has lasted three seconds. A reply's
  "Stream connection interrupted" note followed the local stream only; it now
  shows only when the server no longer reports the agent as running.

- The Projects page opens without waiting seconds (KT-987). Every refresh of
  the project list, on any page, asked each audited project for its drift —
  which hashes every source the audit mapped — and repeated it on the next
  refresh; the first open measured 3.9 s for one project. Drift is now asked
  only on the Projects page, once per project and audit state, and the
  backend reuses a result for a minute while the audit manifest is unchanged
  (an audit recomputes it at once). The project list, a project, its drift and
  audit details no longer wait behind database writes (a first list measured
  8.5 s while a workflow was writing), and the projects' docs are read in
  parallel.

- Kronn shows one loading screen from launch to ready, and no longer looks
  broken while its local service restarts (KT-986). Starting showed the Kronn
  loader, then unstyled black text, then "Cannot connect to backend" after
  about 10 s — a restart with migrations takes longer — and each tab then
  showed its own "Chargement…". Now the loader painted by the first frame stays
  until the first projects and discussions are in, and only its sentence
  changes ("Connecting to the local service…", then "The local service is
  starting…" with a Retry). It keeps trying instead of giving up, and every tab
  is fetched in the background so switching never waits. When the service
  stops while Kronn is open, a "Local service restarting — reconnecting…" pill
  appears at once (it used to wait for a 30 s poll, leaving Projects or
  Plugins empty with no explanation), and the loads that failed retry on their
  own when it answers again.

- The Discussions page no longer freezes on a large base (KT-983). Opening it
  listed batch runs by reading every workflow run's results, 20 to 40 s on a
  10 GB base, while the first opened discussion waited behind that request; it
  now reads an index (45 ms). A discussion's git status also waited on
  `gh pr view` every time: the pull-request link is now cached for two minutes
  and the lookup gives up after 4 s. Selecting a discussion re-rendered every
  card of the sidebar (about 200 ms per click); only the two cards that change
  now re-render.

- In Docker, agents can no longer control the host's Docker (KT-979). The
  host's Docker socket was mounted into the container, so any agent could use
  it to take over the machine. It is now off by default; the project Docker
  panel says how to turn it on (`KRONN_DOCKER_SOCKET=1` in the `.env` of
  Kronn's own folder, not the project's, then `make start`) and what that
  allows.

- In Docker, the backend's MCP sync no longer writes tokens into the CLIs'
  global configs (KT-965). `~/.codex/config.toml`, `~/.copilot/mcp-config.json`,
  `~/.gemini/settings.json` and `~/.claude.json` are mounted from your home and
  read by every agent in the container, whatever its project; an MCP that
  carries secrets is now left out of them, with a warning in the log. Natively
  nothing changes.

- Kronn's data directory is now readable by its owner only (KT-990, first
  step). On Linux it was `0755`, and the database, its WAL and its backups
  `0644`: any account on the machine could read discussions and the encrypted
  secrets. The directory is set to `0700` and every database file to `0600` at
  start-up. Agents run as your user, so this does not keep them out — that
  isolation is planned for 0.14.3.

- In Docker, a project's MCP file that already held a token gives it up
  (found by the 0.14.2 Docker run-through). An entry Kronn had not written
  itself, by hand or by the old `kronn mcp sync`, was kept as the user's, value
  included, even after Kronn imported that MCP and encrypted its token; and the
  backup taken before the rewrite held the token too. An entry that runs the
  server of an MCP Kronn manages is now rewritten with references, and the
  backups are written without the values.

- In Docker, Codex can run commands in a discussion again. Since 0.13.0 the
  adapter that discussions use kept Codex's own sandbox inside the container,
  where it cannot start ("bwrap: No permissions to create a new namespace"),
  so every command failed; there the container is the boundary, as it already
  was for the direct launch. This covers the later turns of a discussion too,
  which resume the Codex session and could not take the sandbox flag.

- In Docker, an agent in a discussion receives the values its project's MCP
  references stand for. Kronn recorded them under the project's host path,
  while discussion agents start under the container's `/host-home` mount of the
  same directory, so no reference resolved and the project's MCPs ran without
  their tokens.

- In Docker, a Claude Code discussion gets its project's MCPs. The adapter
  that discussions use dropped the whole project registry: it read a
  `${KRONN_MCP_…}` reference as a credential, and the synced `kronn-internal`
  entry, which points at the script path the host can see, as a foreign
  declaration. A pure reference now counts as what it is, and Kronn's own
  bridge replaces the synced copy.

- In Docker, a Claude Code discussion works past its first message. The
  volumes that keep Claude Code's sessions were created owned by root, so
  Claude Code, which runs with your UID, could not save a session and every
  later turn failed with "Claude Code reported an unsuccessful result". The
  image now creates them for your user, and `make start` repairs volumes
  created before.

- `kronn mcp sync` no longer writes tokens in clear into repositories while
  Kronn runs in Docker (KT-963). It generated each repository's `.mcp.json`
  from `~/.config/kronn/secrets.toml`, values included, where every agent of
  the container could read them. It now refuses there and points to Kronn's
  MCPs page, which keeps secrets encrypted; natively it still works and says
  that the tokens are in clear. `KRONN_ALLOW_PLAINTEXT_MCP=1` forces it.

- Kronn no longer writes agent files for CLIs this machine does not have, and
  a project can keep them out of its repository altogether (KT-971). The MCP
  sync wrote `.mcp.json`, `.kiro/`, `.gemini/`, `.vibe/`, `.ai/` and `.gitignore`
  lines into every repository, for every CLI, installed or not, with no way to
  turn it off. It now writes only the files of installed CLIs, and the project
  overview offers **Agent files: in the repository / outside the repository**.
  Outside, Kronn writes nothing in the repository and takes back what it had
  put there — your own MCP entries stay; Claude Code keeps its MCP servers,
  read from Kronn's data directory, while Kiro, Gemini and Vibe have none for
  that project.

- The first `make start` no longer leaves root-owned folders in your home
  (KT-960). Docker created every missing mount source itself, as root —
  `~/.codex`, `~/.config/rtk`, `~/.kiro`… — and turned a missing
  `~/.claude.json` into a directory. They are now created as you before the
  containers start, read from `docker-compose.yml` so the list never drifts.

- In Docker, agents can no longer read the host's credential files through
  the read-only home mount (KT-962). `make start` now covers each credential
  path that exists on the host (`~/.config/kronn`, `~/.ssh`, `~/.aws`,
  `~/.config/gh`, `~/.netrc`…) with an empty read-only mount in the generated
  `docker-compose.override.yml`. Restart with `make start` to apply it.

- In Docker, project MCP files no longer hold credential values (KT-964).
  Claude Code's `.mcp.json` now refers to them (`${KRONN_MCP_…}`), and Kronn
  gives the values only to an agent it starts in that project. Kiro, Gemini
  and Vibe get no credential-bearing MCP in Docker until they are proven to
  read such references. Natively, nothing changes.

- An HTTP agent's file search can no longer flood its own context (KT-959).
  On 02/10 a framing step asked `find_files` for `**/*` over a PHP repository
  with `vendor/`: the answer listed every one of up to 20 000 files, about
  880 000 tokens, which every later turn sent again until the request outgrew
  the model and the proxy failed it nine minutes in. `find_files` and recursive
  `list_files` still walk and count the whole tree, but return only the entries
  that fit in 64 KiB, with the number left out and a hint to narrow the
  pattern. Any other tool answer is cut at 320 KiB, saying so, before it enters
  the history.

- A LiteLLM model that answers 404 is now flagged wherever it is picked, and
  can no longer be saved on a tier by mistake (KT-957). On 02/10
  `vertex_ai/claude-opus-5` was set as the standard tier, failed every message
  with a 404, and stayed "available": a discussion with no connection chosen
  runs on the LiteLLM connection, but Kronn recorded the failure under the
  agent's own catalogue, where no model is listed, so nothing was ever flagged
  and the next message was sent anyway. The failure, the up-front refusal and
  the Retry button now use the connection's catalogue, and the model pickers of
  such a discussion read it too. **Test** on a LiteLLM connection now calls
  every chat model the proxy lists, once and with one token, in parallel and
  within about a minute, and records what each answered: on 02/10 the proxy
  took 9 s to refuse `vertex_ai/claude-fable-5@default` by its tag rules, so
  model calls now wait up to 20 s instead of 6 s, which had read that refusal
  as a timeout. Saving a LiteLLM connection calls each tier model it changes;
  a model the proxy refuses (not found, or not allowed for this key), or that
  the catalogue already knows it refuses when the call proves nothing, is kept
  only after an explicit confirmation that names it. A rejected key does not
  block the save, and an unchanged model is not called again. Lists mark a
  model ✅ once a real call (a test, a save, a reply) has answered, ❌ when the
  proxy refused it, with the reason on hover, and nothing when no call has
  proved either — being listed is not being served. A refused model cannot be
  picked for a tier, nor for the image or video slot. While the test calls
  the models, it shows how far it has got (`12/96`), so a minute of waiting
  never looks like a hang. What each model is for — chat, image or video
  generation, embedding — is now read from the proxy itself (`model_info.mode`,
  from `/model/info` or `/model_group/info`): image and video lists are
  filtered on it as for OpenRouter, and only chat models are called.

## [0.14.1] - 2026-09-26

### Added

- A Claude Code or Codex workflow Agent step can name its room: `room_id` (a
  template, for example `{{steps.jeton.data.room_id}}`) makes the step's agent
  a member and the principal of that discussion without a `kr-join` token. The runner hands the
  agent's bridge a capability through the environment on every launch and every
  resume, the bridge joins with it before the first Kronn tool, and the backend
  accepts it only while that very step of that run is running, for that room.
  The membership ends with the step, a replayed step takes over the executions
  its interrupted session steered (their deliveries wake it), and
  `task_exec_prepare` works again after `/resume` of an interrupted run.
- An isolated workflow run can start from a chosen commit instead of the
  project checkout's HEAD: `workspace_config.base_ref` (`origin/main`, a tag, a
  SHA; "Start from" in the workflow editor). A remote branch is fetched first,
  within 60 s and one fetch at a time per repository, so parallel foreach items
  do not refuse each other on the ref lock. A failed fetch or an unknown ref
  refuses the run with a message naming what to check, rather than starting
  from a stale copy or the main checkout. The worktree of a run left
  `Interrupted` and not resumed within `server.interrupted_worktree_ttl_days`
  (default 7, `0` = never) is reclaimed at boot, except a dirty or detached
  one; commits no base holds stay on a branch listed on the run.
- A workflow step can launch another workflow without waiting for it:
  `TriggerWorkflow` creates the child run through the same path as a manual
  launch (its variables mapped from templates, its snapshot prepared, its own
  concurrency limit and key applied), then continues at once. The child run
  records `triggered_by_run_id` and shows the launching workflow as its origin;
  the step keeps `child_run_id`. Loops between workflows are allowed (a chain of
  more than 20 runs launching one another is refused), and a refused launch
  ends the step with `TRIGGER_REFUSED`, which `on_result` can branch on. A
  phase no longer needs an Exec calling `POST …/trigger` and hanging up after
  `run_start`. Migration 194 adds the column to runs.
- `SubWorkflow` passes values to its child: `sub_workflow_variables` maps the
  child's launch variables to templates rendered in the parent run (in a
  foreach, `{{current_task.*}}` too), and the child's variable snapshot is
  prepared like a manual launch's. A child that declares variables used to fail
  for want of a snapshot. Mapped names must be declared by the child, and a
  parent variable resolved from the project environment or the Kronn context
  is never forwarded.
- A workflow's `concurrency_limit` can be counted per business object:
  `concurrency_key` (for example `"{{ticketKey}}"`) is rendered at each launch
  from the run's launch variables and stored on the run. Runs with different
  keys run side by side; a launch whose key is already at the limit is refused
  with `Concurrency limit reached for key …`, as the per-workflow limit already
  was. The key may read only `user_input` variables: one resolved from the
  project environment or the Kronn context is refused at save, since the key
  is stored in clear. Migration 193 adds the column to workflows and runs.
- A workflow run can carry a plain business label from its launch:
  `POST /api/workflows/{id}/trigger` accepts `state` beside `variables`, and
  `GET /api/workflows/{id}/runs?state_key=…&state_value=…` returns the runs
  holding that entry, newest first. The last run about a ticket now takes one
  call instead of reading the detail of every recent run.
- A Page action's `user_input` field can start from the clicked row's data:
  with a `<page.dataset…>` `source_ref`, Kronn resolves it server-side when the
  card opens and the reader edits it before launching, instead of retyping a
  debrief that is already in the dataset.
- `task_exec_status` can wait for an execution: `wait_for` lists statuses (for
  example `["AwaitingReview", "Done", "Blocked"]`) and the call returns as soon
  as the execution is in one of them, with `wait: {matched, timed_out,
  waited_ms}`, instead of the principal sleeping and re-reading. `timeout_secs`
  bounds it (60 s by default, 170 s at most).
- A workflow Agent step records the prompt-cache tokens Claude Code reports
  beside its input and output: `cached_prompt_tokens` (reads) and
  `cache_write_prompt_tokens` (writes), on the step result and on each attempt.
  `tokens_used` keeps counting uncached input plus output; an orchestrator step
  that declared 21 593 tokens had also read 1 554 330 cached tokens and written
  80 271. While an Agent step runs, its latest tool call (tool, target, time) is
  stored on the in-flight step result as `last_activity` and returned by
  `workflow_run_status` as `current_activity`, so every reader sees what the
  step is doing, not only the client that started the run.
- Workflow templates accept one explicit fallback, `{{path ?? "text"}}` (or
  `'text'`, taken verbatim). It renders the literal when the path is absent or
  JSON null, so a step reading a step that a `Goto` skipped runs instead of
  failing the run; a present empty value stays empty, and an absent reference
  without `??` still fails as before. Saving still refuses an unknown step name
  behind a fallback and now refuses a malformed one; the wizard no longer warns
  about a guarded reference to a later step. `{{run.id}}` gives the current
  run's id. See [the template grammar](docs/architecture/overview.md).
- `ApiCall` and `BatchApiCall` steps can fetch an image or another file through
  the API broker with `api_response: {"type": "Binary"}`: the credentials stay
  on the server and the step returns `{content_type, size, base64, data_uri}`
  instead of failing on JSON parsing. Only the declared media types are
  accepted (`image/*` by default, `*/*` refused), and a body over `max_bytes`
  (256 KiB by default, 2 MiB at most) fails the step rather than being
  truncated. Published as a `data:` URI, a Jira attachment thumbnail shows in a
  Page without opening its image policy to another domain. Steps without the
  option parse JSON as before.
  See [binary responses](docs/operations/deagent-apicall.md#binary-responses-images-and-other-files).
- A room's native agent can prepare and launch a task execution itself, as
  its principal, without a CLI joining the room. Kronn identifies it from the
  turn it is running, so only that room's agent is accepted.
- Workflow Agent results retain execution provenance for initial, repair,
  escalation and debate attempts, including model resolution, structured
  runtime model observations and format fallback. Compact agent/model badges
  follow the retained output; historical runs keep their existing metadata.
  See [workflow agent provenance](docs/operations/workflow-agent-provenance.md).
- A workflow run's Agent step lists every attempt in its details (role, agent,
  reported or resolved model, format fallback, duration, outcome) and marks the
  one whose output was kept. A step with no recorded model shows "unknown
  model" instead of today's step configuration.
- Later workflow steps can read an Agent step's provenance as
  `steps.<name>.provenance` (agent, model, connection, role, whether the output
  was retained), and `PublishPageData` can publish it as a typed value, so a Page
  names the agent behind its analysis instead of hard-coding it. A step without
  recorded provenance exposes none.
- Task execution usage for HTTP agents records the prompt tokens a provider
  served from its cache, when it reports them (`prompt_tokens_details.cached_tokens`
  or `cache_read_input_tokens`). Turns that do not report it stay unknown and are
  counted separately, so no cache rate is inferred for them.
- HTTP task execution usage also records the prompt tokens a provider wrote to
  its cache (`cache_creation_input_tokens`), per turn, per phase and in total.
  Kronn asks LiteLLM to mark Anthropic cache breakpoints on the system prompt
  and the last message of Claude requests, which brought the input cost of a
  replayed Sonnet task to about a quarter of its uncached price.
  `KRONN_LITELLM_PROMPT_CACHE=0` turns it off.
- Open 2–12 selected discussions in a separate mosaic tab, with Artifact-style
  layouts, plan progress, recent messages and saved response checkpoints.
  Each tile scrolls independently and links to its full discussion. The bounded
  read-only monitor shares one WebSocket and batches refreshes without marking
  discussions read or launching agents; a missing room does not block its peers.
  Selecting a tile opens a collapsible input bound to that discussion, with its
  mentions and draft; messages use the durable outbox route.
  [Monitoring limits and behavior](docs/operations/discussion-mosaic.md).
- The discussion asset carousel has a copy button. Text, JSON and log files are
  copied whole (up to 2 MiB), images as PNG, and videos only where the browser
  accepts that type; otherwise the button says why it is unavailable.
- Artifacts can be exported and imported as versioned JSON bundles containing
  current HTML, retained data and linked automation definitions. Import previews
  creation, reuse and conflicts, remaps references into a new Artifact and keeps
  new workflows disabled. Missing local configuration is shown before import;
  stale previews and failed imports leave no partial resources.
- Artifact import shows each new Quick Exec's command and arguments and
  requires explicit approval before creating it. New Quick APIs show their
  method and endpoint in the preview.
- HTML previews in discussion messages can become Artifacts with an editable
  title, unchanged HTML/CSS/JavaScript, and a link back to the source message.
  Repeated titles create distinct Artifacts without overwriting existing ones.

### Changed

- The Pages interface is now named Artifacts in all four languages, including
  workflow publishing and the mosaic. Existing URLs, identifiers, API routes
  and MCP tool names remain compatible.
- Workflow, Quick API, Quick Exec and Artifact exports replace literal credentials
  (authorization headers, secret query or body values, `--token`-style
  arguments) with a marker and list the masked fields in the file, never their
  values. `{{…}}` references are kept. A notice follows the download, and the
  import preview lists the masked fields before confirmation.

### Fixed

- A workflow whose Agent step uses LiteLLM starts when the LiteLLM proxy is
  declared in Settings (`agents.lite_llm.base_url`), even with no local
  `litellm` binary. The pre-flight check refused it as "not installed",
  although that proxy can run on another machine.
- The "RTK not installed" badge of an agent card reads at 4.5:1 in the matrix
  theme: its tinted background took the low-emphasis text to 4.41:1.
- Text typed the instant a discussion's input appears is kept and saved as
  its draft. The input bound its discussion only after the first paint, so
  such text was neither saved nor kept when the discussion finished loading.
- `scripts/check-app-icons.mjs` requires the desktop icons to be 8-bit RGBA,
  which Tauri needs to build the app, and compares them with a fresh render on
  exact pixels. A lossless re-encode no longer fails it; a resample or a
  retouch still does. The shipped icons are unchanged from 0.14.0.
- The boot purge of finished workflow runs no longer removes a worktree that a
  finished sub-workflow shares with its parent while that parent is still
  running, paused at a gate or resumable after an interruption.
- An arbitration card lets the reader take a checked option back, and offers
  a Comment action: the text reaches the agent that asked, marked as not a
  decision, and the question stays pending. Before, a checked radio button
  could not be unchecked and a written reply always settled the question.
- A workflow with `require_isolation` and a SubWorkflow foreach accepts a
  `concurrency_limit` above 1: each run owns its worktree, so two runs overlap
  while each foreach stays sequential. In such a fresh worktree the foreach no
  longer skips every item on `No such file or directory`: it creates the
  untracked `.kronn/` folder before writing `current_task.json`.
- Kronn no longer deletes a repository's own skills and agent files. At every
  startup the native sync removed any `.claude/skills`, `.agents/skills` or
  `.gemini/skills` folder (and any agent file) it had not just written, and
  appended a whole-folder ignore rule such as `.agents/` that cancelled the
  repository's `!.agents/skills/`. It now records what it writes in
  `.kronn/native-files.json` and only removes an unmodified, untracked file
  of its own; it ignores only what it wrote, and drops a whole-folder rule an
  earlier sync appended over a re-included folder.
- A kronn-action block removed from a Page's HTML is no longer listed among
  its actions after the next publication; its launches stay in the history.
- The dark themes pass WCAG AA: axe, plus a re-measure of the text it leaves
  undecided behind gradients, now finds no contrast failure on Projects,
  Discussions, Planning, Plugins, Workflows, Pages, Settings or an open action
  card in `dark`, `gotham` or `matrix`, where it found 23, 404 and 395. Gotham
  and matrix low-emphasis text (`--kr-text-muted` down to `--kr-text-ghost`) and
  a few status colours were lightened, keeping their hue, to at least 4.5:1;
  sakura and euronews ghost text reaches 3:1. Settings' debug switches no
  longer show a light-grey browser button, unavailable models are muted
  instead of faded, and agent names blend their brand colour with the text
  colour. Thirteen `:focus-visible` rules no longer hide the focus ring.
  `pnpm lint:theme` (also in CI) measures every theme and refuses undefined
  custom properties, white or black text pinned on a token fill (in a
  stylesheet or an inline `style={{ }}` object) and removed focus rings;
  `e2e/specs/a11y-dark-themes.spec.ts` scans the rendered screens. The Plugins
  page's scope tip no longer prints white text on the accent.
- The "▶ Launch" button of a native action card, the project git switcher's
  button and the current-branch marker no longer print white text on the
  accent: they use `--kr-text-on-accent`, which reads at 15.97:1 on the default
  lime, 13.58:1 on the gotham yellow and 15.38:1 on the matrix green, where
  white was 1.18, 1.43 and 1.37. The discussion weight panel and the prompt
  variable editor no longer open white in dark themes. The token guard now
  refuses any `var(--kr-*)` that `tokens.css` does not define, even behind a
  fallback, and white text pinned on an accent fill.
- A shell-less worker's edit to a PHP, Twig, SCSS/CSS, TS/JS or JSON file is
  refused before it reaches disk when it leaves an orphan delimiter or an
  unclosed Twig block, or when an `edit_lines` replacement shifts the
  indentation of the first or last line it replaces; the diagnostic sends the
  worker into its one strict correction, as a Rust parser error already did.
  Local models got bounded edits with indented edges wrong on every measured
  case. The prelocalized worker brief no longer carries the human-arbitration
  and parent-milestone sections.
- An `Exec` step's `---STATE:k=v---` and `---ARTIFACT:name---` markers are read
  from the command's raw stdout instead of the JSON-escaped copy in its
  envelope. A multi-line value or artifact now reaches later steps and the run
  state with real line breaks rather than literal `\n`, and quotes and
  backslashes are no longer escaped. Only stdout is read; there a `STATE` value
  may span lines up to its closing `---`.
- A Kronn action card opened from a Live Page no longer closes every 30 s
  when the Page refreshes, and keeps what was typed in it; it now follows its
  row when new data makes the Page redraw. Each row's state carries its launch
  id (`data-kronn-action-launch`), so a Page tells a new attempt from the
  previous one. A field's placeholder reads as an example (`e.g. ollama`)
  instead of passing for the value an empty field would send.
- An accepted delivery from a Claude Code worker names the model its runtime
  reported serving (from the `assistant` event of its stream) instead of
  "Model unknown". A requested model is shown only when none was reported, and
  reassigning the worker clears the previous one (migration 192).
- A restart in the middle of a task execution's integration no longer keeps the
  backend from answering while the interrupted validations are replayed, which
  could outlast the 300 health probes `kronn start-dev` waits for. Boot still
  reclassifies every interrupted execution, but replaying validations,
  rebuilding a candidate, applying and provisioning now start once the server
  listens, in the background and one execution at a time, each logged as it
  completes. A resume requested for an execution already being resumed is
  refused with `a resume of this execution is already running` instead of
  running twice.
- A principal following a task execution no longer reads 10 to 17 thousand
  characters per `task_exec_status` call. `view: "compact"` returns the status,
  attempt, review rounds, delivered `head_sha`, last error, the latest
  candidate's validations (command, exit code, duration) and `next_action` in
  under 1 000 characters. The full view stays the default, for reviews and
  diagnosis.
- `task_exec_reassign` is accepted from `AwaitingReview`. The pending delivery
  is rejected but kept in the attempt history, and the requested worker starts
  the next attempt on the same task, room and worktree, instead of the
  principal cancelling and relaunching the task.
- A worker's delivery now wakes the CLI principal waiting in the parent room.
  The review request, escalations, integration refusals, campaign pauses, the
  undelivered-worker notice and the terminal notice were addressed to the
  room's native agent, so the principal's `disc_wait_for_peer` withheld them
  and timed out. They now address the joined CLI that launched, reviewed,
  resumed or reassigned the execution while it remains in that room, and the
  room's agent otherwise.
- A prelocalized rework no longer edits the launch line numbers on moved
  content. When a delivery changed the file's line count, `request_changes`
  replayed the same range and a local worker deleted the neighbouring rule.
  Before a rework, a resume or a reassignment, Kronn now re-anchors the range
  on the lines around it and tells the worker the new range; if anything
  outside the range changed, it refuses with `stale range: …; relaunch with a
  new worker_scope`, naming the launch range and the observed change.
- A CLI task worker that runs `git commit` itself can no longer deliver a
  commit carrying an invented identity. At delivery, every `Signed-off-by`,
  `Co-Authored-By` or similar trailer in the delivered commits must name the
  repository's git identity, the one `git commit -s` signs with; otherwise the
  delivery is refused with the offending lines and how to fix them, before any
  review or integration. Approval checks it again for deliveries accepted
  earlier, and the CLI worker brief now asks for `git commit -s` and no
  hand-written trailer.
- A task execution no longer stays in `Applying` forever when another one lands
  on the same target branch during its validations: Kronn rebuilds the candidate
  on the new tip, validates it again and applies it (up to three times). Any
  other refusal to apply, or a target that keeps moving, parks it in `Blocked`
  with a reason code and a notice in the principal room; `task_exec_resume`
  then continues from the real target.
- A workflow with launch variables triggered from MCP (`workflow_trigger`) now
  runs. That launcher never prepared the encrypted variable snapshot the UI
  prepares, so the run died at start and stayed "Running" forever, counting
  against the workflow's concurrency limit. MCP now goes through the UI's
  launcher, a variable preflight failure is returned to the caller as with
  `qp_run`, and a run whose execution errors in the background (MCP, UI,
  schedule, tracker, resume) is marked Failed with the reason (KT-786).
- Two workflows of one project that run in its main checkout and start at the
  same moment (crons sharing a minute, for example) no longer make one of them
  fail at once with "Refusing to run in the main checkout". The later run now
  waits its turn, in arrival order, for up to 60 s
  (`KRONN_MAIN_TREE_WAIT_SECS`) and stops waiting if it is cancelled; past that
  delay the refusal names the run still holding the checkout. A workflow that
  never writes the checkout can declare it (`workspace_config.main_tree_read_only`,
  or "Does not write to the project checkout" in its advanced settings) and
  then runs without taking this lock (KT-787).
- A Live Page action whose workflow run was interrupted by a restart no longer
  stays "running" forever and blocks its row. The interruption now reaches
  the run's shared status, runs left in that state by earlier versions are
  repaired at startup, and a row whose last launch finished opens on a fresh
  launch, with the last result one click away.
- A Page opened in its own tab now keeps up with new data. It is read again
  every 30 s and when the tab comes back into view, and the new data is sent
  to the open page without reloading it, so scroll and open rows are kept.
- Opening a discussion no longer downloads every image it contains. An image
  thumbnail, which is the whole file, loads as it nears the screen: on a
  2,000-message room with 12 images, 9.5 MB instead of 15.6 MB at opening.
- An open discussion no longer re-downloads its whole transcript when an
  unrelated workflow or media run reports progress. Every refresh now asks for
  the detail only if it changed, and a burst of events collapses into one
  request. On a 2,000-message room with a workflow running: opening it went
  from 66 MB to 27 MB, and 20 s at rest from up to 27 MB to 1.9 MB.
- In `kronn start-dev`, a build that writes generated sources under a
  `target/` directory (another checkout's Cargo build, for example) no longer
  restarts the backend and cuts the agents it is running. The file watcher now
  ignores `target/`, and a watched build restarts the backend only when it
  actually changed the binary.
- The document exporter (`kronn-docs`) no longer outlives a backend that is
  killed outright (crash, SIGKILL, a hot-reload restart that times out). It
  now exits as soon as the backend's end of its stdin pipe closes, instead of
  piling up orphaned processes across restarts. Development setups that use
  the desktop bundle need `make docs-bundle` once to pick this up.
- `kronn start-dev` no longer moves `/opt/homebrew/bin` and `~/.cargo/bin`
  ahead of your own PATH; it adds them at the end, and only when missing. With a
  second, older agent CLI installed through Homebrew/npm, Kronn used to run that
  copy: an old Claude Code served `opus` as Opus 4.8 while the up-to-date CLI
  serves Opus 5.5. The Agents settings now warn when another copy of a CLI with
  a different version is on PATH, naming the path and version of each.
- A long discussion left open no longer freezes the page every few seconds.
  Unrelated refreshes (room links, the dashboard, background status) re-rendered
  every message; the transcript is now reused while nothing it shows changed.
  On a 2,000-message room at rest: from about 0.9 s pauses every 5 s to short
  ones (production build: 0.9 s of long tasks per 20 s down to 0.35 s).
- An approved task whose integration cannot start no longer sits silently in
  `Approved`. When the target branch is checked out in no worktree (or in
  several), or another precondition fails, the execution records why and the
  principal room gets a notice naming the fix, for example
  `git worktree add <path> <branch>`. The approval stays valid: once fixed,
  resuming the execution or approving again starts the integration.
- An execution interrupted while its merge was being applied can be resumed
  again. If the merge had already landed, even with more commits on the target
  since, resuming now closes it as done instead of staying `Interrupted`; if it
  had not, resuming replays the apply safely. The recovery action Kronn
  proposes is always one that resume accepts.
- The Automations list no longer waits on a scan of every workflow run to find
  each workflow's latest one. A `(workflow_id, started_at)` index answers it
  directly: on a 7 GB database, from 0.85–3.7 s to 19 ms.
- An open discussion no longer re-downloads its whole transcript every five
  seconds. The refresh sends the revision it holds and the server returns the
  detail only when it changed; on a 2,000-message room this removes about
  40 MB per minute of transfer while it sits idle.
- `disc_link` now reports whether the session it just bound is actually usable
  by `task_exec_prepare`/`task_exec_launch`, instead of a bare success that
  left the gap to surface later as an unexplained `rejoin_required`. The
  response now says so at link time and names the exact next call
  (`disc_invite_peer` then `disc_join`) when a rejoin is still needed (KT-737).
- A workflow Agent step run through ACP now records the token usage its agent
  reports instead of 0. When the agent reports none, the step's
  `tokens_used` is `null` (with `tokens_status: "not_measured"` in the MCP run
  status) and the run view shows "tokens unknown" rather than a zero. Run
  totals still add up only the measured steps (KT-735).
- When a Claude or Codex agent run through ACP fails, the error now includes
  the end of what the agent printed on stderr (for example an expired login)
  instead of only "exited with status 1". A prompt that cannot be delivered
  because the agent already quit reports the agent's exit status and stderr
  rather than "Broken pipe". The executed command line is logged at debug
  level with prompts and secret values left out (KT-666).
- A local linked repository whose path does not exist on this machine no longer
  makes the Claude task worker unavailable for the whole project on macOS: it is
  skipped with a warning. A linked repository or project that exists but cannot
  be read as a Git checkout still refuses the worker, and the refusal now names
  it instead of suggesting a reassignment. Saving linked repositories now
  rejects a local path that does not exist; remote URLs are unchanged (KT-741).
- Native ACP replies no longer include echoed user prompts, including Vibe's
  copy of Kronn's injected instructions. Only agent message chunks contribute
  answer text; tool and usage events remain separate (KT-729).
- Codex discussions with a project-synced internal MCP bridge no longer fail
  immediately during bootstrap: the adapter emits the reserved
  `kronn-internal` entry exactly once (KT-730).
- A draft typed in a discussion after sending, then left for another
  discussion, is no longer erased when the earlier message is acknowledged;
  only the sent text itself is cleared.
- Native backend hot reload uses the initial startup readiness budget instead
  of stopping a still-starting backend after roughly 30 seconds. Slow project
  MCP synchronization can finish before the HTTP listener becomes ready;
  diagnostics distinguish readiness timeout from an actual backend exit.
- Workflow HTTP agents recover once from an explicit unsupported structured
  output response by keeping the schema in the prompt and retaining the model,
  tools and local validation policy. A persistent notice records the fallback,
  including when repair or escalation replaces the answer. Generic 501 errors
  are no longer retried as transient failures, and tool-support advice appears
  only when the provider explicitly rejects tools. Invalid schemas, credentials
  and quota errors remain failures.
- Workflow document audits keep native Unix filename bytes instead of failing
  on non-UTF-8 names. Escaped diagnostic labels distinguish these files without
  changing their names, contents or index entries.
- Workflow document audits preserve preexisting and concurrent working-tree
  changes, staged content and untracked files. They compare pre-step content
  fingerprints instead of restoring every dirty document from HEAD or deleting
  it. Changed content with a credential signal now fails the step with a
  persistent, secret-free diagnostic; legitimate documents over 8 KiB are not
  rejected for their size. See the [audit and recovery notes](docs/operations/workflow-docs-audit.md).
- Commits made through a task worker drop `Signed-off-by`, `Co-authored-by` and
  similar identity trailers written by the model, keeping only the sign-off Kronn
  adds from the git configuration. The tool result lists what was removed. A
  worker could otherwise record an invented identity in the history.
- A room wait ended by another Kronn tool call is no longer silent. The bridge
  serves one call at a time, so a host that moved `disc_wait_for_peer` to the
  background stopped listening at its next call while the protocol said the
  wait remained active. That call's result now carries `wait_preempted`, the
  wait's own result says `interrupted`, and the protocol text asks for a re-arm.
- A failed workflow import rolls back every bundled resource, including Pages
  and Quick Prompts created before the error. Late validation or database
  failures no longer leave partial imports or activate an empty Pages library.
- A Live Page's 30-second auto-refresh, and switching to another Page, each
  fetch that Page's detail exactly once instead of twice (KT-736).
- Triggering a workflow from MCP with an argument its tool does not declare
  (for example `vars` instead of `variables`) now fails with an error naming
  the expected `variables` argument, instead of silently dropping the value
  and reporting an unrelated "variable is required" error (KT-738).

## [0.14.0] - 2026-09-23

### Added

- Text, JSON and log attachments open in the discussion carousel from their
  message or the Assets panel. The preview preserves source text, marks files
  truncated at 256 KiB, and keeps the original file available for download.
  Images, videos and text can be browsed together.
- Live Page action history keeps up to 1,000 finished launches per block,
  alongside active launches. New launches, declines and completions prune old
  snapshots atomically; their results and discussions remain available. An old
  pruned card cannot accidentally start another run.
- Agents can discover the versioned discussion action contract through
  `tool_manual({tool: "signals"})`. The native and MCP paths share the same
  schema for proposing existing Quick Prompts, Quick APIs, Quick Execs and
  workflows. Reading it and emitting a proposal leave execution to the human.
- HTTP discussion agents can list and read workflows, consult canonical step
  contracts, create disabled drafts, and patch disabled workflows. Drafts stay
  disabled for human review; omitted fields and project bindings survive edits.
  The MCP bridge and native tools share one step contract, including corrected
  Notify and JsonData examples. Validation includes one successful local-model
  three-step workflow and three failed attempts; it does not establish a general
  authoring success rate. The [retained observations](docs/research/native-workflow-2026-09-22.md)
  document both outcomes.
- The setup wizard shows the folders Kronn can actually reach, and several can
  be chosen at once. Somebody whose repositories live outside the home had only
  a text field, and a browser cannot supply an absolute path — it yields a
  relative name and nothing else. The server lists the home, whatever Docker
  mounted, the configured paths and every top-level directory that is not the
  system's, one level at a time, refusing anything outside them. Under Docker
  this also shows what the container ended up seeing, which is rarely what was
  pictured. The desktop build can open the system's own dialog instead; that
  integration was compiled and tested with doubles, but the system dialog was
  not opened manually during this validation. The server folder browser is the
  exercised path. And
  when the scan finds nothing, the screen names the paths it walked, so an
  empty result can be told apart from a mount that never happened.
- A ceiling that cuts an agent run is now said in the discussion: which one, its
  value, how many calls were refused and what they were after. The same message
  asks whether to go further — more calls or rounds for the agent's next turn, no
  call limit for the rest of the discussion, or keep the partial answer. Keeping
  it wakes nobody. A grant only moves a counter: repeated-call, same-answer,
  error-circuit and duration guards stay exactly as they were, and only a question
  Kronn itself recorded can raise a budget.
- A project declares once how its isolated worktrees are prepared
  (`Project.workspace.hooks`), and every workflow of the project that asks for
  isolation inherits it, overriding it field by field. A worktree is invisible
  to containers mounted on the main checkout, so a validation launched there
  reads the wrong code and passes — green, silent and wrong. The recipe that
  fixes that is the project's, and until now it had to be copied into every
  workflow that needed it. A project that declares nothing behaves exactly as
  before.
- An agent no longer has to name the connection a generation runs on. Omitted,
  Kronn uses the only one configured for that modality; when several can serve
  it, the request is refused with their names rather than a budget being
  chosen for you, and the answer says which connection was billed. Contract
  tests cover selection, ambiguity and idempotent retries. A launch with no
  identifier in the request is now backed by a
  [native check across seven models](docs/research/media-without-connection-id-2026-09-22.md):
  six models were checked on September 22 and the 35b on September 23; all
  created a pending job without asking for an id. The 2b model first
  corrected an invalid resolution. Workers were stopped; image generation
  itself and a before/after comparison are outside this check.
  The bridge also stopped pointing at `mcp_list` for that id, which never listed
  a connection.
- Progressive tool loading is available with `KRONN_TIERED_TOOLS=1`; full
  declarations remain the default. The native dispatcher now reaches
  `tools_load`, which previously failed with "unknown tool". Earlier simulated
  measurements missed that defect and their performance claims are withdrawn.
  A real-executor campaign across six local models succeeded in 35/42 scenarios
  with full declarations and 17/42 with progressive loading. Loading a family
  now reserves its complete JSON size, including separators.
- HTTP discussion agents can list, launch, create and update Quick Prompts.
  Launches retain the saved model and bindings; updates preserve omitted fields
  and the existing project scope. New prompts inherit the room's project unless
  explicitly created as general. Batch launch and deletion remain unavailable,
  and bounded task workers do not receive these tools. Seven native scenarios
  passed on both Sonnet through LiteLLM and Qwen through Ollama on the same final
  executable. Quick Prompt and media cells verify queued jobs, not execution of
  their children; [the report](docs/research/native-qp-litellm-ollama-2026-09-22.md)
  records input-token measurements and the single-observation limits.
- `disc_update`: an agent can rename the discussion it works in, pin it or
  archive it. It cannot change the agent that answers, the model tier, the
  connection, the project or the instructions bound to the room: those decide
  who answers and with whose credentials, and stay a human decision.
- A Quick Prompt backed by an HTTP connection can be compared across agents
  again. Its connection used to leave with every target, so each CLI or local
  one was refused at dispatch: a sandbox comparison ended 0 of 5. The target
  now decides which connection its run uses, and a connection Kronn cannot
  route to yet is shown in the list with the reason rather than dropped from
  it in silence.
- The round ceiling of a discussion agent follows the window its model really has
  instead of one constant: measured for a local model, published per endpoint for
  an OpenRouter one, and the previous 150 where the provider says nothing. In a
  discussion, reaching it now ends on a partial answer rather than a failed run.

### Fixed

- Desktop installers include the internal MCP bridge with its Python runtime
  and direct agent callbacks to the embedded backend's actual port. Claude and
  Codex no longer depend on the build machine's source path for this bridge.
  Claude workers on native Windows report their unavailable required sandbox
  without weakening isolation or disabling ordinary discussions.
- Opening the desktop app no longer saves its temporary listener port into
  the shared CLI configuration. Later CLI starts retain their configured port
  instead of timing out while waiting for an API on a different port.
- Live Pages follow Kronn's selected theme instead of the operating system's.
  Refreshes with unchanged data no longer republish to the iframe and reset
  local page state; newly loaded frames still receive the current data.
- Optional workflow inputs left blank or omitted no longer disappear from
  execution snapshots and fail Gate messages or Exec stdin templates with
  "Unknown workflow template variable" (#213). Declared empty inputs remain
  available, Select defaults still apply when omitted, and required inputs
  and genuinely unknown variables retain their checks. Existing snapshots
  stay immutable; affected runs need a new launch.
- Quick Exec runs retain their measured exit code and captured stderr across
  reloads, including failed commands and invalid JSON output. The run card
  shows the exit code and folds stderr separately from the business result;
  old runs and processes with no measured exit code remain unknown.
- A Quick API cannot be deleted while a workflow references it. The refusal
  names the affected workflows and steps, including collection sources and
  failure handlers. The workflow editor also flags missing Quick APIs left
  by older deletions before a run is launched.
- Expanding a workflow run loads its complete step outputs instead of showing
  the empty output fields from the compact history list. A failed detail read
  is visible and can be retried; collection failures include the saved source
  id alongside the alias and cause.
- API steps accept successful responses with an empty body, including 204
  after an assignment, transition or deletion. They return null data with the
  HTTP code in the summary and signal, so a completed write does not look like
  a JSON parsing failure. Nonempty invalid JSON still fails explicitly.
- Single Quick Prompt launches retain saved effort and output-token limits in
  an immutable launch snapshot. HTTP requests apply those controls on each
  tool round, even after the template is edited. A transport that cannot apply
  an explicit control refuses the launch with an explanation.
- A discussion agent can read its own history even when its room has no
  project. Reading a different room still requires a shared project.
- The setup folder picker can select the mounted root itself, keeps the latest
  requested folder when responses arrive out of order, and reports unreadable
  directories instead of showing them as empty.
- Repositories are found wherever they live. The scan did not follow symbolic
  links, so the obvious workaround — a link in the home pointing at the real
  folder — changed nothing. It follows them now, guarding against walking the
  same directory twice. Identity was also keyed on the repository's name and
  remote, which merged two clones of one repository into a single entry and
  collapsed every local-only repository sharing a directory name, a missing
  remote reading as an empty string rather than as absent. A workstation
  holding `git/<organisation>/<repository>` for several organisations saw one
  of them. It is now the directory itself.
- An agent that read its task three times lost the ability to read it at all.
  The duplicate-call guard does not merely refuse a repeated call, it removes
  the tool's declaration from the request — so after a change made the answer
  different, the agent could no longer see it. The reading tools come back once
  progress actually moves, without ever widening the catalogue the run was
  given.
- A local run is sized from what it actually costs instead of a guess. A prompt
  was priced at three bytes per token for everything; measured across six local
  models, prose runs 4.9 and JSON tool declarations 3.7, so Kronn over-estimated
  a request by 24 to 64% and refused runs that would have fit. The window a
  machine can hold is now computed too, from the model's own weight and the
  cache cost it publishes, rather than read off a memory band: a model that says
  how wide its attention is gets its real window, and one that says nothing
  keeps exactly the ceiling it had.
- The project documentation is no longer copied into every request to an HTTP
  agent. Those agents read files through their own tools now, so the prompt
  carries the doc's section index and the path, and the agent opens it when the
  turn needs it. Measured on six local models plus two remote providers: every
  one of them reads the doc in a single call and answers as well as before, for
  42% fewer prompt tokens on a turn that does not need it. `read_file` was
  already declared to them; the prompt was still telling them it was the only
  file they could see. An agent with no file tools keeps the inline copy, and
  `KRONN_INLINE_PROJECT_DOC=1` restores it for anything else.
- An Ollama request is sized with the tools it carries. The window was computed
  from the messages alone, while the declared catalogue reaches the model in the
  same prompt and is the larger half of a short turn. The guard that refuses an
  oversized prompt, whose whole purpose is to stop Ollama from silently dropping
  the head of a conversation, was reading the same blind estimate. Both count the
  catalogue now, and a request that still does not fit says so in the log instead
  of being quietly truncated.
- The worker mitigations for Ollama's MLX engine apply only to the versions that
  need them. They were sized against a prompt-prefix bug fixed in 0.34, so on a
  newer server an MLX worker explores as long as any other instead of stopping
  eighteen rounds early. Kronn reads the server version once per endpoint; one
  that does not answer keeps the mitigation. The window cap stays either way:
  MLX fixes the slot when the model loads, which is a memory bound, not a
  latency one.
- The `/no_think` control token is no longer sent to a server that honours the
  `think` flag, which Ollama has done since 0.19.
- A tool an agent cannot see is no longer read as a tool that does not exist.
  Asked for two things at once, a local model answered that the image tool "is
  not available in my current catalogue" and stopped, without ever loading the
  family that declares it; another sent `tools_load` with no arguments at all,
  read the refusal as a dead end and abandoned the task. The index now says that
  a tool it names is one load away and that calling it missing is always wrong,
  and a `tools_load` with no family is answered with the families that exist
  rather than an error. Measured on six local models: all six now reach the
  tool, where two of them used to give up.
- A local model that falls into a repetition loop no longer blocks its run.
  Ollama caps nothing on output, so such a model generated until the context
  window was full: measured on a local 12b, a single turn ran past twenty
  minutes and returned nothing, and the same turn answers in eight seconds once
  the output is bounded. One turn now generates at most a quarter of its window
  (1024 to 8192 tokens); `KRONN_OLLAMA_NUM_PREDICT` moves that bound and `0`
  removes it.
- The command Kronn offers to install or update Ollama now matches the machine
  it runs on: Homebrew on macOS, winget on Windows, the vendor script on Linux.
  That script refuses to run anywhere but Linux, so a Mac reading the update
  badge was handed a command that could not work.

---

## [0.13.2] - 2026-09-19

### Added

- The Assets panel of a discussion is split into tabs. **Tout** keeps the
  inventory with its search and filters, **Creator** replaces the "Générer un
  média" button with the generation form, and **Editor** appears once the
  discussion holds two clips or more.
- Editor puts the clips in the order they should play, by drag-and-drop (a
  line shows where the clip will land) or with the arrows, each shown by a
  small thumbnail of its first frame, its length and its price. A clip that
  has nothing to do with the film is moved to "Hors version finale": it stays
  in the discussion and is not played. The order and the clips set aside are
  saved with the discussion, and a clip generated later joins the end of the
  film.
- Editor shows the total length and the total price of the clips in the
  final cut. A clip with no declared price, an upload, or one billed on your
  own key is not added: the total says how many were left out rather than
  counting them as free.
- "Lire la version finale" plays every clip of the film in order in a
  full-screen player, each one starting as the previous ends. It only plays
  them; no video file is produced.

### Fixed

- An agent in a plain discussion can generate an image or a video again, as
  0.13.0 promised. `agent_list`, the only place a media connection id appears,
  refused any agent that was not a member of a room, and an agent Kronn
  launches in a plain discussion never is one. On a live instance an agent was
  refused three times, tried three spellings of the model name, and stopped to
  ask for an id it had no way to see. Reading the catalogue is not a
  delegation: it now answers in every discussion, while `task_exec_prepare`
  and `task_exec_launch` keep their guard.
- A generation accepts the connection's alias or display name, not only its
  opaque id. A name two connections share is refused rather than guessed, and
  every refusal lists the connections that can generate the requested
  modality, with their ids, so the next attempt is the right one. An alias and
  the id it stands for are one generation: a retry that switches between them
  is not billed twice.

## [0.13.1] - 2026-09-18

### Fixed

- A Live Page button now launches the row it sits on. A page listing tickets
  draws one button per row from a single action block, and the first click
  consumed that block for every row: each later click, on any ticket, showed the
  first ticket's success and ran nothing. The block is now an offer that is
  never used up, and each click is its own launch. A row still running is not
  launched twice, a finished run no longer disarms its button, and a button
  without a row binding can be relaunched. Launches recorded before the upgrade
  are kept.

- A Live Page button now shows how its row went, and its card says where the
  run is. The button is marked while its run goes and once it has succeeded or
  failed, including after a reload. Clicking a row that is still running
  reopens that run rather than a blank form, clicking the same button again
  closes the card, and the card also closes with × or Escape. The card names
  the row it is about, says the run is starting before one exists, and shows
  the run's steps and elapsed time while it goes; it used to fold them away
  exactly then. Once a run is over, the card offers to launch the same row
  again.

- An action card tells what its run produced, on a Page and in a Discussion
  alike. A row that has run reopens on its latest run rather than on a blank
  form. A workflow's steps are listed by name, type, status and duration
  instead of a raw JSON result, and the discussions the run opened are shown
  with their agent's state and the beginning of its latest answer, one click
  from the full text.
- A Quick Prompt launched from a card is done when its agent has answered,
  not when its discussion was created. The card said "done" and the Page
  button showed ✓ while the agent was still reading, and kept saying it if the
  agent then failed. The launch now stays running until the agent's first turn
  ends, then succeeds or fails with the reason; later turns in that discussion
  are not the launch's. This holds in Discussions and on Live Pages alike.
- A Quick API card reads as its summary line ("POST …/graphql → 46 items") and
  a table of its first rows instead of up to 20 000 characters of JSON, and a
  Quick Exec card shows its JSON output as key → value. The raw payload stays
  one fold away.
- Agents are told how to put these buttons on a Page where they write its
  HTML. The `page_update_html` tool now has a manual of its own, sharing the
  button contract with `page_create`'s: one block for every row, the button
  states, what the card shows, and why a block's reference must stay stable
  across revisions. It used to say nothing about buttons, though it is the
  tool that adds them to an existing Page.
- `workflow_run_discussions` finds the discussions a workflow's Batch Quick
  Prompt step opened. They belong to a child batch run, and only the run's own
  discussions were looked up, so the tool answered "none" for exactly the runs
  whose work happened there.

## [0.13.0] - 2026-09-17

### Added

- An agent can ask to be called back when a generation settles, instead of
  polling for it. Without it, chaining an image into a video cost a full agent
  turn per step — which is what a room waking up for no apparent reason was.
  It fires on failure too: an agent told only about successes waits for ever on
  a clip the provider refused.

- Settings shows where the database's weight actually sits, one bar per table.
  Row counts mislead badly: one instance held 34 678 messages in 41 MB and
  2 554 workflow runs in 2 620 MB, so ranking by count pointed at the wrong
  thing entirely. Measured on demand — never on page load, since reading the
  b-trees costs about a second on a large database.

- The worker catalogue an agent reads now includes the configured external
  connections, and says which media each one can generate. A connection has no
  local binary, so agent detection produced nothing for it and it was absent
  from the catalogue entirely — an agent could not delegate to a configured
  OpenRouter, and had no way to learn that image or video generation was
  available at all. Modalities are listed one per configured model and stay
  absent otherwise, so an agent is never told it can produce a video that the
  generation request would then refuse.

- HTTP agents author and run automations like CLI agents do: `qe_run`,
  `qa_create_draft`, `qa_update`, `qe_create_draft`, `qe_update`, alongside the
  `qe_list` that was missing. An agent could already start a saved Quick Exec
  through `agent_job_start` and had no tool that could tell it one existed.
  Authoring stays out of worker rooms, where a task is already briefed.

  An update names only what changes: Kronn reads the stored definition and
  overlays the call onto it. Demanding the whole object would have been the same
  defect as an id no tool can produce — `qa_list` returns a compact view by
  design, since it is paid on every turn, and could never have supplied one.

- A `tool_manual` on the native surface, the one the MCP bridge has had. The
  native catalogue is re-sent on every turn, so a description that explains an
  argument shape is paid on every message of every room; the declaration now
  carries the contract and the manual carries the detail. The five new tools
  cost 4 546 B rather than the 12 913 B their bridge equivalents weigh.

- Each media slot in that catalogue now carries what the provider advertises —
  supported durations, resolutions, aspect ratios, frame positions — read from
  the same catalogue the launcher shows. An agent asking for a 15 s clip on a
  model that tops out at 12 s learned the list only from the refusal, one turn
  and one provider round-trip later. An unreadable catalogue leaves the envelope
  unstated rather than empty: empty lists would read as "supports nothing".

- A duration, resolution or ratio the model's catalogue excludes is now refused
  by Kronn, with the list, instead of by the provider a round-trip later. Only
  an explicit list refuses — a field the provider never advertises, or a
  catalogue that could not be read, still leaves the submission to decide.

- Agents are told how to chain clips: a reference picture may be an earlier
  clip's last image, and since Kronn cannot cut that image out server-side
  (KT-550), the surfaces now say to ask the reader to keep it from the Assets
  carousel. Passing a clip id as a reference used to answer "not an image",
  which named the mistake without naming the way through it.

- Installed CLI versions are compared with current stable releases from their
  official sources. RTK and ccusage have separate checks and diagnostics;
  checking never installs an update. Cached results and an explicit recheck
  keep discovery bounded instead of hard-coding the next version to expect.
- Claude model pickers use the installed runtime's account-specific catalogue,
  including newly exposed model identifiers, and share refreshed results.
  A transient discovery failure preserves known available choices; an actual
  authentication or availability failure is still reported.
- Claude and Codex tiers can pair a model with an optional reasoning effort
  advertised by that model. An execution override wins over a same-model tier
  preset; leaving both empty sends no effort flag and keeps the CLI default.
  Changing models clears an incompatible preset in the same save. A Quick
  Prompt launch keeps its explicit effort independently of later prompt edits
  or deletion, bound to that discussion's agent, tier and resolved model.
- **Important message** opens a plain-text form above the ordinary composer:
  Information, Decision or Blocker, with an optional link to an existing task
  in this discussion's plan. No JSON to write and no permanent key field in the
  normal composer. Publishing never creates, edits or completes a task; the
  card's link opens that task in the plan. The ordinary draft stays independent.
  Form drafts survive closing and navigation in memory, not a browser reload.
  An uncertain send keeps its original identity for reconciliation instead of
  blindly publishing again.
- Important messages are persisted objects, not formatting. The structured
  `kronn-important` contract also supports a scope change, a DoD waiver, a blocking
  alert, an action needed from a human, an accepted delivery — and Kronn keeps a
  card the discussion can filter, count and step through, instead of a bold line
  that scrolls away. A worker's delivery report stays a delivery report: when a
  fact inside one matters, a separate card points at it. A replay after a
  restart collapses onto the fact it already recorded rather than doubling it.
- Publishing a card asks who you are. It takes a credential enrolled in
  Settings and a single-use proof bound to the room and to the exact text, so a
  captured request cannot be replayed or aimed somewhere else. A worker reaching
  the room through the agent bridge publishes none while it is working, whether
  it holds a valid credential or not — that path authenticates the session and
  its assignment, and a worker reports through its delivery. The human composer
  takes another route, proving the grant and the single-use proof without a
  session. Caller-authored cards require an enrolled
  credential; without accepted authority the ordinary message still posts and
  the response explains the refused card. Kronn's own orchestration steering
  events use their backend authority and do not require a human credential.
- The publication bootstrap can be rotated, and recovered when it is lost.
  Rotating it from Settings writes the replacement to the operator's private
  file and answers with the path — the secret travels over no wire, and the
  screen locks back because the authority just used is gone. An operator who
  lost it leaves a `recover-admin-secret` file in the Kronn data directory and
  restarts. Credentials already enrolled keep working through both.
- An agent can put a decision to the human and stop, without the question
  scrolling away. A `kronn-question` block becomes a card that stays pinned
  while it waits: the question, what it blocks, the options with their
  consequences, the agent's recommendation — marked, never pre-selected — and
  free text, because the right answer is regularly one nobody listed. The
  answer is a durable message, so whoever picks the work up later reads the
  decision instead of asking again. A malformed block says which field refused
  it rather than vanishing in silence.

- A discussion can be opened for a media generation alone. New discussion
  offers `Generate media` when a compatible model is configured; the room is an
  ordinary one with no agent, and nothing is generated until the form is
  explicitly submitted.

- An image in the assets carousel can become another image or a video. The
  actions appear only for what the configured models can actually do, and the
  source image is carried into the generation form rather than re-attached by
  hand.

- HTML source files preview in place. Projects > Code switches between the
  source and a sandboxed static render — no scripts, no navigation, no network
  — and Live Pages revision diffs offer the same before/after view.

- A provider whose quota escalation was never closed can be re-armed. Tasks
  that reached a terminal state stop holding the lock, and an audited manual
  re-arm releases the rest without replaying any old work.

- A reassigned worker can deliver again after a timeout or unavailability
  interrupted an unreviewed delivery. The next attempt gets fresh review
  identities; an already occupied message ID no longer rolls the delivery
  back. Earlier messages stay intact and the new request names the current
  commit to review.

- Quick Prompts read `{{env.NAME}}` like every other placeholder, and their
  editor offers the project's own environment names rather than a blank field.

- The discussion plan filters on what is running. `Focus / In progress / All`,
  with an indicator on tasks that are actually working, and the choice is
  remembered.

- Kronn accepts support through Ko-fi, from the GitHub funding metadata and a
  card under the version in Settings.

- Search can be told where to look. A term that appears in one discussion's
  title and in twenty transcripts used to drown the room actually named after
  it, and no amount of ranking fixes that — the reader wants to exclude, not to
  re-sort. So the scope is a filter, title / content / both, and it is
  remembered between visits. Under "both", a title match now comes before a
  content match: someone searching for a name is looking for a place, not for
  an occurrence. Both HTTP search endpoints gained the same scope; the MCP
  `disc_search` tool does not offer it yet, so an agent's search stays unscoped.
- An approved delegation now publishes its report. The derivation and the
  store existed but nothing called them, so an accepted delivery produced
  nothing at all. Both approval paths publish it — the ordinary one, and a
  replayed approval that repairs a crash between the approval and its message.
  A new report and its message are written together, so one can no longer be
  created without the other; a record left without its message by an earlier
  run gets one, rendered from the stored payload rather than the caller's.
  Publication is attempted after the approval, which is already durable: a
  failure is logged and can be retried by a later approve replay, not rolled
  back — an unreadable manifest or a database fault means no report yet, not a
  corrupted approval, and a manifest that never parses never produces one. A
  stored report payload that cannot be read back renders as a diagnostic
  instead of vanishing. The
  worker's duration counts from its own assignment, so earlier review rounds
  are not charged to its attempt.
- Workflows, quick prompts, quick APIs and quick execs can be deleted from
  their own row — armed by a first click, done by a second, never before the
  server confirmed — and several at once from the sidebar selection. While
  armed, the control says what goes with it: the past runs a workflow takes
  along (their discussions stay), the workflow steps a prompt or an API is
  used by (they fail on their next run), or that nothing else is touched. A
  count that cannot be read says so, instead of reading as nothing.
- A clip's last frame can be kept as an asset of its own, with a link back to
  the clip it came from. It is decoded in the browser, by the player already
  showing the clip, and runs only from a click in that viewer — there is no
  headless path, so no agent can ask for it through MCP. It works wherever the
  interface is served, Docker included.
- Image and video generation on HTTP connections (LiteLLM, NVIDIA, OpenRouter).
  Media models are configured as their own slots on a connection — modalities,
  not quality tiers — so a text step can never select "tier Image". A
  generation is launched from a discussion's Assets tab or by an agent through
  MCP (`media_generate`, `media_job_status`); each launch gets its own bubble at
  its chronological place in the transcript, which turns on the spot from
  pending into the finished media — or into a stated failure — and comes back
  the same way after a reload. The launcher is free again the moment the job is
  accepted, so a video and two images can progress in parallel without mixing
  up their states. Videos play inline with native
  Picture-in-Picture, and opening any asset browses every image and clip of the
  discussion in one carousel. Media spend is its OWN counter, reported per
  generation from the provider's billed figure, never recomputed from a
  published rate. The estimate shown before sending comes from past billed
  generations, and reads as unknown — never as free — when there is none.
  A finished generation shows the media itself inside its bubble, with the
  price in the corner and a single way out — the asset in the carousel; the raw
  job JSON folds away behind a "Details" toggle, and the only duration on
  screen is the one of a produced clip. A video's soundtrack is now a stated
  choice on both surfaces — a checkbox in the form and `generate_audio` on the
  MCP tool, checked/true by default, which is what the providers were already
  doing silently. That silence had a clip refused for audio copyright with
  nothing in the request to explain it. The launcher's durations, resolutions,
  ratios and audio switch use the provider's own catalogue when it advertises
  them, instead of a fixed list that offered choices the model rejects; a model
  that advertises none keeps the previous defaults rather than leaving an empty
  picker. A video can start from — or end on — an image the discussion already
  holds, picked in the form or named by an agent through MCP. That image is
  referenced by id and travels inline: no local source path or Kronn asset-fetch
  URL is sent to the provider.
  An asset can finally be deleted, from the viewer, in two steps — the control
  removes bytes from disk and sits next to "close", so it arms before it acts,
  the viewer closes rather than silently landing on the neighbouring media, and
  a server refusal leaves the file exactly where it was.
  An image generation can now be drawn from SEVERAL pictures of the discussion,
  up to the number each model advertises, rather than an assumed universal
  limit, and the bubble opens every one of them. Asking for a
  frame on an image, or for several images on one frame, is refused instead of
  being quietly reduced to something else and billed for it.
  A clip's last picture can be kept as a file of the discussion in one click,
  and reused straight away as the starting image of the next generation — no
  download, no re-upload, no dependency to install. It is decoded by the
  viewer's own player, which reads these clips where the backend cannot, and an
  extraction that decoded nothing says so rather than attaching a black
  rectangle. A picture too narrow for the provider blocks the launch before it
  is billed.

- Discussions now report their storage weight, split by what a cleanup could
  actually reclaim: attachment bytes held on disk, extracted document text, and
  message content. The sidebar shows a green / amber / red indicator whose
  detail panel breaks the three masses down and states how much is reclaimable
  without losing any conversation. The indicator is configurable from Settings
  (`[server.discussion_weight]`: `enabled`, `amber_bytes`, `red_bytes`) and
  disabling it removes the queries entirely, not just the badge. Weights are
  served by a bounded batch endpoint — it never scans every discussion.

- Codex and Claude Code run through the same create/resume/stream/cancel/
  close ACP contract as the native ACP agents, and that contract is the normal
  path rather than an opt-in. The per-agent environment variables
  (`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE`) changed meaning
  accordingly: unset selects the adapter, and returning to the direct CLI now
  takes an explicit `0` or `false` — a deliberate, observable fallback instead
  of the default. Task workers take the same route, with the narrow policy the
  direct builder already applied carried over whole: their own worktree, no
  inherited user configuration, a short tool allowlist, a fresh session, and no
  resume of a room's conversation. A shared, scoped, audited permission broker
  denies filesystem/terminal requests, unbound sessions, out-of-project paths
  and unauthorized MCP server/tools by default, and a worker never receives the
  room bridge an ordinary discussion gets. Project MCP servers are never
  inlined with credentials, prompts travel on stdin, and normal discussions
  persist the native Claude/Codex conversation id so a backend restart resumes
  the same CLI session without reusing it across projects. See
  `docs/operations/acp-adapters.md`.

- A dedicated `http_transport` module now owns the seam between LiteLLM,
  NVIDIA and named Custom connections and the shared OpenAI-compatible chat
  codec, kept deliberately separate from the ACP boundary (`docs/design/
  adr-004-http-transport.md`). The OpenAI Chat codec selection is an explicit,
  single decision point; models positively identified by the catalogue as
  image/video-only are refused before text dispatch with a diagnostic.
  Discussions persist a sticky named connection (`discussions.connection_id`)
  so an ordinary reply with no explicit `@mention` keeps resolving through the
  same connection instead of losing it — previously only the very first
  message of a Custom-connection discussion reliably carried its target.
  Compare's AI judge/prompt-improver launch and multi-agent orchestration
  debates can now address a specific named connection instead of only a bare
  agent type, and orchestration validates that connection and its model against
  the catalogue before each launch, as Quick Prompts already did. See
  `docs/operations/http-transport.md`.

### Changed

- The public-site gallery was rebuilt from the application as it is today:
  twenty-four cards in the three languages, shot against a seeded sandbox with
  real providers rather than mocks. Six captures still showed the shell from
  before the Pages tab existed, and four capabilities — projects, planning, the
  automation library, the database weight — had no card at all. Each caption now
  names what you can do, the condition that makes it possible, and glosses the
  words this project invented. The four animated cards became video with an mp4
  and a webm source, pause when off-screen, and carry a badge; every slide has
  its own shareable `#media=` link, and the carousel stopped skipping the
  videos.

- Projects > Code fetches one folder when it is opened, instead of the whole
  repository up front. The old whole-tree ceiling could silently cut the tree,
  making omitted files indistinguishable from files that did not exist. The
  remaining bound applies to a single answer and is reported when reached.

- A `[src: …]` citation reads as a chip rather than as notation. The file's name
  and lines instead of its whole path, a sha cut to seven characters, a URL by
  its host — with the exact reference on the chip, because a citation exists to
  be checked. It carries the verdict the backend already reached: verified,
  suspect, or honestly unknown.

- The panel strip floats over the conversation instead of holding a column.
  Closed, it reserved its width down the whole height for two buttons sitting at
  the top of it; only the message-search bar now steps aside for it.

- The Kronn mark is drawn rather than resampled, at every size it appears —
  browser tab, desktop icon, app header, share card — and the startup screen
  turns it slowly instead of showing a generic spinner.

- The model catalogue is one sorted table instead of separate stacked lists.
  Models share an alphabetical table, sortable by name or source, invertible, with
  search that matches the exact model id as well as the displayed name, and a
  click on a source narrows to it. The sources keep their re-check control on
  one line each rather than one card each.
- Adding a model by hand moved to the foot of that block, leaving discovery
  and the existing catalogue first. Manual entry stays, because a target
  whose detection returns nothing has no other way to name a model.

- Settings names the three ways of reaching a model. Config > Agents listed
  seven full-width CLI cards, then Ollama as a special case inside the same
  loop, then the external connections in a framed box of their own — one
  undifferentiated list. There are three framed zones now, CLI /
  Local / External API, built the same way so they read as three of a kind, and
  the model catalogue moved below them: a catalogue is what the modes draw
  from, not a fourth way of reaching a model.
- A CLI agent's card shows which model each tier will actually run, the way an
  external connection already did — economy, standard, reasoning, with the
  override you are editing, else the assignment its runtime catalogue carries,
  else an explicit default label — three cases in that order, and never a model
  guessed from a list baked into the frontend. Configuring them stays behind
  the edit control, which now sits next to delete as an icon pair, so a CLI
  agent and an API connection offer the same two actions in the same place.

- The agent cards sit in two independent columns. Laid out as a grid, a row
  was as tall as its tallest card, so a short one left a hole beside a tall one
  — and expanding either pushed both columns down, moving cards the reader was
  not looking at. Each column is its own stack now: a card only ever moves the
  cards under it, in its own column. Below the breakpoint the two collapse back
  into one list in the original order.

- An agent's card says its name once. The title and the mention-colour chip
  beside it both carried it; the title is the colour control now, so picking a
  colour happens on the name that colour applies to. What only matters while
  configuring — the full-access switch, the API keys, the model tiers — folds
  behind a "Configure" button, and a card at half the width stacks its header
  instead of splitting it into a left and a right side.

- The discussion header reads as two lines instead of three. Inviting a peer
  now sits at the right of the title's own line, where an action on the whole
  discussion belongs, and the participant chips moved down to share the second
  line with the control that decides whether the discussion answers by itself —
  first position, since it changes what everything beside it means. What the
  discussion IS rather than what it does — its project, its cost, its worktrees
  — folds behind a "details" button at the right of that second line, and the
  fold is remembered, so anyone who wants those figures permanently opens the
  row once.
- The panel strip lines up with the message-search bar beside it in both
  states — open and closed — its delete keeps the red it had in the header, and
  search moved to the right of the control that opens the strip. That control
  now shows an active state, so the strip says which of its buttons put the
  panel on screen.
- A discussion's title no longer vanishes on a narrow screen. The id pill and
  the session binding shared its line while the title was allowed to shrink
  away. Those two move into the details fold on narrow screens, and the title
  keeps the width it needs.
- A discussion's header stopped acting on the discussion. Search, export and
  delete joined the panels in the strip that sits above the panel column, so
  the header only describes what the discussion IS — its title, its agent, its
  tier, its counters. Message search opens level with that strip rather than a
  few banners lower, because the control and its field belong to the same row.
- The attached-runs list is gone from the transcript. It repeated what the
  Automations page already shows, and cost a slice of the conversation's height
  to do it; a launched action is visible as its own card in the thread.
- A discussion's panels — plan, assets, code, terminal, settings and message
  search — left the header row. It also carries the title, the agent, the tier
  and the counters, and every version narrowed it further. One control now
  opens the panel column, landing on whichever panel was last read, and it
  lives in that column rather than in the header: a control that opens a panel
  has to sit against it, and a row of counters came between the two. Moving
  between panels happens in the same strip, which widens into icons once a
  panel is open. Each panel already draws its own header, so listing those same
  icons up in the discussion header showed every one of them twice. Clicking
  the panel already open closes it, as the header buttons did.
- Eligible conversation turns resume with a bounded delta instead of resending
  the whole thread. A completed checkpoint establishes what the agent has
  already seen; without that proof, Kronn keeps the full-context fallback.
  Five things keep it honest: the delta is everything since the agent's last
  turn, never just the newest message, because the human or another agent writes
  in between; the marker is a message id, so a history that was edited or pruned
  simply fails to match and the full prompt goes out, where a numeric cursor
  would have sent the wrong slice; the cursor only advances once the reply is
  stored, so an interrupted turn is replayed rather than skipped; a dead
  conversation id would fail the turn outright, so a direct Claude CLI turn
  probes its own `--print --resume` session store first and a miss means full
  prompt, while the ACP and adapted routes — where that probe would always miss
  and force a full prompt every time — rely on their checkpoint and a bounded
  fallback when a resume finds a safe absence; and resume never travels without
  its delta, nor a full prompt with a resume. Task workers never resume — a
  worker opens on a fresh worktree, and a room's history is not its business.

- An agent's prompt no longer carries every MCP server's documentation. It
  concatenated all of `docs/operations/mcp-servers/*.md` in full on every spawn
  whatever the discussion was about. It now carries the server listing,
  the `mcp__<server>__<tool>` convention, a pointer to `tool_manual` for the
  Kronn tools, and where to read a server's own notes if the agent is going to
  use it. Only CLI agents received this block, and they can read those files:
  the prompt now points to the documentation instead of embedding it all.
  Discussions without a project already worked this way, and the two paths now
  agree.

- Automatic conversation summaries are gone. They fired after every reply past a
  per-agent threshold, and had been dead in practice: the global default was
  `Off`, acting as a master kill-switch, so a discussion displaying `Auto` never
  summarised — a strategy shown in the interface that could not apply. Removed
  rather than repaired, because every runtime can now read the thread back
  itself (`disc_meta` and `disc_get_message` over MCP, or a declared tool),
  which is cheaper and more
  precise than a summary produced in advance for a need nobody expressed. The
  `Auto` strategy no longer exists; rows written before 0.13.0 read back as
  `OnDemand`, which keeps `disc_summarize` and the summarise action working for
  an agent or a human reopening a long room. When history is trimmed, the notice
  now tells the agent to reread the thread first, and to ask the user only if
  the answer is not there.

### Fixed

- During an ACP turn the discussion's log panel showed nothing but the internal
  thread marker: seventeen `Bash` calls produced seventeen `[acp-tool] Bash`
  lines and nothing else. Routing tool calls through the run's stderr capture
  is what lets the transcript render them under the reply, but the same capture
  feeds the live panel verbatim, so the marker escaped to a surface a human
  reads — filling the panel's fifty-line window and evicting the diagnostics it
  exists for, and leaking into the collapsed status line by default. The task
  and its test now share one predicate, so the two halves cannot drift apart
  again (#210).

- A room with two thousand messages froze on open. Neither the data nor the
  query was the cost — two megabytes in total, answered in under ten
  milliseconds — it was parsing every message's markdown up front. Messages now
  start as text and become markdown as they approach the viewport or the
  browser goes idle. Every message stays mounted, so search, Cmd+F and every
  jump to a message are untouched.

- Replies written by an ACP agent carried their own tool calls inside the prose
  — one held ninety-three of them — because the transport forwarded a call on
  the channel carrying the reply. They now reach the tool group under the
  message, and the replies already written are repaired when displayed.

- Listing runs shipped every step's full output to the browser, two hundred
  rows at a time. On one instance that was 2 620 MB of a 7 200 MB database,
  duplicated from the runs themselves; a migration reclaims it.



- Mention three agents on one message and two of them would answer nothing at
  all. Three separate causes, each hiding the next. A room backed by an external
  connection put that connection on every dispatch in it, including siblings
  explicitly targeting a native CLI, which were then refused for not being the
  agent it serves — an inherited connection that does not match is absent, not
  wrong. A refused start wrote its reason to the job and the room had no way to
  read it: the list of a discussion's agents excluded anything that had failed,
  so the placeholder vanished and nothing replaced it. And ACP held a whole
  prompt turn to the thirty seconds meant for a handshake, while the CLI path
  grants the same work minutes to hours. A refused agent now stays visible and
  says why, in the words the backend wrote.

- An agent was told it could generate images and videos, called the tool, and
  was answered "unknown tool". The tool was declared in the catalogue and
  implemented in its dispatcher; the router between them still listed only the
  two older names. The router now asks the catalogue instead of repeating it, so
  a tool cannot be declared without being reachable.

- The same picture could be bought twice. Media generation keyed jobs by a
  caller-supplied idempotency key and fell back to a random id, so an agent
  retrying after a wake paid again — and the field was declared with no
  description, so no agent could know that reusing it was the point. The key is
  now derived from the request when none is given, on both transports.

- A Live Page became unusable after its first CTA: listing its actions
  reconciled them against their runs, and reconciling meant writing, on the
  read-only connection the endpoint holds. It looked healthy until the first
  launch, then failed for everyone and could not recover — the reconciliation
  that would have moved the state on is exactly what was refused. Reads now
  project the reconciled state without persisting it.

- The child room of a delegated task stopped accepting messages once its worker
  launched. Asking the worker a follow-up, or a second agent for a review, was
  refused because the ROOM hosts a worker — a question about the room deciding
  for a dispatch that was never a launch.

- A durable wake that could not validate was retried every ten seconds
  indefinitely, with the reason written only to a row nothing displays. It is
  logged, and the loop ends after thirty attempts on an explicit escalation.

- An action card opened by a CTA at the end of a row was a few dozen pixels
  wide. Its width was the space between two offsets, so it shrank as the click
  moved right — measured at 42px, and 0 on a narrow shell. The card now keeps
  its width and slides left when there is no room.

- Two dependency overrides were pinning the versions an advisory had just
  named, so the mechanism meant to fix that class of problem had become its
  cause.

- A refused agent start said only "agent execution preflight failed". Kronn
  knew why — a missing project path, an unreachable endpoint, a connection
  without one — and replaced it with a sentence nobody can act on, for every
  case but two. Mentioning several agents and watching two of them vanish
  twenty seconds later left no trace anywhere; finding the cause meant reading
  the database. Refusals now carry the reason, which the neighbouring
  retryable-outage path already surfaced anyway. What is settled and what is
  worth retrying is still told apart — that decision never depended on hiding
  the diagnosis. Two places produced that sentence, and the second is the one a
  room goes through: it settles a tracked run, so its text is what lands in the
  job's last error and in the room. Its fifteen conditions — each of which had
  just established something precise — all reported the same nine words. Each
  now reports what it found.

- A backend restart could silently cancel the agents that had not spoken yet.
  Mention several agents on one message and they share a trigger, but they run
  one at a time: the first to answer posts a message newer than that shared
  trigger, so the restart recovery — which retires a turn once the room has
  spoken past it — retired the ones still waiting. They were answering the very
  same question. A newer message now only retires a turn when it does not come
  from a job triggered by that same message; a genuine new user turn still
  retires what preceded it.

- A configured external connection could not be mentioned in the composer.
  Typing `@open` only ever suggested the native OpenCode agent: the mention
  catalogue was built from a static list of the ten providers that ship with
  Kronn, plus the joined CLI sessions, and a connection had no way in whatever
  its alias. Each one is now offered under its own alias and pinned to its
  connection, so two cannot be taken for one another.

- The list of running discussions reported its own bookkeeping keys instead of
  discussions. A reply registers for cancellation under its dispatch id, not
  the discussion's, so the list missed every durable reply — the ordinary case
  — while counting workflow runs as discussions, which is what made the "N
  running" badge overcount. Keys are now resolved to the discussion they belong
  to, and anything that is not one is left out.

- A stream that went quiet was reported as a lost connection, and the report was
  wrong. The run lives in a spawned task the client cannot stop: it keeps going
  and persists its answer. Believing the agent dead, an operator resends and
  re-bills a whole context while the first reply lands on its own. Three faults
  stacked up, now fixed together: the watchdog's abort did nothing at all — it
  fired on a reference the line before had just deleted; the threshold was 5
  minutes where the backend grants a silent agent 15, so every Codex turn past
  five minutes tripped it; and the decision was made from local text alone,
  although the server's own list of running agents was already polled every 5
  seconds in the same component. The watchdog now asks the server, waits as long
  as the server does, actually aborts, and says what is true — the stream was
  interrupted, the agent is still running.

- A refused arbitration had nowhere to go: `pending` and `answered` were the
  only states, so answering was the only way to clear a question, including one
  that had become pointless. Refusing is now recorded as a resolution and
  reaches the asker through the same dispatch as an answer.

- A room backed by a custom external connection could not be replied to at all.
  Its composer was disabled under "this agent is currently disabled or
  uninstalled", so an OpenRouter answer could be read and never answered.
  `Custom` is an HTTP connection with no local binary, absent from the agent
  table by construction; binary detection returning nothing about it was read
  as evidence that it was missing. Detection silence about a remote provider is
  no longer treated as an answer.

- An ordinary reply in such a room could also fail with "the selected external
  API connection is unavailable", although the connection existed and the
  discussion recorded it. A reply that arrives without a dispatch job carried no
  connection at all; the discussion's own connection — the durable target such a
  reply is meant to resolve to — was never consulted. It is now the fallback,
  and an explicitly dispatched connection still wins over it.

- Agent replies arriving over ACP were shredded, with newlines inside words
  ("Rom", "u"). ACP delivers one fragment per model chunk and forwards it
  verbatim, but consumers put a line separator back between them, because the
  decision was made from the agent's type — which says which CLI runs, not how
  its output is framed. The transport now declares it. This affected every ACP
  agent, not only the one where the pieces were small enough to make it obvious.

- The live agent-log panel was unreadable in the light theme: 2.4:1, dark text
  on a dark ground. It paired a background that stays dark in every theme with a
  text token whose name says "muted" — a role, not a colour, and a dark one
  under a light theme. It now uses the token that means "text on a dark ground",
  as code blocks on that same background already did.

- Discussions could show a pending arbitration nobody asked for, reading "Which
  option should we use?". That is Kronn's own documentation example: when a run
  fails, the agent's raw output is folded away as technical details, that output
  includes the system prompt, and the prompt documents the arbitration format
  with a complete example. Ingestion read the whole message and recorded it —
  and nothing could clear it, since only a human answer closes an arbitration.
  Folded technical context is no longer scanned for questions.

- Launching Kronn on macOS installed a rebuild watcher. `kronn start` runs the
  backend natively there, because Docker cannot reach the host CLIs or the
  Keychain — and it inherited the development file watcher with it, so every
  pull that touched Rust rebuilt and swapped the process, cutting whatever
  agents were running. Hot reload now belongs to `kronn start-dev` and
  `make dev`, where someone editing Kronn wants it.

- Testing an external API connection could report HTTP 400 on an endpoint that
  works. The probe sent a chat completion to the first model the endpoint
  listed, whichever that was; on a proxy that also serves embeddings or
  rerankers, that entry answers 400 because it does not do chat. The configured
  tier models were never sent at all except for two providers. The test now
  probes the models the operator configured, probes nothing when none are set —
  an authenticated catalogue already establishes the credential — and names the
  model that failed, since one model failing and the connection failing are
  different facts.

- `rustls` moves to 0.23.45, clearing RUSTSEC-2026-0285. The dependency had not
  changed; the advisory database had.

- The application icons were not derived from the mark they show. At 512 px the
  shipped icon carried a soft halo and a gradient ground the source does not
  produce; at 32 px the drift was small enough to go unnoticed. Each icon is
  regenerated from its own canonical SVG — the desktop family keeps its dark
  plate, the web family its transparent ground — and a versioned check
  re-renders all of them, compares the images embedded in the ICO and ICNS
  containers against a fresh render, and fails on a hand edit.

- Projects > Code listed nothing under folders that came late in the alphabet.
  pnpm's content-addressable store is not a name the hand-written skip list
  knew, and generated files exhausted the walk's budget, so it stopped
  before reaching `site/`. Git already knows what is generated: the walk asks it
  once and does not enter what it ignores.

- A folder that had not loaded yet was drawn exactly like an empty one. It
  opened onto nothing and its contents turned up seconds later; it now says it
  is loading, and says so differently when the request failed.

- A directory reached through a symlink could serve files from outside the
  project. Symlinked children were skipped, but the directory the caller named
  never was — precisely because it was the one they chose.

- The global search re-ran the same term without noticing the scope had changed.

- A project's Docker link opens the port the container actually published.
  Compose files routinely name that port through a variable, so several
  projects on one machine take turns on 443 and the rest land on 8443 or 9443
  — but Kronn read the compose file's default and sent the link to 443
  regardless. It reached whichever project holds that port, which answers with
  an error from an unrelated application: nothing says you are on the wrong
  project, so the bug gets hunted in the wrong place. The running container's
  own publisher is now the authority, the port is still omitted when it is the
  scheme's default, each published host keeps its own, and an endpoint nothing
  is listening on is shown as unreachable rather than offered as a link.

- OpenCode answers in the room again. Every chunk it streams carries its text
  as a single content object, a shape the ACP reader handled neither as a
  string nor as an array — so the whole reply fell through, the turn was
  recorded as failed with no visible output, and the message offered two
  guesses that were both wrong. Its private reasoning, which arrives the same
  way, is read and deliberately not shown: it is a scratchpad, and folding it
  into the answer would leak it. The turn's tokens are read from the response
  too, where ACP puts them, instead of being dropped with it. Missing cost
  information remains unknown rather than being inferred from those tokens.

- The frontend lint gate is green again, by fixing what it flagged rather than
  by raising its ceiling. The release had added twenty-six warnings to a budget
  that had four left; all twenty-six are gone, and the final frontend candidate
  measures 63 warnings against a ceiling of 63. Nearly all described one habit — state written in an effect to
  correct what the render before it already showed — so those values are
  resolved where they are used, and the ones describing a particular thing now
  carry which thing. Two genuine exceptions are declared at the line with their
  reason: an image array whose identity changes every render, and the registry
  of in-flight AbortControllers, which the rule would have held in state.

- An action card aimed at another project's Quick Prompt launches, and says
  where it will run. The preflight refused it outright, which made a proposal
  unusable for the one thing it was for. The whole mechanism is human-gated —
  the agent proposes, Kronn checks the target, a person decides by clicking —
  and the checks that matter all still run: the target exists, its variables
  match its contract, the proposed project is the target's own, and that
  project exists. What the guard was refusing after all of those is a decision,
  not a risk. The card now carries the target's project, quietly and always,
  because lifting the guard without saying where the action goes would be worse
  than the guard. Discussions and Live Pages changed together: the same gesture
  cannot mean two things depending on where the card is drawn. Server-side
  resolution of a Live Page's dataset-bound values is untouched.

- Notes have a place of their own. An agent could already ask for a clean list
  of a discussion's notes; the route that served it was never called from this
  side, so the person who wrote them could not read them back — theirs were
  mixed into the transcript, behind a switch that showed all or none, with no
  way to list, correct or remove one. There is a Notes panel now, beside the
  others: it lists them, writes them where the list already is, corrects one in
  place and deletes it through the tombstone Kronn already had. A room with
  months of notes loads them a page at a time rather than in one request.
- Search can be pointed at notes alone. The scope filter gains a fourth
  option beside title, content and both: "notes only", for when the note is
  what you are after and the transcript around it is noise. Scoping to notes
  drops title matches with it — a note has no title of its own, and keeping
  them would quietly widen the search back to the whole discussion.

- An edited note says so, with the date. Silently replacing what someone wrote
  for themselves is the one thing an editable note must not do, so the panel
  carries the moment of the last rewrite — and stays quiet on a note that was
  never touched. On a note somebody else wrote, the edit and delete controls
  are hidden and their author is named instead. That is a guard rail in the
  interface, not an authorisation: Kronn has no multi-user authentication, the
  pseudo is declarative, and the endpoints behind those buttons accept any
  caller. It keeps a shared room from being tidied by accident; it prevents
  nothing. An unsigned note counts as this machine's own, so nobody is locked
  out of the notes they wrote before setting a pseudo.
  Successive corrections now share the discussion's ordered revision sequence.
  Updating the text and recording its history are atomic: an audit failure
  leaves the original note intact instead of changing it behind an error.
  Shared-note corrections keep their note semantics on the receiving peer:
  they no longer remove later conversation replies, change the note's written
  date, or replace its routing. Synchronization and the revision trail remain.

- A deleted note leaves the notes list. Its tombstone stays in the transcript,
  where a gap has to be explained, but a list of notes is not a transcript:
  showing the marker there would be listing something its author removed. The
  agent tool reads the same list and stops seeing it too, and the count agrees
  with what the list returns.

- A task can be taken out of a discussion's plan. It could be added, and then
  only moved between "active" and "later" — never removed. A room that runs for
  weeks accumulates work belonging to the next release, and reading its plan
  meant reading past all of it. `task_unlink_discussion` is the reverse of the
  tool that adds one; the task, its history and its other discussions are kept.
  It fits under the MCP surface ceiling rather than over it: the four heaviest
  descriptions it pushed past the line were tightened by nearly what it costs,
  and the ceiling moved by six bytes to admit it, then came back down as the
  surface was trimmed further.

- Kronn stopped telling agents to run linters the project does not have. A
  `composer.json` was enough to write `Lint: phpcs` into all eight generated
  instruction files, but phpcs is an optional Composer package, absent from
  most PHP projects — so every agent read a command that could not run, and
  the contradiction propagated eight files at a time. It is now declared only
  when something proves it: the vendored binary, a `require-dev` entry, or a
  ruleset. Ruff had the same flaw, since it does not ship with Python either,
  and a `lint` script was chained onto `tsc --noEmit` whether or not
  `package.json` defined one — which made the whole command fail where it did
  not. Linters that come with their toolchain, clippy and `go vet`, are still
  assumed from the language.

- Codex and OpenCode list their models again. Both had moved to shapes the
  catalogue reader did not know — Codex now describes each reasoning effort as
  an object instead of a bare name, and OpenCode spells its session options
  `currentValue` and `options[].value`. Neither is standardized, and a
  catalogue left in error refuses every turn for that agent, so both readers
  now accept the old and the new spelling. Codex also supplies a display name
  of its own ("GPT-5.6-Sol"), which beats showing the bare id.
- A message with no one mentioned now says where it goes, and reaches it. In a
  room whose native agent is switched off, an ordinary turn used to resolve to
  no destination at all — the native responder disabled, no peer named — so it
  reached nobody and the writer was told nothing. It now goes to the sessions
  joined to that room, which is what switching the agent off already promised.
  The header names that destination, resolved by the router itself rather than
  guessed from the discussion's configured agent, because that setting survives
  being switched off and announced a reader who received nothing. When there is
  genuinely no one, it says so.
- An agent reached over ACP — OpenCode and the other native ACP runtimes — now
  receives Kronn's own MCP bridge, so it can answer in the room it was invited
  to instead of joining it mute. Only the command travels over the protocol:
  the bridge reads its credentials from the environment it inherits from the
  process Kronn spawned, so none is ever serialized into an ACP payload.
- A generation asking for a source image on a provider that takes none is
  refused at submission, naming what is missing, instead of queueing a job that
  retries the same refusal until its deadline expires.
- A provider failure no longer ends a turn in complete silence. When the API
  is overloaded or a spend limit is hit, Claude Code does not report a
  structured failure — it writes an assistant message of its own, which no
  streaming delta precedes. The stream reader skipped every assistant message,
  rightly so for ordinary ones (they repeat what was already streamed, and
  keeping them would print each reply twice), and in doing so discarded the
  only account of the failure. The turn ended with no visible explanation.
  Errors the CLI writes itself are now read — and only
  those, identified by two independent markers so a build that stops setting
  one still surfaces them.

- An action whose target was deleted can no longer be launched. Preflight runs
  when the proposal is ingested, but the click can come much later — a
  transcript, and a published page, outlive the Quick Exec they name. Nothing
  checked again at launch, so the click was accepted for something that no
  longer existed. The action now says the target is gone instead. The check
  lives in the launch engine both surfaces share, so the guarantee cannot hold
  in a discussion and lapse on a page.

- A prompt's version history no longer outlives the prompt it belongs to.
- A clip recorded as `text/plain` still plays as a clip.
- A Quick Prompt batch item no longer carries a prompt of its own. The field
  was in the contract and nothing ever read it: each run takes the Quick
  Prompt's saved template and fills it from the item's variables. Removing it
  from the contract makes that explicit — a client still sending the field is
  ignored rather than refused, since the request shape accepts unknown keys.
- The agent bootstrap (`docs/AGENTS.md`) is back under its context ceiling
  without the ceiling moving. Verbose task-router rows and pointer-only
  sections were reduced. The router now routes and stops explaining — every
  detail it dropped already lives at the destination it points to — and the sections
  that only carried a pointer became rows of that same table. The inventory
  of root redirector files moved to `docs/repo-map.md`, where repository
  structure is documented.

- Two CLI sessions of the same provider joined to one room — two Claude Codes,
  say — now both appear among the participants, and each stays mentionable by
  its own alias. A session that was working rather than listening read as
  Offline, and the rule that hides the stale row left behind by a reconnecting
  CLI could not tell that row from a live peer: it hid whichever session was
  busy, so one Claude could not see or address the other. A row is now hidden
  only when it is Offline and nobody has heard from it for a while; a working
  session touches `last_seen` on every append and every wait, so it stays.

- Sending a message in a room with joined CLI agents, in a human-only
  discussion, re-sending a duplicate, or revising a message no longer raises
  "SSE stream closed before a terminal event" on every turn. The frontend had
  learned on 1 September to read a stream that closes without a terminal event
  as an interruption — which is what protects a half-written reply across a
  backend restart — while five backend paths still ended their stream on
  purpose without saying so. Those paths now close with an explicit `complete`,
  so the interruption rule stays true and stops firing on finished turns.

- Listing a workflow's runs no longer carries every step's full output just to
  render its name and status. Outputs are blanked inside SQLite on the listing
  paths, avoiding repeated decoding and transmission of complete results.
  `/api/workflows` likewise projects only the last-run fields it uses. Opening
  an individual run still serves its complete results.

- A discussion turn that fails on a saturated provider is retried instead of
  stopping silently. An overloaded turn could end without an explanation or
  another attempt. Saturation now retries with a growing delay,
  and the last attempt keeps the provider's own message in the room rather than
  going quiet. A hard quota is never retried — it is checked first — and `529`
  is never matched on its own, since those digits appear in ids, token counts
  and durations where a false positive would spend a real API call.

- The durable error of a failed turn no longer describes an ordinary room as a
  task worker. Every unsuccessful turn wrote the same "task-worker
  startup/completion failed … use `task_exec_reassign`" text, which sent readers
  looking for a sub-discussion that never existed and overwrote the provider's
  actual reason. Real task workers keep that guidance; an ordinary room now
  states the cause it actually hit.

- An agent that cannot start because of a NUL byte in its command line now says
  what carries it — an environment variable by name, an argument by the flag it
  follows, the program name, or the working directory — instead of repeating
  `nul byte found in provided data` on repeated attempts. The failure is settled
  before the process runs, so it is now a hard preflight failure, surfaced in
  the discussion. Values are never logged, only their carrier.

- The stream now says when a tool STARTS, not only when it finishes. During a
  long-running Bash call, the interface previously sat on a
  frozen placeholder while Kronn already knew which tool was running.

- A Page's inline Kronn action CTAs (`data-kronn-action`) now work from the
  standalone tab and every mosaic tile, not only the embedded viewer: clicking
  one opened only a same-origin link relay with no action handler, so the
  click was silently intercepted and lost. The three surfaces now share one
  `useLivePageActions` hook and the native `LivePageActionCard`, loading and
  validating each Page's own action list, failing closed on a removed
  `action_ref`, and keeping every mosaic tile's action state fully isolated
  from its siblings. A terminal action's "open discussion" jump opens a fresh
  same-origin tab since these surfaces have no Dashboard shell to navigate
  within.

- OpenCode no longer gets locked out of a discussion just because
  `~/.local/share/opencode/auth.json` is missing, empty, or unreadable.
  OpenCode also accepts environment credentials, its own provider config, and
  local or no-auth providers Kronn cannot see from that one file, so the
  absence of a confirmed auth signal now reports "unknown, assume runnable"
  instead of a hard "not ready" that blocked dispatch outright.

- The image/video model pickers for an HTTP connection (Settings > Agents >
  External API) no longer go silently empty for a provider whose catalog
  carries no modality metadata, as with NVIDIA's `/v1/models`: every one of
  its entries used to be treated as chat-only, hiding its real image/video
  models behind a filter that could never match. The picker now reads
  catalog evidence and renders one of three states — known (strict filter,
  unchanged for OpenRouter), unknown (the full catalog behind an explicit
  warning, search and free-text entry preserved), or unsupported (the
  connection proves it has no compatible model). A saved selection is never
  cleared by a refresh or a re-test, whatever the state.

- A task execution parked on a CLI worker's control offer — waiting for the
  exact target session to accept — can now be redirected to another worker.
  It used to refuse outright with "not a resumable worker state", stranding
  real work behind a CLI that never showed up. The same execution, checkout,
  history, attempts and review budget carry over untouched; the stale offer
  is cancelled in the same move, so the original CLI can no longer accept it
  late. Every other blocked reason — a session already committed elsewhere,
  or a protected-merge checkpoint — still refuses reassignment and requires
  a human decision, unchanged.

- An ACP agent that fails to start no longer leaves its process running. Nine
  early returns ended the session without stopping the host it had just
  spawned; `shutdown` killed without reaping, so the entry stayed in the
  process table; and the task draining the agent's stdout owned nothing — a
  dropped join handle detaches instead of cancelling, which left the drain
  alive after its process was gone. A start failure now terminates the host on
  every path while preserving the error that caused it, the process is killed
  and reaped, and the drain is joined under a bound: a panic still surfaces as
  a panic, and only the cancellation Kronn asked for is treated as ordinary.
  Proven against a real subprocess observed over a socket, with no PID polling
  and no sleeps.

- An identity Kronn does not recognize is refused instead of quietly becoming
  a Custom agent. Two database readers turned any unknown `agent_type` into
  `Custom`, and an unknown connection preset took the same road through
  `Other`, so corrupt data was routed as a working agent instead of being
  reported — on every read path that resolves message targets, session rows,
  the peer import and the model catalogue. Those readers now fail closed, the
  way the orchestration reader already did. Explicitly stored `Custom` and
  `Other` keep their established routing, and the error deliberately omits the
  offending value rather than echoing corrupt data back. The rule is written
  down in `docs/decisions.md`, where the next reader will look for it.

- A multi-agent debate can no longer reach a provider without the model check
  every other launch path applies. Orchestration called the runner directly for
  the summary, each participant round and the synthesis, so a model the
  catalogue positively marks as incompatible for that connection — an
  image/video-only one aimed at a text launch — went out anyway. Workflow Agent
  steps ran the check, but the call did not name the connection the step would
  actually use: the step could declare one and the check would still judge the
  agent's own catalogue, clearing a model that target refuses and refusing one
  it serves. A step now resolves its named connection once, fails closed when
  that connection is missing or carries a blank endpoint, and every dispatch
  it makes — normal, repair, escalation, author and reviewer — checks its own effective model
  against that target's catalogue before sending. A reviewer belonging to a
  different agent no longer inherits the author's connection, while a reviewer
  the step declares as sharing it still does. A refusal surfaces on the stream
  as an error instead of silence. And a debate held entirely between HTTP
  connections stops sweeping the machine for local CLI agents it will never
  launch.

- A delegated worker hears its own room again once its task is done. The server
  already moved the session back to the discussion that delegated the work, but
  the agent's bridge kept listening to the closed child room — and refused to
  switch on its own, rightly, since a bridge that changes rooms unprompted is
  worse than one that stops. It can now recover that single hop, and only that
  one: the server proves the exact terminal execution, that this session was
  its worker, and that the orchestrator wrote both return traces, before it
  hands the session back. A bridge deliberately bound to a third room is still
  refused, and a quiet wait no longer reports calm from a room the session has
  left — it follows the return and keeps listening where the work is.

- OpenCode gets a runnable Linux copy on a macOS host, like the agents it sits
  next to. Its Darwin binary cannot exec inside the Linux container, so Kronn
  skips it — but nothing installed a Linux one in its place, and the agent
  quietly fell back to the npx runtime, which re-pays a Node cold start on
  every session. It is now mirrored at container start like Claude, Codex,
  Gemini and Copilot: only when the user actually has it on their Mac, never
  reinstalled when a container copy already exists, and a missing or failing
  npm degrades with a diagnostic instead of stopping the container. The boot
  mirror also gained its first tests — the mechanism had produced two silent
  detection bugs before this one, and a parity check now names any agent left
  skipped without a replacement.

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
