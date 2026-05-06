# Désagentification — `StepType::ApiCall`

## What this is

A workflow step type that hits a Kronn-configured API plugin directly
from the Rust engine (zero tokens), extracts a field from the JSON
response via JSONPath, and pipes the value to the next step (another
ApiCall, an Agent, or a BatchQuickPrompt that fans out).

The vision: the agent stops doing `Bash curl` on APIs we already know
how to call. Every plugin-configured call becomes a first-class step
with typed extraction.

## Status (2026-04-26)

| Phase | Status |
|---|---|
| Phase 0 — Foundation (backend: models + extract + security + executor + endpoints) | ✅ **SHIPPED** (1280+ backend tests) |
| Phase 1 — Chartbeat vertical (wizard UI + starter template) | ✅ **SHIPPED** (836 frontend tests) |
| Phase 1b — GitHub vertical (hybrid MCP+REST, path placeholders, multi-config picker) | ✅ **SHIPPED** |
| Phase 1c — Jira/Atlassian vertical (Basic auth, templated base_url, JQL search) | ✅ **SHIPPED** (2026-04-26) |
| Phase 2 — Cloudflare (GraphQL kind, datetime trap, rate-limit governor) | 🔴 Not started |
| Phase 3 — Polish (dry-run before save, pitfall tips, log redaction UI) | 🟡 Partial (AI helper bubble, plugin tips registry, run-history snapshots done) |
| Phase 4 — Mobile read-only | 🔴 Not started |
| Phase 5 — Future (JMESPath opt-in, response cache, pagination walk) | 🟡 Partial |

## Known gaps carried into Phase 2

- **Rate-limit per-plugin** — scaffolding planned, not yet shipped.
  `governor::RateLimiter` keyed on `(plugin_slug, config_id)` in
  `AppState`. Defaults: Jira 8 req/s, Cloudflare ~3 req/s per
  `account_id`, Chartbeat unbounded. Without it, a batch QP over 50
  Jira issues will 429-storm. Tracked as **WU2**.
- **Pagination walk** — auto-detection shipped (Jira offset, CF cursor,
  Stripe `has_more`). Multi-page walk loop not yet. Tracked as **WU3**.
- **Wizard headers / body / method override** — fields exist on the
  model + runner, UI doesn't expose them. POST with a body still
  requires the raw API. Tracked as **WU4**.

## Context files (backend)

- `backend/src/models/mod.rs` — `StepType::ApiCall`, `ExtractSpec`,
  `PaginationSpec`, 12 flat `api_*` fields on `WorkflowStep`
- `backend/src/workflows/api_call_step.rs` — pure extraction + pagination
  shape detection (no HTTP)
- `backend/src/workflows/api_call_security.rs` — SSRF host allowlist,
  public-IP check with fast-path, `ResolvedAuth` redact, URL query redact
- `backend/src/workflows/api_call_executor.rs` — `execute_api_call_step_core`
  (no DB) + `execute_api_call_step_with_db` (DB + OAuth2 cache wrapper).
  `SecurityPolicy::production()` vs `::allow_loopback_for_tests()`
- `backend/src/api/workflows.rs` — `test_extract` + `test_api_call`
  handlers, `TestExtractRequest/Response`, `TestApiCallRequest/Response`
- `backend/src/workflows/runner.rs:178` — dispatch arm for `StepType::ApiCall`

## Context files (frontend)

- `frontend/src/components/workflows/ApiCallStepCard.tsx` — wizard card
  with 3 sub-components (`QueryParamsEditor`, `JsonTreeViewer`,
  `ExtractPanel`, `NextStepBanner`)
- `frontend/src/lib/workflow-templates/chartbeat-top5.ts` — starter
  template + `cloneTemplateSteps` helper + `assertTemplateInvariants`
- `frontend/src/components/workflows/WorkflowWizard.tsx` — renders
  `ApiCallStepCard` when `step.step_type.type === 'ApiCall'`, loads
  `availableApiPlugins` from `mcpsApi.overview()`, exposes "Load
  starter template" button

## Decisions tranchées (post 8-expert review)

| Question | Verdict | Why |
|---|---|---|
| Extraction DSL | **JSONPath via `serde_json_path` crate** | `jmespath-rs` abandoned 2022. RFC 9535 mature, same syntax as Jira/Cloudflare/Chartbeat docs. Wrap missing fns (`length()`) later if needed. |
| Struct shape | **Flat fields on `WorkflowStep`** | Mirrors `BatchQuickPrompt`. Single-row JSON storage, simpler migration + UI. |
| Test endpoints | **Two: `test-api-call` + `test-extract`** | Real-HTTP test for wizard + pure extract on a JSON sample so users can refine path without re-hitting the API. |
| Size-1 unwrap | **`$.total` → `42`, not `[42]`** | Users write `{{steps.X.data}}` and expect the scalar. Templates get unreadable otherwise. |
| Mobile | **Read-only in v1** | Editing an ApiCall step on <768px is a friction trap (long JSON, keyboard, preview). Trigger-and-monitor OK. |
| UI label FR | "🔌 **Récupérer des données**" | Verb-action. "ApiCall" stays in code. Extract = "Ce qui sort d'ici". Batch QP wiring = "Lance un prompt pour chaque élément" (not "fan-out"). |
| Wizard order | **endpoint → test-blank → params → retest → extract** | Tester avant de templater laisse voir la shape. Exception : endpoints that require params (Jira JQL). |

## Non-negotiables (kept through Phase 1)

1. **Click-to-pick** on the raw JSON response → path auto-populated in the
   focused field.
2. **Preview live** under every path input — resolved value (truncated)
   + detected type. Updates debounced 150 ms.
3. **Template workflow préfilé** as onboarding — Chartbeat top 5, one-click.
4. **Auto-pagination detection** in responses — silent page-1-only is a
   critical bug vector (Jira caps at 50).
5. **Security triple guard**:
   - SSRF: `base_url` never templated, host allowlist vs plugin spec,
     public-IP check with IP-literal fast path (`[::1]` blocks on IPv4 hosts)
   - Secret leak: manual `Debug` impl on `ResolvedAuth` with redact
     (`Bearer *** (len=42)`), query string redacted in error logs
   - Injection: body JSON rendered via `serde_json::Value` walk (never
     string interpolation), query params `percent-encode` after render

## Anti-patterns banned (seen from Zapier / Postman / n8n / Make)

- Custom JS test scripts (`pm.test(...)`) — barrière techno
- "Data structure" generation before a real test (Make.com)
- Modal full-screen expression editor for `{{var}}` (n8n) — inline only
- Hiding raw JSON behind "Show raw" (Zapier) — raw is the source of truth
- Pixel-perfect drag-drop mapping (Make.com) — fragile, inaccessible

## Phase 0+1 lessons learned (bugs encountered)

1. **`jmespath-rs` unmaintained** — caught in the tech-lead vs
   backend-expert trade-off. Switched to `serde_json_path` (RFC 9535).
2. **Size-1 unwrap critical for UX** — initial impl always returned
   an array. `$.total` → `[42]` is fine for code, terrible for template
   variables. Fixed, regression test guards it.
3. **Codex RTK hook detection path** — related: first-pass RTK
   detector looked at `.codex/config.toml` when RTK actually writes to
   `.codex/AGENTS.md`. Regression test added.
4. **Wiremock loopback vs SSRF guard** — `assert_public_ip` rightly
   blocks 127.0.0.1, but wiremock lives there. Fixed with explicit
   `SecurityPolicy::allow_loopback_for_tests()` — no silent `cfg(test)`
   bypass. The SSRF regression test keeps using `::production()`.
5. **`WorkflowRun` has no `project_id`** — runner dispatch uses the
   parent `workflow.project_id` instead. Also: the wizard loads
   plugins via `mcpsApi.overview()` at mount; every workflow-page test
   file needs the mock override or the wizard crashes.
6. **Wizard step dispatch already had `ApiCall` in the selector** — but
   zero body rendered. `P1.2b` wired the `<ApiCallStepCard>` in the
   ternary ladder between `BatchQuickPrompt` and `Notify`.
7. **Cross-host subdomain slip** — `evil.atlassian.net` must NOT match
   a plugin declaring `atlassian.net`. Regression test on
   `assert_host_matches_base`.
8. **IPv6 literal `[::1]` trapped on IPv4-only CI** — `tokio::net::lookup_host`
   fails without v6 connectivity, so the original guard returned
   `ResolutionFailed` instead of `PrivateOrLoopback`. Fixed with an
   IP-literal parse fast path (stripping the brackets) before DNS.

## Plugins landed

- **Chartbeat** (`api-chartbeat`): `apikey` query, flat response, no
  pagination. First vertical, template "top 5 → Résumé IA → Slack"
  cloneable.
- **Jira / Atlassian** (`mcp-atlassian`, hybrid, 2026-04-26): Basic
  auth (`JIRA_USERNAME` email + `JIRA_API_TOKEN`) on a templated
  `base_url` (`{JIRA_URL}` → `https://acme.atlassian.net`). 11
  endpoints curated for the backlog-ops use case: search (JQL),
  issue + comments + transitions, project search/single/components/
  versions, custom-field schema, saved filters. Required two backend
  changes: `ApiAuthKind::Basic` variant in `models/mod.rs` (composes
  `Authorization: Basic <b64>` from two env keys) and `base_url`
  interpolation in the executor (port of the `interpolate_env_template`
  used by the agent context block — single-brace `{ENV_KEY}` style,
  unresolved placeholder surfaces an actionable "fill it in
  Settings → APIs" error). Same MCP token doubles as REST credentials
  — user pastes the API token once, both Quick Prompts (MCP) and
  ApiCall workflows work. Tips registry covers JQL URL-encoding,
  customfield mapping, 401/403/404/400 semantics, and the
  `/search` → `/search/jql` migration (CHANGE-2046, 410 Gone since
  April 2025). The endpoint catalog ships the migrated path by
  default + `/search/approximate-count` for the count case.
  Tips were also re-keyed from `jira` to `mcp-atlassian` so the AI
  helper actually surfaces them (the lookup uses `server.id`, not
  the display name).
- **GitHub** (`mcp-github`, hybrid): same `GITHUB_PERSONAL_ACCESS_TOKEN`
  powers MCP (Quick Prompts via stdio) + REST API (ApiCall steps via
  Bearer auth). 13 curated endpoints (`/user`, issues, PRs, search,
  actions, releases, commits, notifications). Path placeholders
  (`{owner}/{repo}/{issue_number}`) are filled by the user in the
  wizard's editable endpoint combobox — the previous `<select>` was
  swapped for `<input + datalist>` to let the user type over the
  template after picking. Plugin tips registry (`mcp-github` slug)
  warns the AI helper about the placeholder pitfall, the issue/PR
  list overlap, the `q=` URL-encoding, and the SSO-required 403.

## Path placeholders + wizard validation polish (2026-04-26)

GitHub usage exposed three orthogonal gaps:

1. **Path placeholders** (`/repos/{owner}/{repo}/issues`) had no UI —
   the user was supposed to manually type-over the path. Now:
   - New `WorkflowStep.api_path_params: HashMap<String, String>` field
     (Rust + ts-rs auto-mirrored).
   - Executor helper `resolve_path_params` substitutes `{key}` tokens
     (mask-and-restore for `{{var}}` disambiguation, percent-encoding
     for path-segment safety, `{{steps.X.data}}` template expansion
     on the value).
   - Frontend `PathParamsEditor` (in `ApiCallStepCard.tsx`) renders
     one input per detected token with a live "URL résolue" preview
     below. Unresolved tokens flagged amber.
   - Round-trip safe: saved workflows keep BOTH the template and the
     values, so re-edit shows both.

2. **Validation rejected `Prompt missing for "main"` on ApiCall-only
   workflows** — `WorkflowWizard.tsx`'s last-step validator now
   branches per `step_type`: `ApiCall` checks `api_plugin_slug` +
   `api_config_id` + `api_endpoint_path`; `Notify` checks
   `notify_config.url`; the prompt-required path applies only to
   Agent / Custom steps. Three new i18n keys × FR/EN/ES.

3. **Test response viewer was showing the extracted value, not the
   raw JSON** — once the user picked an extract path (e.g.
   `$.toppages[*].path`), every subsequent "Test the call" would
   honour it server-side and return only the extracted array,
   breaking the click-to-pick UX on a second visit. The wizard now
   sends a copy of the step with `api_extract: null` to
   `/test-api-call`; the live preview (right panel) still resolves
   the path client-side via `/test-extract`.

## Multi-config plumbing (2026-04-26)

Two pitfalls that surfaced as soon as the user ran multiple credentials
for the same plugin (perso vs Euronews on GitHub):

1. **Picker collision** — `ApiCallStepCard.tsx` keyed `<option>` on
   `server.id`, so two configs pointing at `mcp-github` collapsed onto
   the same value. The picker now binds on `step.api_config_id` and
   uses `config.id` as the option value; the slug is derived from the
   matching config and BOTH ids are written on change. `selectedPlugin`
   prefers `api_config_id` lookup with a `api_plugin_slug` fallback for
   legacy steps. Regression test
   (`two configs of the same plugin are distinguishable in the picker`)
   locks it.

2. **Registry-vs-DB drift** — when the catalog gains an `api_spec` for
   an already-configured plugin (the GitHub case: user had configured
   it for MCP usage before we shipped `api_spec`), the existing
   `mcp_servers` row keeps `api_spec: None` and the wizard silently
   filters it out. Fixed by `db::mcps::sync_registry_servers_to_db`
   called from `main.rs` on startup: walks `builtin_registry()` and
   re-mirrors registry-managed fields (`api_spec`, `description`,
   `transport`) onto every existing row, leaving user-managed fields
   (env, label, project_ids on configs) untouched. Only updates
   EXISTING rows — never creates rows for plugins the user hasn't
   added. Test: `sync_registry_refreshes_api_spec_on_existing_rows_only`.

## Plugins planned (Phase 2)

- **Jira (Server / Data Center variant)**: shipped is Cloud-only
  (Basic `email:token` on `*.atlassian.net`). For self-hosted
  instances, switch to Bearer PAT — same `JIRA_URL` config_key,
  different auth variant. Detect via URL suffix at save time, OR
  add a second `mcp-atlassian-server` registry entry — TBD.
- **Jira workflow transitions** wrapped as one step (GET transitions
  by id + match-by-name + POST `/rest/api/3/issue/{key}/transitions`),
  removing the 2-call dance from agent prompts.
- **Cloudflare**: `operation_kind: rest | graphql` on a single plugin.
  Force API Token (refuse Legacy Key). GraphQL aggregation trap
  (datetime_gt < dataset resolution → silent empty) caught at save
  with an explicit error.
- **Rate-limit governor** (deferred from Phase 0): `governor::RateLimiter`
  keyed on `(plugin_slug, config_id)`. Defaults: Jira 8 req/s,
  Cloudflare ~3 req/s per `account_id`. Without it, a batch QP over
  50 Jira issues will 429-storm.

## Metrics of success

- ✅ One-click template "Chartbeat top 5 → Agent → Slack" runs
  end-to-end without manual configuration (cloneable from wizard)
- ✅ A user who's never seen JSONPath builds a working ApiCall step
  using only click-to-pick (Alex persona path, validated)
- ✅ No secret ever visible in `test-api-call` response, no secret in
  logs, no SSRF bypass in security test suite
- ✅ User configures GitHub once → both Quick Prompts (MCP) and
  ApiCall workflow steps work, no token re-entry
- ✅ User configures Jira once → JQL search step in workflow gets
  ticket keys, pipes to a BatchQuickPrompt that fans out per ticket
- ✅ Editing a workflow between runs preserves run-history fidelity
  (`StepResult` snapshots agent / plugin / endpoint at execution time)
- ✅ ApiCall step is **~1000× faster than the equivalent Agent step**
  (user-validated, 2026-04-27): a JQL search that took 60+ s burning
  tokens on an Agent step finishes in < 100 ms on ApiCall — the whole
  désagentification thesis in one observation.
- 🟡 Running a BatchQuickPrompt fed by an ApiCall on Jira (50 issues)
  under Jira's 429 threshold — blocked on rate-limit governor

## AI helper bubble (0.5.2)

Floating chat-bubble inside the ApiCall card. Click "Aide config IA",
pick any locally-installed agent, describe what you want — the agent
streams suggestions as `KRONN:APPLY` blocks that the UI renders as
inline cards with a one-click Apply button.

- File: `frontend/src/components/workflows/ApiCallAiHelper.tsx`
- Wired in `ApiCallStepCard.tsx` header (only when
  `installedAgents.length > 0`); threaded from `WorkflowWizard` via
  the existing `installedAgentTypes` prop.
- **Ephemeral by design**: `discussions.create({ archived: false, … })`
  on agent select, `discussions.delete(id)` on close + on unmount.
  No localStorage. Each helper session starts fresh.
- **System prompt** embeds the API spec (base URL + endpoints list) +
  a fresh "current context" snapshot (endpoint / method / query /
  headers / body / extract) + the last test result (success body
  truncated to 1.5 kB or error message verbatim). Agent is
  instructed to stay in French, ≤3 lines/message, and emit a
  `KRONN:APPLY` block followed by a fenced `json` blob with the
  fields it suggests changing.
- **Context re-injected on every user message**: `buildContextBlock`
  is called fresh at send time and prepended to the user's typed
  text before hitting `sendMessageStream`. Display vs payload are
  decoupled — the chat shows what the user typed, the agent sees
  `### CONTEXTE COURANT` + `### QUESTION DU USER`. This is what makes
  "j'ai changé l'extract path, regarde…" or "pourquoi j'ai un 400 ?"
  Just Work — the agent doesn't have to remember stale state from
  five messages ago.
- **Context chip** above the textarea echoes what's being attached
  (API name · endpoint · last test ✓/❌) so the user knows exactly
  what the agent will see.
- **Apply contract** (`KRONN_APPLY_RX`): only `endpoint`, `method`,
  `query`, `headers`, `body`, `extract` are accepted. `applyToStep`
  silently drops everything else — a hallucinating agent cannot
  rewrite `agent` or `prompt_template`. Regression test guards this.
- **Auth slots are stripped from suggestions**: when the selected
  plugin declares `ApiKeyQuery` / `ApiKeyHeader` / `Bearer` /
  `OAuth2ClientCredentials`, the matching key is removed from any
  `query` / `headers` block before applying. Source of truth:
  `frontend/src/components/workflows/apiCallAuth.ts` (`authSlotsForServer`,
  `stripManagedQuery`, `stripManagedHeaders`). Without this filter,
  agents kept proposing `apikey: 'VOTRE_API_KEY'` placeholders that
  would shadow the user's real configured key — caught from a real
  Chartbeat session. The system prompt also tells the agent
  explicitly via the `### AUTH` block.
- **Auth panel in the wizard** (`AuthManagedSlots` in
  `ApiCallStepCard.tsx`): green-tinted read-only fieldset above the
  query editor showing each managed slot as `<param-name>` ·
  `••••••••` · 👁 (toggle reveals the env key). Both pedagogical
  ("ta clé est déjà câblée, ne la remets pas") and the same
  invariant the AI helper enforces.
- **Suggestion card**: assistant prose is shown, then the fenced JSON
  is replaced by a structured card listing the field-by-field
  changes + one Apply button. Once applied, the card flips to
  "Appliqué ✓" (green border).
- **Streaming**: reuses `discussions.runAgent` for the first
  response, `discussions.sendMessageStream` for follow-ups. Abort
  via the `Stop` button or unmount; backend `discussions.stop` is
  also called best-effort.
- **Picker shortcut**: when only one agent is installed, the picker
  step is skipped — clicking the trigger jumps straight to the
  bubble. Multi-agent shows a popover with `agentColor` dots.
- **No project required**: the helper passes `project_id: null` to
  `discussions.create` when the wizard's project field is empty. The
  real context the agent needs is the API spec (selected plugin),
  not a project — and forcing project pick first would break the
  natural "open helper → ask which endpoint" flow.
- **Error surface**: stream + create errors show in the bubble
  footer; `console.error` mirrors them for DevTools diagnosis.
- i18n: 17 keys under `wf.apicall.helper.*` (FR/EN/ES, parity test
  green).
- Tests: `__tests__/ApiCallAiHelper.test.tsx` covers the field
  allowlist, single-agent shortcut, multi-agent picker, no-project
  pass-through, parser, context-block builder, plugin tips registry,
  trigger-disabled-when-no-API. Streaming flow not unit-tested (the
  `discussionsApi.runAgent` mock is exercised end-to-end on the
  branch — manual QA).
- **Trigger disabled when no API selected**: without `selectedServer`
  the agent has no spec, no endpoints, no auth — its advice degenerates
  to "go pick an API". Hard-disable + tooltip rather than letting the
  user open an empty helper.
- **Plugin tips registry** (`apiCallPluginTips.ts`): per-slug debugging
  lore (Chartbeat host pitfall, Jira 401/403/404 semantics, Cloudflare
  GraphQL datetime trap, Adobe Analytics rsid, Google Search/GSC quota
  + siteUrl format). Injected as a `### TIPS PLUGIN` section in the
  system prompt + the plugin's `docs_url` (or fallback) is offered for
  the agent to redirect to instead of hallucinating.
- **System prompt structure**: `# Rôle` / `# Ce que tu peux et ne peux
  PAS faire` (no MCP, no fetch, no "go check online") / `# Méthode de
  debug` (6-step ordered procedure) / `# Endpoints AUTORISÉS` /
  `# AUTH` / `# TIPS PLUGIN` / `# Doc officielle` /
  `### CONTEXTE COURANT` / `# Style`. The Style section explicitly
  tells the agent that `***` displayed in logs means masking ONLY —
  the real key IS sent — to stop the recurring "tu es sûr que la clé
  passe ?" dead-end where the agent keeps doubting the auth.

## Extract UX (post-Phase 1 wins)

The "right panel" of the card (where the user picks the JSONPath) was
the second friction wall. Three layered improvements turned it from
"learn JSONPath syntax" into "click the answer".

### 1. Click-to-pick everywhere, with DWIM wildcard

Every renderable atom in the JSON tree is clickable: object keys,
array index buttons (`[0]`, `[1]`…), array count markers (`[N]`),
and leaf values. The wiring lives in `JsonNode` (`ApiCallStepCard.tsx`)
and propagates two parallel path streams via props:

- `pathSegments` — the concrete path with `[0]`, `[1]`, … indices,
  used when the user picks a leaf VALUE (`"fr.euronews.com/"` →
  `$.toppages[0].path`, the specific item).
- `wildcardSegments` — the same path with the closest enclosing
  array's index swapped for `[*]`, used when the user picks a KEY
  (`path` inside `toppages[0]` → `$.toppages[*].path`, all items).

The DWIM rule is: **clé dans un tableau → tous les éléments ; valeur →
cet élément précis ; [N] → itérer ; [i] → cet objet précis**. This
matches the natural mental model — the user is *pointing at what they
want to extract*, not constructing a path. Regression tests in
`__tests__/ApiCallStepCard.test.tsx` lock both directions.

### 2. Smart suggestions chips (`apiCallSuggestions.ts`)

Pure function `suggestPaths(value)` walks the response (depth ≤ 3),
detects up to 6 actionable paths, and ranks them. Chips render above
the path input with the resolved sample preview:

- Array of objects → `Tous les "<scalar>"` (priority field from
  `id, key, name, title, path, url, slug, email, pseudo`, fall-back
  to first scalar in the item)
- Array → `Itérer sur les N éléments` (`$.foo[*]` for fan-out)
- Array → `Le 1er élément` (`$.foo[0]` for "tester avant fan-out")
- Numeric counter detection by name (`total`, `count`, `totalCount`,
  `total_count`, `length`)

Chips share i18n keys (`wf.apicall.suggest.*`) so the {0} / {1} args
get localised counts. Algorithm tested in
`__tests__/apiCallSuggestions.test.ts` (Chartbeat shape, Jira shape,
counter detection, fallback, MAX_SUGGESTIONS cap, dedup).

### 3. Deep one-line preview (`previewString`)

The path resolves through the `/test-extract` endpoint and the value
is rendered into a `<code>` chip. Old impl returned `Array(5)` /
`Object` — useless. New impl walks one level deep:

- Array of strings → `["a", "b", "c", … (+2)]` with each string
  truncated to 30 chars at depth 1.
- Array of objects → `[{…}, {…}, {…}]` — placeholders so the chip
  stays one-line; the JSON tree on the left is for deep inspection.
- Object → `{id: 42, title: "Hello", meta: {…}}` (top 3 keys + collapse
  for nested).
- Empty / null / scalar — direct stringify.

Exported from `ApiCallStepCard.tsx` for unit tests; the type pill on
the right (`array(5)`, `object`, …) stays as the canonical type
indicator from the backend.

### 4. Visual feedback when path is populated

A 600ms accent-coloured CSS pulse (`@keyframes wf-apicall-pulse`)
fires on the input when the path changes — the user knows the click
landed without scanning the form.

### 4b. Headers editor promoted out of Advanced

Real-world bug from a GitHub session: user applies a `User-Agent`
suggestion, chip flips to ✓ Appliqué, wizard shows nothing changed.
Root cause was the Headers editor living in the (collapsed-by-default)
"Advanced options" section — the apply DID land in `step.api_headers`,
the user just couldn't see it. Solved by moving the Headers
`KeyValueEditor` out of the advanced collapsible: it now renders
inline directly below the Query Params editor, always visible.
Headers are too commonly needed (User-Agent, X-API-Version, Accept)
to hide. Body / Method / Output var / Timeout / Retries stay in
Advanced (genuinely power-user). The auto-expand still applies for
those remaining fields: false → true transition only, sticky on
manual collapse. Toggle renamed to "Options avancées (body, méthode,
timeout, retries, output var)" so the mental model matches. A
discreet accent dot (`wf-apicall-advanced-dot`) signals "something
is set here" when the section is collapsed.

### 5. Test button derives `project_id` from the plugin config

`handleTest` in `ApiCallStepCard.tsx` falls back to
`config.project_ids[0]` when the wizard's project field is empty.
The friendlier error `wf.apicall.testNeedsProjectLink` (i18n FR/EN/ES)
takes over when the plugin config isn't linked to any project at all
— the user gets a clear "go to Settings → APIs and tick a project"
instead of the previous opaque "projectId missing".

## i18n keys added (cumulative for 0.5.2 work)

`wf.apicall.helper.*` (≈18 keys), `wf.apicall.authManaged*` (3),
`wf.apicall.suggest.*` (5), `wf.apicall.testNeedsProjectLink`,
`wf.apicall.pathParams*` (4), `wf.apicall.advancedToggle` updated,
`wf.apicall.endpointPlaceholder`, `wf.liveStep.*` (4), plus updated
`wf.apicall.clickToPickHint`. All FR/EN/ES; parity test in
`__tests__/i18n.test.ts` green.

## Run-history honesty (2026-04-26)

When the user edits a workflow between runs (swap the agent, retarget
the API plugin, change the endpoint), the run history would silently
start describing the *current* config rather than what actually ran.
Fix: snapshot the relevant fields onto `StepResult` at execution time.

### Backend

- New optional fields on `StepResult` (`models/mod.rs`):
  - `step_kind: Option<String>` — `"Agent" | "ApiCall" | "Notify" | "BatchQuickPrompt"`
  - `step_agent: Option<AgentType>` — only set for Agent steps
  - `step_api_plugin_slug: Option<String>` — only set for ApiCall
  - `step_api_endpoint_path: Option<String>` — only set for ApiCall
- Snapshot happens in `runner::execute_run` right before the
  outcome is pushed/emitted, so every executor path benefits without
  duplicating logic. Legacy rows (pre-snapshot) deserialise with
  `None` and the frontend renders nothing — graceful degradation,
  no crash.

### Frontend

- `RunDetail.tsx` displays a per-step badge derived from the snapshot
  rather than from the (possibly-edited) workflow:
  - `🔌 API mcp-github · /user`
  - `📤 NOTIFY hooks.slack.com`
  - `Codex` / `Claude Code` (agent steps)
  - `Layers BATCH` (batch fan-out)
- The `ts-rs` auto-mirroring picks up the new fields; manual update of
  `frontend/src/types/generated.ts` for the optional fields with the
  right TS shape.

## Live step status (2026-04-26)

The static `running...` placeholder for the in-flight step turned
into a guessing game ("is it stuck or just thinking?"). New
`LiveStepStatus` component in `RunDetail.tsx` ticks every second:

- Estimates step start time from `run.started_at + sum(durations of
  completed steps)`.
- Shows step-type-aware activity hint: *"Appel HTTP en cours"*,
  *"L'agent réfléchit"*, *"Envoi du webhook"*, *"Fan-out en cours"*.
- Tabular-num elapsed counter so digits don't jitter.
- 4 i18n keys (`wf.liveStep.*`) × FR/EN/ES.

Limitation: doesn't stream the agent's actual output (would require
re-plumbing SSE to the workflow run page). The compteur + activity
hint solve 90% of the "is it frozen?" anxiety without the heavy
infrastructure.

## RTK matrix + activation refresh (2026-04-26)

Two RTK fixes shipped alongside the workflow polish:

### `--codex` / `--gemini` activation matrix

Original code passed `rtk init -g --codex --auto-patch` which RTK
rejects with `--codex cannot be combined with --auto-patch`
(empirically — `--auto-patch` is the Claude-only "yes to settings.json
patch prompt" flag, the Codex/Gemini flows write a dedicated config
file with no prompt to auto-answer). Matrix now:

- Claude: `init -g --auto-patch --hook-only`
- Codex: `init -g --codex` (alone)
- Gemini: `init -g --gemini` (alone)
- Copilot CLI / Kiro / Vibe / Ollama / Custom: not supported by RTK
  upstream, returns `None` from `rtk_args_for`.

### `POST /api/rtk/deactivate`

Mirror of `/activate` with `--uninstall` appended per agent. Frontend
gains a discreet outline button "Désactiver RTK (N)" in
`CompressionSection` visible whenever ≥1 agent is configured.

### Badge refresh after activate

The `onActivated` parent refetch now runs in a `setTimeout(200)` from
the `finally` block — fires regardless of success/error and gives
RTK's filesystem writes time to flush before `agentsApi.detect()`
re-reads them. Without the deferral, re-detection raced the writes
and the badge stuck on the old state until a manual refresh.
