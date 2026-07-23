// ╔═══════════════════════════════════════════════════════════════════════════╗
// ║  AUTO-GENERATED — do not edit manually.                                     ║
// ║  Source: ts-rs bindings in backend/bindings/ (`#[derive(TS)]` on Rust      ║
// ║  models). Assembled by frontend/scripts/assemble-generated-types.mjs.       ║
// ║  Regenerate: `make typegen`. CI fails if this file drifts from the models.  ║
// ╚═══════════════════════════════════════════════════════════════════════════╝

/**
 * Result of adding a contact, with optional diagnostic hint for unreachable peers.
 */
export type AddContactResult = { contact: Contact,
/**
 * Human-readable hint explaining why the contact is pending (network mismatch, etc.)
 */
warning: string | null, };

export type AdoptHostMcpRequest = {
/**
 * Source file as reported by host_discovery (e.g. "/home/user/.claude.json").
 */
source_file: string,
/**
 * Scope from host_discovery so we can disambiguate the same MCP name
 * living in multiple Claude scopes (user vs local-per-project).
 */
scope: HostScope,
/**
 * Entry name as it appears in the host file.
 */
name: string, };

export type AgentApiCallRequest = {
/**
 * `KRONN_DISCUSSION_ID` of the disc making the call, when the
 * agent was spawned from a Kronn disc (auto-injected by the
 * runner). 0.8.6 — now optional: host-CLI sessions (regular
 * `claude` / `codex` etc. launched outside Kronn) don't have it
 * and would otherwise be locked out of the broker. Project scope
 * falls back to `project_id` explicit OR to `config.project_ids[0]`
 * derived from the chosen `api_config_id`. The disc-id path is
 * preferred when available — it links the call to a specific
 * conversation for future audit-log entries.
 */
disc_id?: string | null,
/**
 * 0.8.6 — explicit project scope override. Set this when the
 * agent knows the right project (e.g. from `mcp_list.configs[].
 * project_ids`) but doesn't have a disc id. Highest priority of
 * the 3 resolution sources (explicit > disc > config-derived).
 */
project_id?: string | null,
/**
 * Plugin slug — same value the agent sees in `mcp_list`'s
 * `servers_with_api[].id`. Either this+`api_config_id`, OR
 * `quick_api_id`, MUST be provided.
 */
api_plugin_slug?: string | null, api_config_id?: string | null, quick_api_id?: string | null,
/**
 * Endpoint path on the plugin's API. NOTE (2026-06-24): the declared
 * `ApiSpec.endpoints` are INDICATIVE, not an allow-list — the executor
 * does NOT reject undeclared paths; it forwards ANY path to the plugin's
 * base URL with auth injected (the declared list only drives method
 * resolution + display). So agents can call valid-but-undeclared
 * endpoints; the API itself is the real authority. The host-match +
 * public-IP `SecurityPolicy` is the actual guard, not the endpoint list.
 */
endpoint_path: string,
/**
 * HTTP method override. For a DECLARED path the method defaults to the
 * one in the plugin spec; for an UNDECLARED path it defaults to GET — so
 * a write (POST/PUT/…) on an undeclared path MUST set this explicitly.
 */
method?: string | null,
/**
 * Path-segment parameters (e.g. `/repos/{owner}/{repo}` →
 * `{"owner": "DocRoms", "repo": "Kronn"}`).
 */
path_params?: { [key in string]: string } | null,
/**
 * Query-string parameters (percent-encoded after rendering).
 */
query?: { [key in string]: string } | null,
/**
 * Extra headers (auth comes from the plugin spec, not here).
 */
headers?: { [key in string]: string } | null,
/**
 * JSON body for POST/PUT/PATCH (string leaves are templated).
 */
body?: JsonValue | null,
/**
 * JSON extract specification (same shape as workflow ApiCall).
 */
extract?: ExtractSpec | null, };

export type AgentApiCallResponse = {
/**
 * `true` when the HTTP call resolved to a 2xx AND the envelope
 * was successfully parsed. Maps to `status == "OK"`.
 */
success: boolean, duration_ms: number,
/**
 * The `data` field from the canonical envelope. `None` on
 * extract failure or transport errors.
 */
data: JsonValue | null,
/**
 * `"OK"` | `"ERROR"`.
 */
status: string,
/**
 * One-line summary suitable for the agent to echo back to the
 * user without dumping the full payload.
 */
summary: string,
/**
 * HTTP status code when the call reached the server. `None` for
 * transport-level errors (DNS, TLS, timeout before connect).
 */
http_status: number | null,
/**
 * Filled when `success == false`. Carries the human-readable
 * error from the executor so the agent can self-correct (wrong
 * endpoint, bad params, expired token, …).
 */
error: string | null, };

export type AgentConfig = { path: string | null, installed: boolean, version: string | null, full_access: boolean, };

/**
 * One decision row. Mirrors the `agent_decisions` table 1:1 — see
 * migration `051_agent_decisions.sql`.
 */
export type AgentDecision = { id: string, run_id: string, step_name: string, workflow_id: string, project_id?: string | null, ticket_ref?: string | null,
/**
 * `decided` | `mocked` | `blocked`. `clear` entries are NOT
 * persisted (trivial by definition).
 */
category: string, decision_id: string, what: string, chosen?: string | null,
/**
 * JSON array of strings (each string = one rejected option).
 */
options_json?: string | null, why?: string | null, placeholder?: string | null, strategy?: string | null, revisit_when?: string | null, needed_from?: string | null, workaround?: string | null,
/**
 * `pending` | `auto_approved` | `human_approved` | `overridden` | `resolved`.
 */
gate_status: string, override_value?: string | null,
/**
 * JSON array of `"file:line"` strings, populated by the drift
 * detector after the implement step runs.
 */
code_locations?: string | null, created_at: string, resolved_at?: string | null, };

export type AgentDetection = { name: string, agent_type: AgentType, installed: boolean, enabled: boolean, path: string | null, version: string | null, latest_version: string | null, origin: string, install_command: string | null, host_managed: boolean, host_label: string | null,
/**
 * Agent is runnable via npx/uvx fallback even when no local binary is found
 */
runtime_available: boolean,
/**
 * `rtk` binary found on the host (PATH). Same value for every agent
 * detection in a given sweep, but kept per-agent so the frontend can
 * render the state inline without a separate endpoint.
 */
rtk_available: boolean,
/**
 * The agent's own config file declares an RTK hook. Always `false` for
 * agents that have no shell-exec (API-only agents like Vibe) or no
 * hookable config (Ollama) — they're considered non-applicable.
 */
rtk_hook_configured: boolean,
/**
 * Optional i18n key for a runtime-degradation warning the frontend
 * should surface inline. Set per-agent at detect time.
 * Examples:
 *   - `"vibe.sdk_fallback"` — Vibe SDK signature mismatch detected
 *     (sentinel file present); the runner falls back to direct API
 *     mode, losing the local-tools (bash/file I/O) capability.
 *
 * `None` means "no degradation detected, agent is healthy".
 */
runtime_warning?: string | null, };

export type AgentProfile = { id: string, name: string, persona_name: string, role: string, avatar: string, color: string, category: ProfileCategory, persona_prompt: string, default_engine?: string | null, is_builtin: boolean,
/**
 * Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
 */
token_estimate: number, };

export type AgentProjectUsage = { project_id: string, project_name: string, tokens_used: number, message_count: number, };

export type AgentsConfig = { claude_code: AgentConfig, codex: AgentConfig, gemini_cli: AgentConfig, kiro: AgentConfig, vibe: AgentConfig, copilot_cli: AgentConfig, ollama: AgentConfig,
/**
 * Per-agent model tier overrides (Economy/Reasoning model names).
 */
model_tiers: ModelTiersConfig, };

export type AgentSettings = {
/**
 * Explicit model override (expert mode). Takes priority over tier.
 */
model?: string | null,
/**
 * Abstract tier selection. Resolved to a concrete --model flag per agent.
 */
tier?: ModelTier | null, reasoning_effort?: string | null, max_tokens?: number | null, };

export type AgentType = "ClaudeCode" | "Codex" | "Vibe" | "GeminiCli" | "Kiro" | "CopilotCli" | "Ollama" | "Custom";

export type AgentUsageSummary = { agent_type: string, total_tokens: number, message_count: number, by_project: Array<AgentProjectUsage>, };

export type AiAuditStatus = "NoTemplate" | "TemplateInstalled" | "Bootstrapped" | "Audited" | "Validated";

export type AiConfigStatus = { detected: boolean, configs: Array<AiConfigType>, };

export type AiConfigType = "ClaudeMd" | "ClauseDir" | "AiDir" | "CursorRules" | "ContinueDev" | "McpJson" | "Custom";

export type AiFileContent = { path: string, content: string, };

export type AiFileNode = { path: string, name: string, is_dir: boolean, children?: Array<AiFileNode>, };

export type AiSearchResult = { path: string, match_count: number, };

export type ApiAuthKind = { "ApiKeyQuery": { param_name: string, env_key: string, } } | { "ApiKeyHeader": { header_name: string, env_key: string, } } | { "Bearer": { env_key: string, } } | { "Basic": { user_env: string, password_env: string, } } | { "BasicApiKey": { env_key: string, } } | { "OAuth2ClientCredentials": { token_url: string, client_id_env: string, client_secret_env: string, scope: string, extra_headers?: Array<OAuth2ExtraHeader>, } } | { "TokenExchange": {
/**
 * Endpoint relative to the plugin's `base_url`. Examples:
 * `/sessions`, `/oauth/token`, `/v1/auth/exchange`.
 */
endpoint: string,
/**
 * HTTP method — typically `POST`, occasionally `PUT`.
 */
method: string,
/**
 * Request body template. String leaves support `${ENV.KEY}`
 * substitution from the decrypted env (e.g. `"${ENV.API_KEY}"`).
 * Non-string leaves pass through as-is.
 * Didomi example:
 * ```json
 * {"type": "api-key", "key": "${ENV.API_KEY}", "secret": "${ENV.API_SECRET}"}
 * ```
 */
body_template: JsonValue,
/**
 * Body serialization format on the wire.
 */
body_format: TokenExchangeBodyFormat,
/**
 * JSONPath to extract the token from the response. Examples:
 * `$.access_token`, `$.data.token`, `$.session.bearer`.
 */
token_jsonpath: string,
/**
 * Cached-token TTL in seconds. Kronn refreshes at T-30s
 * safety margin. `0` disables caching (re-exchange every call,
 * only useful for testing).
 */
ttl_seconds: number,
/**
 * How to inject the resulting token into subsequent calls on
 * THIS plugin's endpoints.
 */
inject: TokenInjection,
/**
 * Defensive: env_keys the spec needs the user to fill. Empty
 * is permitted but the form/validator can use this to flag
 * missing creds before the exchange fires.
 */
creds_env_keys?: Array<string>, } } | "None";

export type ApiCallLog = { id: string, source: string, project_id: string | null, run_id: string | null, disc_id: string | null, agent: string | null, plugin_slug: string, config_id: string | null, endpoint_path: string, method: string, http_status: number | null, status: string, duration_ms: number, request_excerpt: string | null, response_excerpt: string | null, error_message: string | null, called_at: string, };

export type ApiCallSource = "workflow" | "agent_broker" | "manual_test";

export type ApiCallStatus = "OK" | "ERROR" | "RateLimited" | "TimedOut";

/**
 * A non-secret parameter the plugin instance needs (e.g. host, workspace id).
 * Stored in the same encrypted env blob as the API key, but the UI renders
 * these as plain inputs (no mask). Prompts may reference them symbolically;
 * the broker resolves values server-side when it builds full URLs.
 */
export type ApiConfigKey = { env_key: string, label: string, placeholder: string, description: string, };

export type ApiEndpoint = { path: string,
/**
 * `"GET"`, `"POST"`, etc. Kept free-form to avoid constraining agents
 * that want to call a rare verb.
 */
method: string, description: string, };

export type ApiKey = { id: string, name: string, provider: string, active: boolean, };

export type ApiKeyDisplay = { id: string, name: string, provider: string, masked_value: string, active: boolean, };

export type ApiKeysResponse = { keys: Array<ApiKeyDisplay>, disabled_overrides: Array<string>, };

/**
 * REST API capability for a plugin.
 *
 * Stored on `McpServer` to let a plugin expose an HTTP API alongside (or
 * instead of) an MCP transport. The value is serialized into the
 * `mcp_servers.api_spec_json` column (migration 035) and reused by the
 * prompt-injection path that emits `=== AVAILABLE APIs ===` blocks.
 */
export type ApiSpec = { base_url: string, auth: ApiAuthKind,
/**
 * Short list of the most useful endpoints. Not exhaustive — the UI
 * surfaces `docs_url` for the full reference. Agents may call
 * undocumented endpoints if they know the path; this list is
 * primarily a hint + curl example.
 */
endpoints: Array<ApiEndpoint>,
/**
 * URL of the vendor's API reference documentation.
 */
docs_url?: string | null,
/**
 * Additional config keys the user must provide on top of the credential
 * (e.g. Chartbeat's `host=example.com`). Stored alongside the secret
 * in the config's encrypted env and surfaced to agents only as
 * `${ENV.KEY}` broker references — never as literal prompt values.
 */
config_keys?: Array<ApiConfigKey>, };

export type AppConfig = { server: ServerConfig, tokens: TokensConfig, scan: ScanConfig, agents: AgentsConfig,
/**
 * Output language used by agents when they write their replies.
 * Separate from `ui_language` below which controls the Kronn UI locale.
 */
language: string,
/**
 * UI language (FR/EN/ES) for the React frontend. Persisted here so a
 * Tauri WebView2 localStorage wipe doesn't reset the user's choice
 * every time the app updates or Windows rotates the WebView2 profile.
 * Frontend still writes to localStorage as a fast-path + fallback when
 * the backend is unreachable.
 */
ui_language: string,
/**
 * Persistent STT model choice (e.g. "onnx-community/whisper-tiny").
 * None = first-launch default / user never set it.
 */
stt_model?: string | null,
/**
 * Persistent TTS voice choices, keyed by output language code
 * ("fr" → "voice-id-fr", "en" → "voice-id-en", …).
 */
tts_voices?: Record<string, string>, disabled_agents: Array<AgentType>, };

/**
 * Compact lint feedback echoed to the POSTING agent (tool result), so it can
 * self-correct unverifiable `[src:]` citations in its next message. The full
 * report rides the stored message (UI badge), same as streaming replies.
 */
export type AppendLintSummary = { fabricated_count: number, unsourced_count: number, note: string, };

/**
 * Declared artifact in a workflow. Phase-3 minimal model — only
 * path + optional format hint. Path is resolved relative to the run's
 * workspace; absolute paths and `..` traversal are rejected at
 * validate-time (`validate_artifact_specs`).
 */
export type ArtifactSpec = {
/**
 * Workspace-relative path (e.g. `.kronn/plan.md`).
 */
path: string,
/**
 * Hint for the UI — `"markdown"`, `"yaml"`, `"json"`, `"text"` —
 * informational only, the engine doesn't enforce a format.
 */
format?: string | null, };

export type AuditFileInfo = { path: string, filled: boolean, };

export type AuditInfo = { files: Array<AuditFileInfo>, todos: Array<AuditTodo>, tech_debt_items: Array<TechDebtItem>, };

/**
 * 0.8.2 — Specialized audit types ("Design C").
 *
 * `Full` exposes the canonical 9-step foundation; a launched Full audit
 * appends 7 focused dimensions for a 16-step chain. The other variants run
 * one focused dimension. They share the
 * reconciliation + audit_runs row machinery; only the step list differs.
 *
 * `Custom` is the escape hatch: the caller supplies a free-form prompt
 * (single step). All variants are wired through `kind_to_steps()` in
 * `api::audit`.
 */
export type AuditKind = "Full" | "Drift" | "Security" | "Docker" | "Performance" | "Accessibility" | "Rgaa" | "Database" | "ApiDesign" | "CodeQuality" | "Custom";

/**
 * Live progress of a running audit, exposed via `GET /api/projects/:id/audit-status`.
 *
 * Produced by the SSE streams (`full_audit`, `partial_audit`)
 * which write into `AppState.audit_tracker.progress` as they advance. The UI
 * polls this endpoint to "resume" the progress bar when the user navigates
 * away and comes back — no need to restart the audit since the server-side
 * process keeps running.
 *
 * The struct is deliberately thin: it carries what's needed to paint a
 * progress bar, not the full audit content (that still flows through SSE
 * when the user is actively connected).
 */
export type AuditProgress = { project_id: string,
/**
 * `"installing"` during template install, `"auditing"` during the dynamic chain
 * loop, `"validating"` during phase 3 (validation discussion creation),
 * `"done"` briefly before the tracker clears the entry.
 */
phase: string, step_index: number, total_steps: number,
/**
 * `ai/` file currently being produced (e.g. `"repo-map.md"`), or
 * `"Final review"` for the last step, or `None` between steps.
 */
current_file?: string | null, started_at: string,
/**
 * `"full"` for the chained audit, `"partial"` for drift-triggered
 * sub-audits, `"full_audit"` for the end-to-end variant. Kept as a
 * string so future audit kinds don't force a schema migration.
 */
kind: string,
/**
 * 0.8.3 — live chips state surfaced via the poll endpoint, NOT
 * just via SSE. Solves the case where the SSE stream stalls or
 * buffers (nginx, agent freeze, page re-mount): the frontend
 * polls `/api/audit-status` every few seconds and re-seeds the
 * chips from these fields. Optional so the JSON shape stays
 * backwards-compatible with old clients.
 */
step_tokens?: number | null, total_tokens_so_far?: number | null, current_tool?: string | null,
/**
 * 0.8.4 (#319 / B3) — running count of `tool_call` events the
 * agent has fired DURING the current step. Reset on every
 * `step_start`. Surfaced as a chip after the tool name (e.g.
 * `🔧 Write (14)`) so the user has a "still alive" signal even
 * when the token chip is frozen (heavy step writing many TD
 * files without intermediate `Usage` blocks — the symptom that
 * confused the user during the 8-min Step 8 of the Full audit).
 */
current_tool_call_count?: number | null, };

/**
 * Recommendation emitted by the completion-time cluster detector. Lives in
 * `AuditRun.recommendations_json` as a JSON-encoded list.
 */
export type AuditRecommendation = {
/**
 * The specialized audit kind to suggest.
 */
kind: string,
/**
 * Why this kind is recommended — surfaced in the UI tooltip.
 */
reason: string,
/**
 * Number of TDs that drove the recommendation (the cluster size).
 * Used to rank multiple recommendations.
 */
cluster_size: number, };

/**
 * One row in the `audit_runs` table — one record per audit invocation.
 *
 * Inserted at audit start with `status = Running` and zeroed counts;
 * updated to a terminal status with populated counts when the pipeline
 * finishes. The frontend health badge reads the latest N rows for a
 * project to render the sparkline + delta chip.
 *
 * 0.8.2 — see migration 050 for schema. The `kind` field is forward-
 * compatible: we ship with `Full` only and extend to `Security`,
 * `Docker`, etc. in S2 without touching this struct.
 */
export type AuditRun = { id: string, project_id: string,
/**
 * `Full` | `Drift` | `Security` | `Docker` | `Performance` |
 * `Accessibility` | `Database` | `ApiDesign` | `Custom`.
 * Kept as String for forward-compat (new variants don't break
 * rows already on disk).
 */
kind: string, agent_type: string, started_at: string, ended_at?: string | null, duration_ms?: number | null,
/**
 * `Running` while in flight; `Completed` / `Failed` / `Cancelled` /
 * `Interrupted` once terminal. `Interrupted` (0.8.3 #311) means
 * the SSE stream ended before the executed chain completed, without an
 * explicit cancel — typically a rate-limit, claude crash, or
 * network blip. The frontend treats `Interrupted` specifically:
 * it shows a dynamic resume button for `last_completed_step + 1`
 * instead of a fresh "Lancer".
 */
status: string,
/**
 * 0.8.3 (#311) — last successfully completed step (1-based,
 * matches the executed step-chain indexing). 0 = no step done yet.
 * A chained Full currently completes at 16. Set on every `step_done` where
 * `validate_step_output` returns success=true. Drives
 * the resume mechanism: on resume we start at `this + 1`.
 */
last_completed_step: number,
/**
 * 076 — durable link to the validation discussion created in the SAME
 * transaction as the Completed status. The validate endpoint trusts
 * only this, never title/date heuristics.
 */
validation_discussion_id?: string | null,
/**
 * 076 — structured per-step outcomes (requested/succeeded/unchanged)
 * for partial runs; provenance for the drift oracle.
 */
step_outcomes_json?: string | null, td_critical: number, td_high: number, td_medium: number, td_low: number, td_total: number, td_resolved_since_last: number, td_new_since_last: number, td_carried_over: number,
/**
 * 0-100 health score computed by `compute_health_score` at the
 * moment of completion. `None` while `status == Running`.
 */
health_score?: number | null,
/**
 * Relative path under the project root, e.g.
 * `docs/tech-debt/_reconciliation-2026-05-13.md`.
 */
report_path?: string | null,
/**
 * Raw JSON string of `Vec<AuditRecommendation>`, populated by the
 * completion-time cluster detector (Full audits only). Kept as String
 * in the model to avoid forcing schema migrations on every
 * recommendation-shape tweak.
 */
recommendations_json?: string | null, };

/**
 * 0.8.4 (#298) — Per-step metrics for the post-audit recap panel.
 *
 * One row per step per `audit_runs` row. Inserted at `step_start`
 * (only the started-at + file_label fields are populated), finalized
 * at `step_done` (ended_at + duration_ms + tokens + cli_success), and
 * decorated by `step_warning` (#292) when the step's output doesn't
 * look right.
 *
 * The frontend ProjectCard reads `GET /api/audit-runs/:run_id/steps`
 * for a collapsed "▾ Détails du dernier audit" panel; the table is
 * sortable by `duration_ms` and `step_tokens` so the user can spot
 * the heaviest step at a glance.
 */
export type AuditRunStep = { audit_run_id: string, step_index: number, file_label: string, started_at: string, ended_at?: string | null, duration_ms?: number | null, step_tokens?: number | null, cumulative_tokens?: number | null,
/**
 * `false` when the CLI exited non-zero OR `step_warning` fired.
 */
cli_success: boolean, step_warning?: string | null,
/**
 * Mirrors the `step_warning.repaired` field from #292.
 */
step_repaired_from_template: boolean, };

export type AuditTodo = { file: string, line: number, text: string, };

/**
 * Auto-trigger regex buckets declared in a skill's frontmatter YAML.
 *
 * ```yaml
 * auto_triggers:
 *   common:
 *     - "\\b(pdf|docx?|xlsx?)\\b"
 *   fr:
 *     - "génér.+(fichier|rapport)"
 *   en:
 *     - "generate.+(file|report)"
 * ```
 *
 * The frontend combines `common` + the entry matching the discussion
 * language (or `en` as fallback) into a single regex list, and tests
 * every pattern against the pending message.
 */
export type AutoTriggers = { common?: Array<string>,
/**
 * Per-locale patterns keyed by IETF language tag (`fr`, `en`, `es`,
 * ...). Additional locales can be added without a code change.
 */
locales?: Record<string, string[]>, };

/**
 * 0.6.0 — payload for `POST /api/quick-apis/:id/batch`. Fan-out the same
 * QA over a list of items (sub-domains, ticket keys, languages, etc.)
 * without needing a workflow. Mirror of the `BatchApiCall` step type
 * but standalone — uses the same parallel HTTP executor under the hood.
 */
export type BatchRunQuickApiRequest = {
/**
 * Items to fan-out over. Accepts:
 *   - JSON array of strings (each fills the QA's first variable):
 *     `["www.example.com", "de.example.com", "fr.example.com"]`
 *   - JSON array of objects (each key maps to a variable name):
 *     `[{"host":"www.example.com","limit":"5"}, ...]`
 */
items: JsonValue,
/**
 * Max parallel HTTP calls (default 5, hard-capped at 20).
 */
concurrent_limit?: number | null, };

/**
 * Response from `POST /api/quick-apis/:id/batch`. The full aggregated
 * envelope produced by the BatchApiCall executor — the frontend renders
 * `envelope.data.items[]` as a per-item result table.
 */
export type BatchRunQuickApiResponse = {
/**
 * Overall status: `OK` (all succeeded), `PARTIAL` (some failed), `ERROR` (all failed).
 */
status: string, duration_ms: number, envelope: JsonValue | null, error: string | null, };

/**
 * Compact summary of a batch workflow run, with its parent linear run
 * resolved to a human-friendly (workflow name + run sequence number) label.
 * Consumed by the discussion sidebar to render a clickable pastille on each
 * batch group ("↗ run #3 de Recap hebdo") so users can trace a batch back
 * to the workflow that spawned it.
 */
export type BatchRunSummary = { run_id: string, batch_name: string | null, batch_total: number, status: RunStatus,
/**
 * Id + name of the Quick Prompt that this batch fans out. Resolved from
 * the batch run's virtual `qp:<id>` workflow_id prefix. Used by the
 * sidebar as the batch folder label (instead of the first child disc's
 * title, which is just one ticket id among N and misleads users).
 */
quick_prompt_id: string | null, quick_prompt_name: string | null, quick_prompt_icon: string | null,
/**
 * Parent linear workflow run id (None for top-level manual batches).
 */
parent_run_id: string | null,
/**
 * Name of the workflow that spawned this batch, resolved at query time.
 * None when the parent run was deleted or this is a manual batch.
 */
parent_workflow_id: string | null, parent_workflow_name: string | null,
/**
 * 1-based position of the parent linear run among all runs of that
 * workflow (ordered by started_at). None for manual batches.
 */
parent_run_sequence: number | null, };

export type BootstrapProjectRequest = { name: string, description: string, agent: AgentType, mcp_config_ids?: Array<string>, skill_ids?: Array<string>, };

export type BootstrapProjectResponse = { project_id: string, discussion_id: string, };

/**
 * One **child workflow** declared inside a bundle (2026-06-11). The
 * parent workflow's `SubWorkflow` step references it via
 * `sub_workflow_id: "@bundle:<bundle_id>"`; the server creates the
 * child FIRST (so its real id exists before the parent's step is
 * substituted) and the child inherits the parent's `project_id` when
 * it doesn't set its own (so linked_repos / project MCPs / the
 * `[TRIAGE]` addendum apply inside the child run — see
 * `docs/design/decomposed-autopilot-presets.md` INV-3).
 */
export type BundleChildWorkflow = { bundle_id: string, name: string, project_id: string | null, trigger: WorkflowTrigger, steps: Array<WorkflowStep>, actions: Array<WorkflowAction>, safety: WorkflowSafety | null, workspace_config: WorkspaceConfig | null, concurrency_limit: number | null, guards: WorkflowGuards | null, artifacts: Record<string, ArtifactSpec>, on_failure: Array<WorkflowStep>, exec_allowlist: Array<string>, variables: Array<PromptVariable>,
/**
 * 0.8.5 — optional initial state. Default `true` for back-compat
 * (every UI-driven create stays enabled by default). The MCP
 * `workflow_create_draft` tool sets this to `false` so an
 * agent-spawned workflow lands in the user's Workflows page in a
 * disabled state, ready for review + manual enable. Avoids the
 * "agent just created a cron workflow that fires unattended"
 * failure mode while still letting agents accelerate the
 * adoption of Kronn by drafting common patterns autonomously.
 */
enabled?: boolean | null, };

/**
 * One artifact that was created by the bundle endpoint. The
 * `bundle_id` is the placeholder the caller used; the `id` is the
 * real DB id the artifact now lives at.
 */
export type BundleCreated = { bundle_id: string, id: string, name: string, };

/**
 * One Custom API plugin declared inside a bundle. The wrapped
 * `payload` mirrors what `POST /api/mcps/configs` accepts for the
 * `Custom API` flow (`materialize_custom_server` consumes it).
 */
export type BundleCustomApi = { bundle_id: string, name: string, base_url: string, description?: string, docs_url?: string | null,
/**
 * 0.8.6 — auth scheme. `ApiAuthKind: Default = None`, so the field
 * is back-compat for any payload that omits it (pre-0.8.6 Custom
 * plugins keep working unchanged). When set, the materialized
 * `ApiSpec.auth` carries it instead of hardcoding `None` — which
 * is what made Custom plugins muets côté auth pre-fix (caught
 * 2026-05-19 on Didomi audit).
 */
auth: ApiAuthKind,
/**
 * List of `{label, value}` pairs. The backend slugifies each label
 * into an `env_key` (UPPER_SNAKE_CASE) and stores the value in the
 * encrypted env blob alongside the rest.
 */
fields?: Array<CustomApiField>,
/**
 * 0.8.6 — endpoints the user (often via the `CustomApiAiHelper`
 * fetching the docs) wants declared on this plugin. Without these,
 * the executor's allowlist refuses any agent-driven ApiCall — so
 * declaring them at create time is what flips `mcp_list`'s hint
 * from `NEEDS_RESEARCH` to `READY`. Blank-path entries are
 * silently dropped at materialize time. Each entry: `{path,
 * method, description}` (matches the existing `ApiEndpoint`
 * shape).
 */
endpoints?: Array<ApiEndpoint>, };

/**
 * One Quick API declared inside a bundle.
 */
export type BundleQuickApi = { bundle_id: string, name: string, icon: string | null, description: string, project_id: string | null, api_plugin_slug: string, api_config_id: string, api_endpoint_path: string, api_method: string | null, api_query: { [key in string]: string } | null, api_path_params: { [key in string]: string } | null, api_headers: { [key in string]: string } | null, api_body: JsonValue | null, api_extract: ExtractSpec | null, api_pagination: PaginationSpec | null, api_timeout_ms: number | null, api_max_retries: number | null, variables: Array<PromptVariable>, profile_ids: Array<string>, directive_ids: Array<string>, };

/**
 * One Quick Prompt declared inside a bundle. The `bundle_id` is the
 * placeholder used by `@bundle:<id>` references in the workflow
 * JSON; the wrapped `request` is the same payload `/api/quick-prompts`
 * expects.
 */
export type BundleQuickPrompt = { bundle_id: string, name: string, icon: string | null, prompt_template: string, variables: Array<PromptVariable>, agent: AgentType | null, project_id: string | null, skill_ids: Array<string>, profile_ids: Array<string>, directive_ids: Array<string>, tier: ModelTier, agent_settings: AgentSettings | null, description: string, };

/**
 * Top-level bundle payload. Every section is optional except
 * `workflow` — the bundle is anchored on its workflow. An empty
 * bundle (no QP/QA/CustomAPI, just a workflow) is valid and
 * behaves like a regular `POST /api/workflows`.
 */
export type BundleRequest = { quick_prompts?: Array<BundleQuickPrompt>, quick_apis?: Array<BundleQuickApi>, custom_apis?: Array<BundleCustomApi>,
/**
 * Child workflows created before the parent (2026-06-11). Referenced
 * from the parent's `SubWorkflow` step via `@bundle:<bundle_id>` on
 * `sub_workflow_id`. Cycle / depth / no-gate are validated against the
 * in-memory bundle graph + existing DB workflows.
 */
child_workflows?: Array<BundleChildWorkflow>, workflow: CreateWorkflowRequest, };

/**
 * Response payload from `POST /api/workflows/bundle`. Each section
 * mirrors the request's section so the frontend can show "Created N
 * QPs / M QAs / K Custom APIs / 1 Workflow".
 */
export type BundleResponse = { quick_prompts: Array<BundleCreated>, quick_apis: Array<BundleCreated>, custom_apis: Array<BundleCreated>,
/**
 * Child workflows created before the parent (2026-06-11).
 */
child_workflows: Array<BundleCreated>,
/**
 * The workflow doesn't have a `bundle_id` (only one per bundle);
 * the frontend uses `id` + `name` to navigate to it.
 */
workflow: BundleWorkflowCreated, };

export type BundleWorkflowCreated = { id: string, name: string, };

/**
 * Body of `POST /api/disc/claim-by-token`. A PEER calls this to ask "do you
 * host the room behind this invite code?". Authenticated by `from_invite_code`
 * matching one of our contacts — the same self-auth credential as the WS
 * Presence handshake (so this endpoint is exempt from the bearer middleware).
 */
export type ClaimByTokenRequest = { token: string,
/**
 * The CALLING peer's own invite code — must match a known contact here.
 */
from_invite_code: string, };

export type ClaimByTokenResponse = {
/**
 * True iff WE host the room behind `token`; then we've shared it back.
 */
found: boolean, shared_id: string | null, title: string | null, };

export type CleanupOrphanEnvRequest = { keys: Array<string>, };

export type CleanupOrphanEnvResponse = { configs_updated: number, total_keys_removed: number, };

/**
 * Body for `POST /api/projects/:id/clone-and-remap` — re-clone a project's
 * `repo_url` locally and re-point the existing project at the clone. Used to
 * recover projects whose path no longer resolves after a cross-machine DB
 * import (e.g. WSL `/home/...` paths on macOS).
 */
export type CloneAndRemapRequest = {
/**
 * Optional parent directory to clone into. When omitted the server picks
 * a sensible existing location (common parent of on-disk projects →
 * `KRONN_REPOS_DIR` → first existing scan path).
 */
parent_dir?: string | null, };

export type CloneAndRemapResponse = { project_id: string,
/**
 * The local path the project now points at (where the repo was cloned).
 */
new_path: string, };

export type CloneProjectRequest = { url: string, name?: string | null, agent: AgentType, };

export type CloneProjectResponse = { project_id: string, discussion_id: string | null, };

export type ConditionAction = { "type": "Stop" } | { "type": "Skip" } | { "type": "Goto", step_name: string, max_iterations?: number | null, };

export type Contact = { id: string, pseudo: string, avatar_email: string | null, kronn_url: string, invite_code: string, status: string, created_at: string, updated_at: string, };

/**
 * A file uploaded as context for a discussion.
 * Content is extracted to text at upload time and stored in DB.
 */
export type ContextFile = { id: string, discussion_id: string, filename: string, mime_type: string, original_size: number, extracted_size: number, disk_path: string | null,
/**
 * The message this file is attached to. `None` = pending (still staged in
 * the composer) or a legacy disc-wide file. Always serialized (even when
 * null) so the frontend can split pending-vs-attached without ambiguity.
 */
message_id: string | null, created_at: string, };

export type CreateDirectiveRequest = { name: string, description: string, icon: string, category: DirectiveCategory, content: string, conflicts?: Array<string>, };

export type CreateDiscussionRequest = { project_id?: string | null, title: string, agent: AgentType, language?: string, initial_prompt: string, skill_ids?: Array<string>, profile_ids?: Array<string>, directive_ids?: Array<string>, workspace_mode?: string | null, base_branch?: string | null,
/**
 * Model capability tier (economy / default / reasoning).
 */
tier?: ModelTier,
/**
 * 0.8.5 — when this discussion is being spawned by a Quick Prompt
 * launch (single, batch, or compare-agents path that bypasses
 * `create_batch_run`), the originating QP id. The backend
 * resolves the current version_index and stamps both on the
 * `discussions` row so the metrics aggregator can group.
 * `None` = not a QP launch (briefing / manual / etc.).
 */
originating_qp_id?: string | null,
/**
 * F9 — create a "human-only" disc: the agent runner never spawns on
 * `send_message`. Used by the contact-click → 1:1 human↔human chat flow.
 */
no_agent?: boolean, };

export type CreateMcpConfigRequest = { server_id: string, label: string, env: Record<string, string>, args_override?: Array<string> | null, is_global: boolean, project_ids: Array<string>,
/**
 * Custom API plugin payload. Only honoured when `server_id == "api-custom"`.
 * The backend materializes a new `McpServer` (API-only, `source = Manual`)
 * from these fields, then proceeds with the normal config-creation path.
 * Auth type is always `None` for custom plugins; the agent reads the
 * description + docs_url + fields and figures out auth itself.
 */
custom_spec?: CustomApiPayload | null, };

export type CreateProfileRequest = { name: string, persona_name?: string, role: string, avatar: string, color: string, category: ProfileCategory, persona_prompt: string, default_engine?: string | null, };

export type CreateQuickApiRequest = { name: string, icon?: string | null, description?: string, project_id?: string | null, api_plugin_slug: string, api_config_id: string, api_endpoint_path: string, api_method?: string | null, api_query?: { [key in string]: string } | null, api_path_params?: { [key in string]: string } | null, api_headers?: { [key in string]: string } | null, api_body?: JsonValue | null, api_extract?: ExtractSpec | null, api_pagination?: PaginationSpec | null, api_timeout_ms?: number | null, api_max_retries?: number | null, variables?: Array<PromptVariable>, profile_ids?: Array<string>, directive_ids?: Array<string>, };

export type CreateQuickPromptRequest = { name: string, icon?: string | null, prompt_template: string, variables?: Array<PromptVariable>, agent?: AgentType | null, project_id?: string | null, skill_ids?: Array<string>, profile_ids?: Array<string>, directive_ids?: Array<string>, tier?: ModelTier, agent_settings?: AgentSettings | null, description?: string, };

export type CreateSkillRequest = { name: string, description: string, icon: string, category: SkillCategory, content: string, license?: string | null, allowed_tools?: string | null, };

export type CreateWorkflowRequest = { name: string, project_id?: string | null, trigger: WorkflowTrigger, steps: Array<WorkflowStep>, actions?: Array<WorkflowAction>, safety?: WorkflowSafety | null, workspace_config?: WorkspaceConfig | null, concurrency_limit?: number | null, guards?: WorkflowGuards | null, artifacts?: Record<string, ArtifactSpec>, on_failure?: Array<WorkflowStep>, exec_allowlist?: Array<string>, variables?: Array<PromptVariable>,
/**
 * 0.8.5 — optional initial state. Default `true` for back-compat
 * (every UI-driven create stays enabled by default). The MCP
 * `workflow_create_draft` tool sets this to `false` so an
 * agent-spawned workflow lands in the user's Workflows page in a
 * disabled state, ready for review + manual enable. Avoids the
 * "agent just created a cron workflow that fires unattended"
 * failure mode while still letting agents accelerate the
 * adoption of Kronn by drafting common patterns autonomously.
 */
enabled?: boolean | null, };

export type CustomApiField = { label: string, value: string, };

/**
 * Free-form spec for a user-defined API plugin (the "Custom API" flow).
 * Captured from the frontend form; the backend turns it into an
 * `ApiSpec` + `McpServer` pair on submit.
 */
export type CustomApiPayload = { name: string, base_url: string, description?: string, docs_url?: string | null,
/**
 * 0.8.6 — auth scheme. `ApiAuthKind: Default = None`, so the field
 * is back-compat for any payload that omits it (pre-0.8.6 Custom
 * plugins keep working unchanged). When set, the materialized
 * `ApiSpec.auth` carries it instead of hardcoding `None` — which
 * is what made Custom plugins muets côté auth pre-fix (caught
 * 2026-05-19 on Didomi audit).
 */
auth: ApiAuthKind,
/**
 * List of `{label, value}` pairs. The backend slugifies each label
 * into an `env_key` (UPPER_SNAKE_CASE) and stores the value in the
 * encrypted env blob alongside the rest.
 */
fields?: Array<CustomApiField>,
/**
 * 0.8.6 — endpoints the user (often via the `CustomApiAiHelper`
 * fetching the docs) wants declared on this plugin. Without these,
 * the executor's allowlist refuses any agent-driven ApiCall — so
 * declaring them at create time is what flips `mcp_list`'s hint
 * from `NEEDS_RESEARCH` to `READY`. Blank-path entries are
 * silently dropped at materialize time. Each entry: `{path,
 * method, description}` (matches the existing `ApiEndpoint`
 * shape).
 */
endpoints?: Array<ApiEndpoint>, };

export type DailyUsage = { date: string, tokens: number, cost_usd: number, anthropic: number, openai: number, google: number, mistral: number, amazon: number, github: number, };

/**
 * Result of a `POST /api/db/backup` call. The frontend surfaces the
 * `backup_path` so the user can copy it (or the absolute `.bak` ref
 * if they want to script around it).
 */
export type DbBackupResponse = {
/**
 * Absolute path of the backup file written.
 */
backup_path: string,
/**
 * Size of the backup in bytes (sanity check + UI display).
 */
size_bytes: number,
/**
 * Timestamp the backup was taken (ISO-8601 UTC).
 */
taken_at: string, };

export type DbExport = { version: number, exported_at: string, projects: Array<Project>, discussions: Array<Discussion>, workflows: Array<Workflow>, mcp_servers: Array<McpServer>, mcp_configs: Array<McpConfig>, custom_skills: Array<Skill>, custom_directives: Array<Directive>, custom_profiles: Array<AgentProfile>, contacts: Array<Contact>, quick_prompts: Array<QuickPrompt>,
/**
 * 0.8.9 — Quick APIs (reusable saved API calls). `#[serde(default)]` keeps
 * v3 exports (which had no `quick_apis` field) importable.
 */
quick_apis: Array<QuickApi>,
/**
 * 0.8.9 — Continual-learning candidates (the agent-proposed durable
 * facts/preferences). Same back-compat default as `quick_apis`.
 */
learnings: Array<Learning>,
/**
 * v5 (passe D) — QP version history; without it, imports silently lost
 * the version metrics lineage. `default` keeps v4 exports importable.
 */
quick_prompt_versions: Array<QuickPromptVersion>,
/**
 * v5 (passe D) — anti-repetition rejection counters for learnings.
 */
learning_rejections: Array<LearningRejection>, };

export type DbInfo = { size_bytes: number, project_count: number, discussion_count: number, message_count: number, mcp_count: number, workflow_count: number, workflow_run_count: number, custom_skill_count: number, custom_profile_count: number, custom_directive_count: number, };

/**
 * 0.7.0 Phase 4 — payload for `POST /api/workflows/:id/runs/:run_id/decide`.
 *
 * `decision` is one of `"approve" | "request_changes" | "reject"`.
 * `comment` is optional in general but the frontend enforces a non-empty
 * value for `request_changes` (the agent needs feedback to act on).
 */
export type DecideRunRequest = { decision: string, comment?: string | null,
/**
 * Optional gate identity (step name). When set, the decision only
 * applies if the run is currently waiting on THAT gate — protects a
 * stale caller (e.g. the auto-approve timer armed on gate A) from
 * deciding a later gate B it never saw. 2026-06-10 audit P1.
 */
gate_step?: string | null, };

/**
 * Response for [`decide_run`].
 */
export type DecideRunResponse = { run_id: string, new_status: RunStatus, };

/**
 * A detected network IP with its type.
 */
export type DetectedIp = { ip: string,
/**
 * "tailscale", "vpn", or "lan"
 */
kind: string, label: string, };

export type DetectedRepo = { path: string, name: string, remote_url: string | null, branch: string, ai_configs: Array<AiConfigType>, has_project: boolean, hidden: boolean, };

export type Directive = { id: string, name: string, description: string, icon: string, category: DirectiveCategory, content: string, is_builtin: boolean, conflicts?: Array<string>,
/**
 * Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
 */
token_estimate: number,
/**
 * Optional URL to the source project — set on directives that adapt
 * third-party prompts (e.g. Caveman → github.com/JuliusBrussee/caveman).
 * Surfaces as a small "↗ Source" link in the settings card. MIT-licensed
 * adaptations should include this for attribution.
 */
source_url?: string | null, };

export type DirectiveCategory = "Output" | "Language";

/**
 * One message in a `disc_append` payload. `source_msg_id` is REQUIRED
 * because it's how the dedup pass works — without it we'd duplicate
 * every message on every reconnect.
 */
export type DiscAppendMessage = { source_msg_id: string, role: MessageRole, content: string, agent_type?: AgentType | null, };

export type DiscAppendRequest = { disc_id: string, messages: Array<DiscAppendMessage>,
/**
 * Calling bridge session. New bridges always send this so heartbeat and
 * activity cleanup cannot affect a sibling of the same agent type.
 */
session_id?: string | null, };

export type DiscAppendResponse = { appended: number, skipped_as_duplicates: number,
/**
 * When true, the disc has been edited inside Kronn since the
 * last import — the caller should warn the user before pushing
 * MORE messages (they might be applying stale state on top).
 */
diverged: boolean,
/**
 * Present only for a live single Agent append whose lint had a signal.
 */
lint?: AppendLintSummary,
/**
 * `sort_order` of the LAST appended message (stab-1). Long-polling
 * callers must pass it as `since_sort_order` instead of estimating
 * their position — estimates drift under concurrent posters and made
 * agents silently skip messages. `None` when nothing was appended.
 */
last_sort_order?: number, };

/**
 * Body of `POST /api/disc/create`. The triple `(source_agent,
 * source_session_id, project_id)` is enough to disambiguate: if a
 * disc already exists for the (agent, session) pair, we return its
 * id instead of creating a duplicate.
 */
export type DiscCreateRequest = { title: string, agent: AgentType, language?: string | null, project_id?: string | null,
/**
 * When set, the new disc is immediately bound to this
 * (source_agent, source_session_id) pair.
 */
source_agent?: string | null, source_session_id?: string | null, };

export type DiscCreateResponse = { disc_id: string,
/**
 * `true` when a fresh row was inserted; `false` when an existing
 * disc was returned because (source_agent, source_session_id)
 * already mapped.
 */
created: boolean, };

export type DiscFindBySessionQuery = { source_agent: string, source_session_id: string, };

export type DiscFindBySessionResponse = { disc_id: string | null, };

export type DiscLinkRequest = { disc_id: string, source_agent: string, source_session_id: string, };

export type DiscLoadOtherMessage = { idx: number, role: MessageRole, content: string, agent_type: AgentType | null, timestamp: string,
/**
 * Files attached to this message (0.8.8). Mirrors `disc_get_message` so a
 * cross-disc reader can discover an image's `disk_path` and open it with
 * its file tools — without this, an agent browsing ANOTHER disc only sees
 * the text and is blind to the attached images. Empty for most messages.
 */
attachments?: Array<MessageAttachment>, };

export type DiscLoadOtherQuery = { disc_id: string, from?: number | null, to?: number | null, };

export type DiscLoadOtherResponse = { disc_id: string, title: string, total_messages: number, from_idx: number, to_idx: number, messages: Array<DiscLoadOtherMessage>, };

export type DiscoveredHostMcp = { source_file: string, scope: HostScope, name: string, transport: McpTransport,
/**
 * Env variable **names** declared by the entry. Values are never
 * surfaced through this struct (even though the file already exposes
 * them locally) so the API response is safe to log.
 */
env_keys: Array<string>, managed_by_kronn: KronnOwnership, };

export type DiscoveredKey = { provider: string, source: string, suggested_name: string, already_exists: boolean, };

export type DiscoverKeysResponse = { discovered: Array<DiscoveredKey>, imported_count: number, };

export type DiscoverReposRequest = { source_ids?: Array<string>, };

export type DiscoverReposResponse = { repos: Array<RemoteRepo>, sources: Array<string>, available_sources: Array<RepoSource>, errors: Array<DiscoverSourceError>, };

/**
 * 0.8.7 — per-source failure surfaced to the user (GitLab silently
 * returning 0 repos because the token expired was the trigger ; the
 * front-end now renders a chip with the error so the user knows WHY
 * a source produced no results, instead of guessing the integration
 * is broken).
 */
export type DiscoverSourceError = { source_id: string, source_label: string, provider: string, message: string, };

/**
 * LIKE-based full-text search across disc titles + message content.
 * Cheap-and-cheerful (no FTS5 wiring): finds the N most recent discs
 * where title OR any message content matches `%q%` (case-insensitive
 * — SQLite's LIKE is CI on ASCII by default; for non-ASCII queries
 * the user just adds wildcards).
 *
 * Returns (disc_id, title, snippet) tuples — snippet is the first
 * 80 chars of the first matching message body, or the title if the
 * match was on the title.
 */
export type DiscSearchHit = { disc_id: string, title: string, snippet: string, source_agent: string | null, source_session_id: string | null, };

export type DiscSearchQuery = { q: string, limit?: number | null, };

/**
 * Snapshot of every disc currently bound to a source. Used by the
 * frontend sidebar to decorate items with a "from X" badge without
 * having to query per-disc. Returns `(disc_id, source_agent,
 * source_session_id, imported_at, diverged_at)` tuples for every
 * disc where `source_agent IS NOT NULL`.
 */
export type DiscSourceBinding = { disc_id: string, source_agent: string, source_session_id: string, imported_at: string | null, diverged_at: string | null, };

export type DiscSourceDetail = { current: DiscSourceBinding | null, history: Array<DiscSourceHistoryEntry>, };

/**
 * One row of source-binding history. `unlinked_at IS NULL` ⇒
 * currently bound. The frontend renders these in a tooltip so the
 * user can see the full "this disc was first owned by ClaudeCode
 * session X, then Cursor session Y" chain.
 */
export type DiscSourceHistoryEntry = { source_agent: string, source_session_id: string, linked_at: string, unlinked_at: string | null, };

export type DiscUnlinkRequest = { disc_id: string, };

export type Discussion = { id: string, project_id: string | null, title: string, agent: AgentType, language: string, participants: Array<AgentType>, messages: Array<DiscussionMessage>, message_count: number,
/**
 * Subset of `message_count` excluding `MessageRole::System` rows. The
 * streaming layer persists every tool call + every cached-summary
 * breadcrumb as its own System message, so `message_count` is inflated
 * from the user's point of view ("2 réponses + 50 outils" comptait 52).
 * The unread badge tracks this count instead, so System breadcrumbs
 * don't show up as "messages à lire".
 */
non_system_message_count: number, skill_ids?: Array<string>, profile_ids?: Array<string>, directive_ids?: Array<string>, archived: boolean,
/**
 * User-pinned / favorite discussion — appears in a dedicated "Favorites"
 * section at the top of the sidebar regardless of project grouping.
 */
pinned: boolean, workspace_mode: string, workspace_path?: string | null, worktree_branch?: string | null,
/**
 * Model capability tier for this discussion.
 */
tier: ModelTier,
/**
 * 0.8.10 — explicit model override for this discussion (e.g. inherited
 * from the Quick Prompt that launched it). Wins over `tier` at run time
 * (threaded to the agent as `model_override`). `None` = resolve from tier.
 */
model?: string | null,
/**
 * Pin the first message (protocol prompt) — always include it in agent prompts, never summarize it.
 * Used for validation, bootstrap, and briefing discussions.
 */
pin_first_message: boolean,
/**
 * Cached summary of older messages (eco-design: avoids re-sending full history).
 */
summary_cache?: string | null,
/**
 * Index of the last message included in summary_cache (0-based).
 */
summary_up_to_msg_idx?: number | null,
/**
 * How summaries are produced for this discussion. See `SummaryStrategy`
 * for the semantics. Default `Auto` keeps the historical behaviour
 * (per-agent thresholds with auto-fire after every reply).
 */
summary_strategy: SummaryStrategy,
/**
 * Cumulative count of `kronn-internal` tool calls made by the agent
 * on this discussion. Bumped each time `disc_meta`, `disc_get_message`
 * or `disc_summarize` is hit. Surfaced in the ChatHeader as a small
 * "🔧 N" pill so the user can see when the agent is actively
 * querying its history.
 */
introspection_call_count: number,
/**
 * Shared discussion UUID (None = local-only, Some = replicated with peers).
 */
shared_id?: string | null,
/**
 * Contact IDs this discussion is shared with.
 */
shared_with?: Array<string>,
/**
 * ID of the batch WorkflowRun that spawned this discussion, if any.
 * Used for sidebar grouping under the project ("Cadrage to-Frame — 10 avr").
 * Null for manual discussions created outside of a batch workflow.
 */
workflow_run_id?: string | null,
/**
 * The disc is owed an agent run that hasn't produced a durable trace yet
 * (queued batch child, or a reply in flight). DB-backed so the sidebar's
 * "en file" state survives navigation, reloads and missed WS frames.
 */
awaiting_agent: boolean,
/**
 * Test mode — branch the main repo was on before the user entered test
 * mode. `Some` means the user is actively testing this discussion's
 * branch in their main repo; `None` means normal worktree operation.
 * Used by `test-mode/exit` to checkout back to the user's prior state.
 */
test_mode_restore_branch?: string | null,
/**
 * Test mode — if the main repo was dirty at enter time and the user opted
 * in to auto-stash, this holds the stash message (e.g.
 * `kronn:auto-<disc_id>`) so `exit` can pop the exact stash.
 * `None` when the main repo was clean or the user declined the stash.
 */
test_mode_stash_ref?: string | null, created_at: string, updated_at: string, };

export type DiscussionMessage = { id: string, role: MessageRole, content: string, agent_type: AgentType | null, timestamp: string, tokens_used: number, auth_mode: string | null,
/**
 * Which model tier was used for this message (economy/default/reasoning).
 */
model_tier?: string | null,
/**
 * 0.8.10 — the CONCRETE model this message ran on (e.g. "qwen3:32b",
 * "sonnet"), resolved via `runner::effective_model_flag` at commit time.
 * A discussion can switch models mid-thread, so this is per-message, not
 * per-discussion. `None` = legacy row or a provider-default run with no
 * explicit model flag (Codex/Gemini at default tier) → UI falls back to
 * `model_tier` / the agent name.
 */
model?: string | null,
/**
 * Cost in USD (real from Claude Code, estimated for other providers)
 */
cost_usd?: number | null,
/**
 * Author identity (for multi-user / display)
 */
author_pseudo?: string | null, author_avatar_email?: string | null,
/**
 * 0.8.4 (#294) — when this message came from a CLI transcript
 * import, the source-side message id. Used by `disc_append` to
 * dedupe re-pushes of the same exported transcript. NULL = native
 * Kronn message (created via the UI / API, not imported).
 */
source_msg_id?: string | null,
/**
 * 0.8.5 — wall-clock duration of the agent reply, in milliseconds.
 * Captured by the streaming layer (delta between agent run start
 * and message commit). NULL on User / System messages and on
 * legacy rows (pre-migration 057). Used by the QP-metrics
 * aggregator to compute "avg first-reply duration" per QP version.
 */
duration_ms?: number | null,
/**
 * 0.8.7 anti-hallucination P2 — the lint report for this agent message
 * (niveau 0 heuristic + niveau 1 mechanical `[src:]` verification),
 * computed by `core::anti_halluc::analyze` at finalize. `None` on
 * User/System messages, when the feature is off, or when nothing flagged.
 */
lint_report?: LintReport | null, };

/**
 * Lean surrounding-message shape for a `disc_get_message` window.
 */
export type DiscussionMessageContextItem = { idx: number, id: string, message_ref: string, role: MessageRole, content: string, agent_type: AgentType | null, timestamp: string, };

/**
 * Single-message read shape — same fields as the underlying
 * `DiscussionMessage` minus internal-only metadata.
 */
export type DiscussionMessageRead = { idx: number, id: string,
/**
 * Stable compact reference accepted by `disc_get_message`.
 */
message_ref: string, role: MessageRole, content: string, agent_type: AgentType | null, timestamp: string, tokens_used: number,
/**
 * Files attached to this message (0.8.8). Lets an agent that navigates to
 * an old message see what was uploaded with it instead of being blind to
 * a discussed image. Empty for messages with no attachments.
 */
attachments?: Array<MessageAttachment>,
/**
 * Optional compact context requested by the caller. Attachments stay on
 * the target message only so a small window does not fan out DB reads.
 */
before?: Array<DiscussionMessageContextItem>, after?: Array<DiscussionMessageContextItem>, };

/**
 * Compact metadata returned by `disc_meta` — everything the agent might
 * need to decide whether to fetch context, without leaking the full
 * transcript. `tokens_used_total` is the cumulative billed token count
 * for the discussion (sum of every message's `tokens_used`).
 */
export type DiscussionMeta = { id: string, title: string, agent: AgentType, tier: ModelTier, message_count: number, tokens_used_total: number, summary_strategy: SummaryStrategy, has_cached_summary: boolean,
/**
 * 0-indexed position of the last message included in
 * `summary_cache`. `None` means no summary has been generated yet.
 */
summary_up_to_msg_idx: number | null,
/**
 * Number of non-system messages added since the cached summary was
 * last refreshed. Lets the agent gauge whether the summary is fresh
 * enough to trust.
 */
msgs_since_last_summary: number, language: string,
/**
 * Long-poll pacing contract for multi-agent rooms (stab-1).
 */
poll_policy: PollBackoffPolicy,
/**
 * stab-3 — current server-computed regime for this disc.
 */
pacing?: PacingState, project_id: string | null, };

/**
 * A row of `discussion_sessions` — one live (or historical)
 * binding between a disc and a CLI session.
 */
export type DiscussionSession = { id: number, disc_id: string, agent_type: string, session_id: string | null, role: string, status: string, joined_at: string, left_at: string | null,
/**
 * Last activity heartbeat (migration 064): bumped on every `disc_append`.
 * Surfaced so the UI can show presence freshness — "active" vs "silent
 * (working)" vs "away" — instead of a binary present/absent that makes a
 * quietly-working peer look like a dead room. `None` for rows predating 064.
 */
last_seen: string | null,
/**
 * 0.8.12 PR B — server-derived activity placeholder: `"listening"`
 * (open wait long-poll) or `"reading"` (messages delivered, no reply
 * yet). Expiry is applied at read time — an expired activity is None
 * here, callers never see a stale placeholder.
 */
activity: string | null, };

export type DriftCheckResponse = { audit_date: string | null, stale_sections: Array<DriftSection>, fresh_sections: Array<string>, total_sections: number, };

export type DriftSection = { ai_file: string, audit_step: number, changed_sources: Array<string>, };

/**
 * One piece of evidence backing a claim. `kind` mirrors the citable source
 * types; `reference` is the resolvable ref (file:line / url / disc-id / cmd /
 * user:date); `quote` is the supporting excerpt (the NL premise the Gate-2
 * checker scores against).
 */
export type Evidence = { kind: string, ref: string, quote?: string | null, };

/**
 * Gate-1 verdict for one evidence row (rendered per-row in the validation modal).
 */
export type EvidenceCheck = { reference: string, status: string, fabricated: boolean, };

export type ExecResponse = { stdout: string, stderr: string, exit_code: number, };

/**
 * Extraction specification for an `ApiCall` step's JSON response.
 * Implements RFC 9535 JSONPath via the `serde_json_path` crate.
 */
export type ExtractSpec = {
/**
 * JSONPath expression, e.g. `$.issues[*].key` or `$.data.viewer.zones[0].zoneTag`.
 * Evaluated against the full response (after pagination concat if enabled).
 */
path: string,
/**
 * Default value when the path resolves to nothing. Keeps workflows
 * alive on empty results. Omit = `null`.
 */
fallback?: JsonValue | null,
/**
 * If true and the extraction returns `null` / empty array, the step
 * emits the `NO_RESULTS` signal (status stays `Success`) so `on_result`
 * conditions (Skip, Stop, Goto) can branch on "the API returned
 * nothing". 2026-06-10: default flipped to **true** — a silent empty
 * extraction was a recurring footgun (the step looked OK while
 * downstream `{{steps.X.data}}` was empty). Existing rows that
 * serialized `false` explicitly keep their behaviour (no migration);
 * only specs that omit the field (new / AI-generated) get the safer
 * default. Set `false` deliberately when an empty result is normal.
 */
fail_on_empty: boolean, };

/**
 * Gate-2 faithfulness verdict (`claim ⊨ evidence`). NULL when the checker is
 * `off`. Posture B: informative only — surfaced to the human, never auto-blocks.
 */
export type Faithfulness = "entailment" | "neutral" | "contradiction";

/**
 * Body of `POST /api/disc/fetch-file` (F8). A peer that received a
 * `FileAttached` announcement calls this to pull the binary of a context file
 * it doesn't have. Authenticated by `from_invite_code` matching a contact
 * (same trust model as `claim-by-token`).
 */
export type FetchFileRequest = { file_id: string, from_invite_code: string, };

export type FetchFileResponse = { found: boolean, filename: string | null, mime_type: string | null,
/**
 * Base64-encoded file bytes (None when not found). Base64 keeps the
 * transport a simple JSON envelope; the peer decodes + writes to disk.
 */
data_base64: string | null, };

/**
 * One flagged sentence: a short excerpt + the cue that tripped the heuristic.
 */
export type FlaggedSpan = {
/**
 * A char-bounded excerpt of the flagged sentence (≤ [`SPAN_EXCERPT_MAX_CHARS`]).
 */
text: string,
/**
 * The claim cue that matched (helps the user judge the flag + tune later).
 */
reason: string, };

export type GitBranchRequest = { name: string, };

export type GitBranchResponse = { branch: string, };

export type GitCommitRequest = { files: Array<string>, message: string, amend?: boolean, sign?: boolean, };

export type GitCommitResponse = { hash: string, message: string, };

export type GitDiffQuery = { path: string,
/**
 * When true, return the COMMITTED diff for this path (`<default>...HEAD`)
 * instead of the working-tree diff. Used by the GitPanel "committed on
 * branch" section, where the working tree is clean so a plain `git diff`
 * would be empty.
 */
committed?: boolean | null, };

export type GitDiffResponse = { path: string, diff: string, };

export type GitFileStatus = { path: string, status: string, staged: boolean, };

export type GitPushResponse = { success: boolean, message: string, };

export type GitStatusResponse = { branch: string, default_branch: string, is_default_branch: boolean, files: Array<GitFileStatus>,
/**
 * Files committed on this branch but not yet on default branch.
 * Empty when on the default branch or when no default branch resolves.
 * Lets the "Fichiers" panel surface the disc's cumulative work
 * (what would land in the next merge), not just the uncommitted slice.
 */
committed_files: Array<GitFileStatus>, ahead: number, behind: number, has_upstream: boolean, provider: string, pr_url: string | null, };

/**
 * Which guard tripped — surfaced verbatim in the SSE `GuardTriggered`
 * event so the frontend can render the right badge / toast / explainer.
 */
export type GuardKind = { "type": "Timeout" } | { "type": "MaxLlmCalls" } | { "type": "LoopDetection", step_name: string, };

export type HostScope = { "kind": "ClaudeUser" } | { "kind": "ClaudeLocal", "value": { project_path: string, } } | { "kind": "Gemini" } | { "kind": "Codex" } | { "kind": "Copilot" };

/**
 * How a config should be surfaced to the local CLIs (Claude Code, Gemini,
 * Codex, Copilot) when they run *outside* a Kronn-managed project.
 *
 * Separated from `is_global` (which is Kronn-internal: "applied across all
 * Kronn projects") because the two concepts answer different questions:
 * `is_global` decides Kronn project visibility; `host_sync` decides
 * whether Kronn writes the entry into `~/.claude.json` & friends.
 */
export type HostSyncMode = "None" | "GlobalOnly" | "MirrorAll";

/**
 * 0.6.0 — payload for `POST /api/quick-apis/import`. Mirrors the QP shape.
 */
export type ImportQuickApiRequest = { content: string, project_id?: string | null, };

/**
 * 0.7.0 UX pass — payload for `POST /api/quick-prompts/import`.
 */
export type ImportQuickPromptRequest = { content: string, project_id?: string | null, };

export type ImportResult = { warnings: Array<string>, invalid_paths: Array<string>, };

/**
 * 0.7.0 UX pass — payload for `POST /api/workflows/import`.
 * `content` is the raw JSON of a `WorkflowExportEnvelope` (string
 * rather than nested object so the frontend can `JSON.parse` once
 * from the dropped file and pass it through). `project_id` is the
 * importer's project to attach the workflow to — `None` keeps it
 * unattached (the user picks a project later via the wizard).
 */
export type ImportWorkflowRequest = { content: string, project_id?: string | null, };

/**
 * Wire shape returned by the invite endpoint. The frontend displays
 * `instruction_text` directly in the copy-paste modal — the wording
 * lives server-side so we can tweak it (i18n, channel, etc.) without
 * shipping a frontend release.
 */
export type InviteResponse = { token: string, disc_id: string, expires_at: string, ttl_seconds: number, instruction_text: string, };

/**
 * Metadata returned by `create_invite_token`. The plain `token` field
 * is the ONLY place the plaintext value ever lives outside the agent
 * that's about to consume it — never logged, never persisted.
 */
export type InviteTokenIssued = { token: string, disc_id: string, expires_at: string, };

/**
 * One invite-token row, hash-only. Returned by audit-style queries
 * (who joined via which invite). Never carries plaintext.
 */
export type InviteTokenRecord = { id: number, disc_id: string, created_at: string, expires_at: string, used_at: string | null, used_by_session_id: number | null, };

/**
 * Result of an atomic invite-token consumption + session creation.
 * Used by the `POST /api/discussions/peer-join` endpoint that
 * the `disc_join` MCP tool calls.
 */
export type JoinViaTokenResult = { disc_id: string, session_pk: number,
/**
 * Plain resume credential, returned once. Only its SHA-256 hash is stored.
 */
resume_token: string, };

export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;

export type KronnOwnership = { "type": "NotManaged" } | { "type": "ManagedByMarker", "config_id": string } | { "type": "ManagedByHash", "config_id": string };

export type LaunchAuditRequest = { agent: AgentType,
/**
 * 0.8.2 — Specialized audit type. Omitted/null defaults to `Full`
 * for backwards-compat (the only kind the UI knows about pre-0.8.2).
 */
kind?: AuditKind | null,
/**
 * 0.8.2 — Caller-provided free-form prompt; only honored when
 * `kind == AuditKind::Custom`. Ignored otherwise.
 */
custom_prompt?: string | null,
/**
 * Resume an interrupted run. The server loads this `audit_runs` row,
 * verifies it belongs to the project and is `Interrupted`, then derives
 * BOTH the kind and the checkpoint (`last_completed_step`) from the row
 * — `kind`/`custom_prompt` above are ignored when this is set. This
 * makes resume impossible to misuse: no client-supplied step count to
 * oversize, and no way to graft a checkpoint onto the wrong pipeline.
 */
resume_run_id?: string | null, };

/**
 * A continual-learning candidate (table `learnings`).
 */
export type Learning = { id: string, claim: string, evidence: Array<Evidence>, kind: LearningKind, status: LearningStatus, scope?: LearningScope | null, confidence?: number | null, faithfulness?: Faithfulness | null, discussion_id?: string | null, project_id?: string | null, source_agent?: string | null, promoted_target?: string | null, created_at: string, last_validated_at?: string | null, validated_by?: string | null, };

/**
 * Type of a learning. Bound to anti-hallu `SourceKind` at the gate
 * (spec §5): `fact` needs a Verified file/url, `preference` a dated user
 * confirmation, `inference` is Unchecked → never auto-extracted to a truth
 * file without double validation.
 */
export type LearningKind = "fact" | "preference" | "inference";

export type LearningProposeRequest = { claim: string, evidence: Array<Evidence>, kind: LearningKind, discussion_id?: string | null, project_id?: string | null, source_agent?: string | null, confidence?: number | null, };

/**
 * One row of `learning_rejections` — the anti-repetition counter keyed by
 * claim hash. Exported/imported with the DB (passe D: losing it reset the
 * auto-reject threshold after a migration).
 */
export type LearningRejection = { claim_hash: string, reason: string, count: number, last_at: string, };

/**
 * Where a validated learning is routed (spec §7). `User` → `~/.kronn/
 * user-context/learnings.md`; `Project` → `docs/learnings.md`. NULL until the
 * scope router runs at validation time.
 */
export type LearningScope = "user" | "project";

/**
 * Lifecycle of a learning candidate.
 */
export type LearningStatus = "pending" | "validated" | "rejected" | "stale" | "promoted" | "promoting";

/**
 * A companion repository linked to a project. The `location` is
 * either a filesystem path (preferred — gives the agent direct
 * read access) or a URL (GitHub/GitLab — the agent still gets it
 * in context as a pointer to fetch on demand). The `kind` is just
 * a UI hint for the icon + grouping; it doesn't change runtime
 * behavior.
 */
export type LinkedRepo = { id: string, name: string,
/**
 * Bucket for UI grouping + icon: `"api"` | `"iac"` | `"design"`
 * | `"shared-lib"` | `"docs"` | `"other"`. Stored as String for
 * forward-compat (new kinds don't break rows already in DB).
 */
kind: string,
/**
 * Filesystem path (`/home/user/repos/my-api`) OR URL
 * (`https://github.com/org/my-api`). The agent decides what to
 * do with it based on the format.
 */
location: string,
/**
 * One-line explanation of why this repo is linked. Shown to
 * agents in the prompt prelude so they know when to consult
 * each link (e.g. "GraphQL schema lives here" vs "frontend
 * design tokens").
 */
description?: string, };

/**
 * 0.8.6 (#27) — One row in the linked-repos picker. Surfaces the
 * minimum needed for autocomplete : project id (for stable React
 * keys), human name, path, and a `proximity_hint` (`'same-parent'` |
 * `'other'`) so the UI can render a "Companion projects" group at
 * the top of the dropdown vs an "Other projects" group at the bottom.
 */
export type LinkedRepoCandidate = { id: string, name: string, path: string, proximity_hint: string, };

export type LinkMcpConfigRequest = { project_ids: Array<string>, };

/**
 * The lint result attached to an agent message.
 *
 * Two independent signals:
 * - `unsourced_count` / `flagged_spans` — **niveau 0**, the cheap prose
 *   heuristic (low confidence, lenient, may have false positives).
 * - `sources` / `fabricated_count` — **niveau 1**, mechanical verification of
 *   every `[src: …]` the agent emitted (high confidence, ungameable).
 */
export type LintReport = { unsourced_count: number, flagged_spans: Array<FlaggedSpan>, sources: Array<SourceCheck>, fabricated_count: number,
/**
 * **Niveau 1.5 soft signal.** Natural inline anchors (backticked
 * `path:line`) the agent emitted that did NOT resolve. Distinct from
 * `fabricated_count` (which is reserved for formal `[src:]` markers that
 * failed — high confidence): an inline anchor that doesn't resolve is
 * honestly "couldn't verify" (typo? cross-repo? wrong line?), surfaced as
 * a soft amber pill, NOT a red "fabricated" one.
 */
unverified_count: number, };

/**
 * A configured instance of an MCP server — with label, env secrets, etc.
 * Multiple projects can share the same config (deduplication by config_hash).
 */
export type McpConfig = { id: string, server_id: string, label: string, env_keys: Array<string>, env_encrypted: string, args_override: Array<string> | null, is_global: boolean, include_general: boolean, config_hash: string, project_ids: Array<string>,
/**
 * Migration 036 — opt-in outbound sync to host CLI files.
 * Defaults to `None` for safety; existing rows get `None` from the
 * SQL migration's `DEFAULT 'None'` clause.
 */
host_sync: HostSyncMode, };

/**
 * Display-safe version of McpConfig (secrets masked)
 */
export type McpConfigDisplay = { id: string, server_id: string, server_name: string, label: string, env_keys: Array<string>, env_masked: Array<McpEnvEntry>, args_override: Array<string> | null, is_global: boolean, include_general: boolean, config_hash: string, project_ids: Array<string>, project_names: Array<string>,
/**
 * True when env_keys exist but decryption fails (secrets need re-entry).
 */
secrets_broken: boolean,
/**
 * See `McpConfig::host_sync`.
 */
host_sync: HostSyncMode, };

export type McpContextEntry = { slug: string, label: string, content: string, };

/**
 * Registry entry — an MCP available for installation
 */
export type McpDefinition = { id: string, name: string, description: string, transport: McpTransport, env_keys: Array<string>, tags: Array<string>, token_url?: string | null, token_help?: string | null,
/**
 * Who built this MCP server (e.g. "Anthropic", "Redis Labs", "Fastly").
 */
publisher: string,
/**
 * True when the MCP is built by the vendor of the service it connects to
 * (e.g. Fastly MCP by Fastly = official, GitHub MCP by Anthropic = not official by vendor).
 */
official: boolean,
/**
 * API capability declaration when the plugin exposes a REST API
 * (pure-API like Chartbeat, or hybrid like Jira which has both an MCP
 * and a REST API). Mirrored onto the corresponding `McpServer.api_spec`
 * at seed time.
 */
api_spec?: ApiSpec | null, };

export type McpEnvEntry = { key: string, masked_value: string, };

/**
 * A known incompatibility between an MCP server and a specific agent.
 */
export type McpIncompatibility = {
/**
 * The MCP server ID (e.g. "mcp-gitlab", "detected:data-gouv-fr")
 */
server_id: string,
/**
 * The agent that is incompatible
 */
agent: AgentType,
/**
 * Human-readable explanation
 */
reason: string, };

/**
 * An MCP config whose declared env keys aren't all populated. The
 * scanner skips writing this entry into project-level config files
 * (`.mcp.json`, `.kiro/settings/mcp.json`, `.gemini/settings.json`,
 * `~/.codex/config.toml`) so the agent doesn't choke on a broken
 * MCP at startup. The UI surfaces this so the operator knows which
 * plugin to fix.
 */
export type McpIncompleteConfig = {
/**
 * MCP config DB id.
 */
config_id: string,
/**
 * User-facing label of the config (e.g. "Adobe Analytics", "GitLab Euronews").
 */
label: string,
/**
 * Server name behind this config (helps the user remember which plugin).
 */
server_name: string,
/**
 * The declared env_keys whose decrypted value is missing or empty.
 * Empty when the issue is decryption failure (the cipher itself is
 * gone, e.g. after a key rotation) — the UI shows a generic
 * "secrets unreadable" hint in that case.
 */
missing_keys: Array<string>,
/**
 * Free-form reason — `missing_keys` for a key-by-key gap, or a
 * short error message for decryption failures.
 */
reason: string, };

export type McpOverview = { servers: Array<McpServer>, configs: Array<McpConfigDisplay>,
/**
 * Set of "slug:projectId" pairs where the context file has been customized (not default template).
 */
customized_contexts: Array<string>,
/**
 * Known incompatibilities between MCP servers and agents.
 */
incompatibilities: Array<McpIncompatibility>,
/**
 * Configs that declare `env_keys` but have empty/missing values for
 * at least one of them — those would fail handshake at agent boot
 * (Connection closed, OAuth invalid_client, etc.) AND poison the
 * per-agent startup latency. Kronn skips them at sync time and the
 * UI surfaces them as warnings so the operator can complete the
 * config or remove the entry.
 */
incomplete_configs: Array<McpIncompleteConfig>, };

/**
 * An MCP server type (e.g. "GitHub", "Atlassian", "Context7").
 *
 * A plugin can have MCP capability (via `transport`), API capability (via
 * `api_spec`), or BOTH — e.g. Jira exposes both a `@modelcontextprotocol`
 * server and a REST API, and a hybrid plugin lets the agent pick the
 * right tool. API-only plugins use `McpTransport::ApiOnly` as a sentinel.
 */
export type McpServer = { id: string, name: string, description: string, transport: McpTransport, source: McpSource,
/**
 * When present, the plugin exposes a REST API. Emitted into the agent's
 * `--append-system-prompt` as a secret-free `=== AVAILABLE APIs ===`
 * block; authenticated execution goes through `api_call`. NULL = MCP only.
 */
api_spec?: ApiSpec | null, };

export type McpSource = "Registry" | "Detected" | "Manual" | "HostImported";

export type McpTransport = { "Stdio": { command: string, args: Array<string>, } } | { "Sse": { url: string, } } | { "Streamable": { url: string, } } | "ApiOnly";

/**
 * Lean attachment descriptor surfaced to agents via `disc_get_message`. The
 * `disk_path` lets a file-tool-capable agent open the image directly.
 */
export type MessageAttachment = { id: string, filename: string, mime_type: string, disk_path: string | null, };

export type MessageRole = "User" | "Agent" | "System";

/**
 * Abstract model capability tier. Kronn maps each tier to a concrete --model flag per agent.
 * Priority: AgentSettings.model (explicit) > ModelTier > Default (no flag).
 */
export type ModelTier = "economy" | "default" | "reasoning";

/**
 * Per-agent model tier configuration. Maps Economy/Reasoning to concrete model names.
 * Stored in config.toml under [agents.model_tiers].
 */
export type ModelTierConfig = { economy?: string | null,
/**
 * User override for the `Default` tier — when set, takes precedence
 * over the built-in fallback in `resolve_model_flag`. Lets the user
 * pick e.g. their preferred Ollama model from the OllamaCard picker
 * without having to edit config.toml. `None` = built-in default
 * applies, preserving backward compatibility for users who never
 * touched the setting.
 */
default?: string | null, reasoning?: string | null, };

/**
 * Global model tier overrides per agent.
 */
export type ModelTiersConfig = { claude_code: ModelTierConfig, codex: ModelTierConfig, gemini_cli: ModelTierConfig, kiro: ModelTierConfig, vibe: ModelTierConfig, copilot_cli: ModelTierConfig, ollama: ModelTierConfig, };

/**
 * Config for the "Multi-agent review" option on an Agent step (see
 * `WorkflowStep::multi_agent_review`). A discussion is created, the step's
 * own agent posts its output, the `reviewer_agent` is invited, and the two
 * debate (reusing the multi-agent orchestration core) until they converge
 * or `max_rounds` is hit.
 */
export type MultiAgentReviewConfig = {
/**
 * The second agent invited to challenge the step's output. Pick a
 * DIFFERENT model family than the step's agent to avoid same-model blind
 * spots (the whole point of a second pair of eyes).
 */
reviewer_agent: AgentType,
/**
 * Reasoning tier for the reviewer (None = the agent's default tier).
 */
reviewer_tier?: ModelTier | null,
/**
 * The debate framing posted into the discussion to open the exchange,
 * e.g. "Voici le plan émis par <initial>. <reviewer>, challenge sa
 * pertinence ; vous devez parvenir à un accord global avant de continuer."
 */
debate_prompt: string,
/**
 * Max debate rounds before falling through with the best-so-far result
 * (bounded so a never-converging debate can't hang the run). Default 3.
 */
max_rounds?: number | null, };

/**
 * State of the LAN/Tailscale exposure toggle.
 */
export type NetworkExposure = {
/**
 * Configured to bind a network-reachable address (`0.0.0.0`/`::`), not just localhost.
 */
exposed: boolean,
/**
 * The configured exposure differs from what the process bound at boot →
 * a restart is needed for it to take effect.
 */
restart_required: boolean, port: number,
/**
 * Reachable addresses (LAN + Tailscale) a peer could use to reach us.
 */
reachable_ips: Array<DetectedIp>, };

/**
 * Network info for multi-user connectivity.
 */
export type NetworkInfo = {
/**
 * Detected Tailscale IPv4 address (100.x.x.x), if available.
 */
tailscale_ip: string | null,
/**
 * The host used in invite codes (domain > tailscale > host).
 */
advertised_host: string,
/**
 * Backend port.
 */
port: number,
/**
 * Configured domain, if any.
 */
domain: string | null,
/**
 * All detected network IPs (tailscale, vpn, lan).
 */
detected_ips: Array<DetectedIp>, };

/**
 * Configuration for a `StepType::Notify` webhook step. Rendered at run-time
 * (URL + body support template expressions like `{{previous_step.summary}}`).
 */
export type NotifyConfig = {
/**
 * Target URL. Supports template variables.
 */
url: string,
/**
 * HTTP method — "POST" (default), "PUT", "GET". Only these three are
 * accepted; anything else fails at execution time.
 */
method: string,
/**
 * Custom headers. Case-insensitive on the wire — we send them as given.
 */
headers?: { [key in string]: string },
/**
 * Request body. Templated. Sent as-is — set `Content-Type: application/json`
 * in `headers` if the body is JSON. Ignored for GET.
 */
body_template: string, };

/**
 * Static header injected server-side alongside the `Authorization: Bearer`
 * from an OAuth2 exchange. Used for providers (Adobe Analytics, some Salesforce
 * endpoints) that require extra identification headers beyond the
 * bearer token. `value_template` supports `{ENV_KEY}` substitution from
 * the config's env map.
 */
export type OAuth2ExtraHeader = { name: string, value_template: string, };

export type OllamaHealthResponse = {
/**
 * "online", "offline", "not_installed", "unreachable"
 */
status: string, version: string | null, endpoint: string, models_count: number,
/**
 * User-facing explanation when status != "online". Contextualized
 * for the detected environment (native, Docker, WSL).
 */
hint: string | null, };

export type OllamaModel = { name: string, size: string, modified: string, };

export type OllamaModelsResponse = { models: Array<OllamaModel>, };

/**
 * What happens when `TypedSchema` validation still fails after a
 * single repair attempt. `Continue` = 0.7.0 behavior (warn + raw),
 * `Fail` = 0.8.3 strict mode for contract steps.
 */
export type OnInvalid = "Continue" | "Fail";

export type OrchestrationRequest = { agents: Array<AgentType>, max_rounds?: number | null, skill_ids?: Array<string>, profile_ids?: Array<string>, directive_ids?: Array<string>, };

/**
 * stab-3 — pacing state COMPUTED BY THE SERVER (the agents apply it, they
 * don't interpret): hot while the last User message is within the lease,
 * cold otherwise. In BOTH regimes `next_delay_seconds` is the instruction
 * to apply verbatim — in cold it is the backoff-ramp step derived
 * statelessly from the elapsed silence (see `pacing_for`).
 * Closed set — a typo'd regime must fail to COMPILE, and the generated TS
 * side gets the `"hot" | "cold"` union instead of `string` (Copilot round 5).
 * Wire format stays the lowercase string the bridge already reads.
 */
export type PacingRegime = "hot" | "cold";

export type PacingState = { regime: PacingRegime, next_delay_seconds: number, attention_until?: string, };

/**
 * Pagination strategy for an `ApiCall` step. `Auto` covers the three most
 * common REST patterns; explicit variants let advanced users hardcode the
 * cursor/offset paths for non-standard APIs (Cloudflare GraphQL for ex.).
 */
export type PaginationSpec = { "type": "None" } | { "type": "Auto",
/**
 * Safety cap. Defaults to 50 when unset.
 */
max_pages?: number | null, } | { "type": "Offset", start_param: string, limit_param: string, limit: number,
/**
 * JSONPath to the total count in the response, e.g. `$.total`.
 */
total_path: string, max_pages?: number | null, } | { "type": "Cursor", cursor_param: string,
/**
 * JSONPath to the next cursor value, e.g. `$.pageInfo.endCursor`.
 */
next_path: string, max_pages?: number | null, } | { "type": "LinkHeader", page_size_param?: string | null, page_size?: number | null, max_pages?: number | null, } | { "type": "Page", page_param: string, page_size_param: string, page_size: number,
/**
 * JSONPath to a boolean / truthy "has more" indicator.
 */
has_more_path: string, max_pages?: number | null, };

export type PartialAuditRequest = { agent: AgentType, steps: Array<number>, };

/**
 * Body of `POST /api/discussions/peer-join`. The token is the
 * plaintext returned by `invite_peer`. `agent_type` + `session_id`
 * identify the calling CLI session so the bridge can rebind a
 * disconnected agent on reconnect.
 */
export type PeerJoinRequest = { token: string,
/**
 * `ClaudeCode | Codex | GeminiCli | Kiro | CopilotCli | Vibe | Ollama | Custom`
 * — same enum as the Rust `AgentType`.
 */
agent_type: string,
/**
 * CLI-assigned session id. UUID-like for Claude Code, numeric or
 * string for others. Treated as an opaque identifier.
 */
session_id: string, };

/**
 * Wire shape returned by `peer-join`. Carries the disc id (so the
 * bridge can stash it as its `_CURRENT_DISC_ID`), a peer count for
 * the agent's first system-prompt notice, and a recent-message
 * preview so the joiner has immediate context.
 */
export type PeerJoinResponse = { disc_id: string, session_pk: number,
/**
 * Opaque reload credential. Persist locally with mode 0600; never log or
 * expose it to the model. The backend stores only its SHA-256 digest.
 */
resume_token: string, peer_count: number,
/**
 * Title of the disc, surfaced in the agent's first reply so the
 * human can verify it joined the right conversation.
 */
disc_title: string,
/**
 * Last N messages already in the disc (default 10). Empty for a
 * freshly-created topic.
 */
recent_messages: Array<RecentMessagePreview>,
/**
 * 0.8.6 fix 2026-05-21 — explicit directive returned to the
 * agent so it understands the multi-agent protocol. Without
 * this, agents like Codex/Vibe would `disc_join` and then just
 * print their intro to their own terminal (invisible to peers).
 * The text tells them : *use disc_append to speak*, don't just
 * reply to the user in your terminal.
 */
next_steps: string,
/**
 * Long-poll pacing contract (stab-1) — walk `poll_backoff_seconds`
 * while the room is silent, reset on any peer message.
 */
poll_policy: PollBackoffPolicy,
/**
 * stab-3 — server-computed pacing, same contract as wait/meta: apply
 * `next_delay_seconds` verbatim before the FIRST wait. Included at
 * join so a fresh peer doesn't need a meta/wait round-trip to pace
 * itself (Copilot review: join was the one response missing it).
 */
pacing: PacingState, };

/**
 * Body of `POST /api/discussions/peer-leave`. Identifies the caller
 * the same way `peer_join` does — by `(agent_type, session_id)` —
 * so the bridge can find its own active session row and mark it left.
 */
export type PeerLeaveRequest = { agent_type: string, session_id: string, };

export type PeerLeaveResponse = {
/**
 * `true` when an active session was found + marked left.
 * `false` when the caller had no active session (already left,
 * or never joined). Either way, idempotent.
 */
left: boolean, };

export type PeerResumeRequest = { agent_type: string, session_id: string, resume_token: string, };

export type PeerResumeResponse = { disc_id: string, session_pk: number,
/**
 * Rotated credential replacing the one supplied in the request.
 */
resume_token: string, };

/**
 * stab-1 (Romu) — EXPLICIT long-poll pacing contract, returned by
 * `disc_meta` and `peer-join` instead of living as an implicit convention
 * in each agent's prompt. Agents walk `poll_backoff_seconds` while the
 * room is silent (staying on the last value once exhausted) and reset to
 * the first entry as soon as a peer message arrives.
 */
export type PollBackoffPolicy = { poll_backoff_seconds: Array<number>, reset_on_peer_message: boolean, max_delay_seconds: number,
/**
 * stab-3 — poll interval while a HUMAN attention lease is active
 * (a User message opens/renews the lease). Debated Claude/Codex,
 * Romu's requirement: sub-minute answers while he is present.
 */
hot_poll_seconds: number,
/**
 * How long a User message keeps the room in the hot regime.
 */
user_attention_lease_seconds: number, };

/**
 * One preserved branch on a workflow run. Mirrors `workspace::PreservedBranch`
 * but lives on the model side so it can serialize to JSON for storage and
 * type-export for the frontend.
 */
export type ProducedBranch = { branch_name: string, head_sha: string, ahead: number,
/**
 * True when the branch had an upstream tracking ref at cleanup time —
 * i.e. the agent at least *tried* to push (and may have partially
 * succeeded; check `ahead` for unpushed commits).
 */
pushed_upstream: boolean, };

export type ProfileCategory = "Technical" | "Business" | "Meta";

export type Project = { id: string, name: string, path: string, repo_url: string | null, token_override: TokenOverride | null, ai_config: AiConfigStatus, audit_status: AiAuditStatus, ai_todo_count: number,
/**
 * Total tech-debt entries detected in the project's docs tree:
 * one count per file under `docs/tech-debt/` plus one count per
 * table row in `docs/inconsistencies-tech-debt.md`. Computed by
 * `scanner::count_tech_debt` and surfaced as a badge on the
 * project card so users see at a glance how many TD items remain
 * to address. Not persisted in DB.
 */
tech_debt_count: number,
/**
 * True when the project still uses the legacy `ai/index.md` layout
 * and no migrated `docs/AGENTS.md` exists. Computed by
 * `enrich_audit_status` — drives the migration banner on
 * `ProjectCard`. Not persisted in DB.
 */
needs_docs_migration: boolean,
/**
 * True when the project directory resolves on disk. Computed by
 * `enrich_audit_status` (the list/get API layer), NOT persisted. The DB
 * row mapper defaults it to `true` so a non-enriched read (e.g. the export
 * payload) never falsely flags a project. Drives the "chemin introuvable —
 * remap" banner + per-card badge after a cross-OS import (WSL ⇄ macOS),
 * where absolute paths don't translate.
 */
path_exists: boolean, default_skill_ids?: Array<string>, default_profile_id?: string | null, briefing_notes?: string | null,
/**
 * 0.8.3 — Companion repos that an agent on this project should
 * know about. A typical setup: a frontend project pointing at
 * the backend API repo, the IaC repo, and the shared design
 * system repo. The audit pipeline + every discussion / QP /
 * workflow running on this project picks up this list in its
 * system prompt prelude. Stored as in-row JSON (small data,
 * projects rarely have more than 5 links).
 */
linked_repos?: Array<LinkedRepo>, created_at: string, updated_at: string, };

export type ProjectUsage = { project_id: string, project_name: string, tokens_used: number, cost_usd: number, };

export type PromptVariable = { name: string, label: string, placeholder: string,
/**
 * Optional human description of what this variable means. Shown in
 * the batch-workflow UI so the user mapping tracker fields to QP
 * variables knows what each one is for.
 */
description: string | null,
/**
 * Whether the variable must be filled before the QP can run.
 * Defaults to `true` for backward compatibility — existing QP
 * variables are treated as required.
 */
required: boolean,
/**
 * 2026-06-10 — optional regex the provided value must match (anchored
 * full-match). Lets a workflow declare a shape (`^[A-Z]+-\d+$` for a
 * Jira key) so a typo like `7152` instead of `EW-7152` is rejected at
 * launch with a clear message, BEFORE it reaches the API as a literal
 * path param and 404s. `None` = no shape constraint (legacy). Invalid
 * regex is treated as "no constraint" (never blocks a launch on a
 * malformed pattern; logged).
 */
pattern?: string | null, };

export type ProposeResult = { accepted: boolean, reason: string | null, warnings: Array<string>, evidence_checks: Array<EvidenceCheck>, learning: Learning | null, };

export type ProviderUsage = { provider: string, tokens_used: number, tokens_limit: number | null, cost_usd: number | null, };

export type QuickApi = { id: string, name: string, icon: string,
/**
 * Optional human description — shown in the BatchApiCall picker.
 */
description: string, project_id: string | null, api_plugin_slug: string, api_config_id: string, api_endpoint_path: string, api_method?: string | null, api_query?: { [key in string]: string } | null, api_path_params?: { [key in string]: string } | null, api_headers?: { [key in string]: string } | null,
/**
 * Same shape as `WorkflowStep.api_body`: a JSON `Value` rather than a
 * raw string. The runtime engine walks the tree and interpolates
 * string leaves only — no string-level templating that would let a
 * `{{var}}` containing `","` punch through into JSON injection.
 */
api_body?: JsonValue | null, api_extract?: ExtractSpec | null, api_pagination?: PaginationSpec | null, api_timeout_ms?: number | null, api_max_retries?: number | null,
/**
 * Variables prompted at run-time (single-call) or whose names become
 * the keys mapped from each batch item (batch-call).
 */
variables: Array<PromptVariable>,
/**
 * 0.8.5 — optional profile binding. Picked up by any downstream
 * agent surface that consumes this Quick API (e.g. the "Compare
 * agents" QA helper). Empty vec = unbound. Pure API calls ignore
 * this; it only matters when the QA result feeds into an LLM step.
 */
profile_ids: Array<string>,
/**
 * 0.8.5 — optional directive binding. Same rationale as
 * `profile_ids` above.
 */
directive_ids: Array<string>, created_at: string, updated_at: string, };

/**
 * Self-contained envelope produced by `GET /api/quick-apis/:id/export`.
 */
export type QuickApiExportEnvelope = { kind: string, version: number, exported_at: string,
/**
 * `id`, `project_id`, `created_at`, `updated_at` are present on the
 * wire but reset at import — fresh values are minted by the importer.
 */
quick_api: QuickApi, };

export type QuickPrompt = { id: string, name: string, icon: string, prompt_template: string, variables: Array<PromptVariable>, agent: AgentType, project_id: string | null, skill_ids: Array<string>,
/**
 * 0.8.5 — optional profile binding (persona injection at launch).
 * Mirrors `WorkflowStep.profile_ids` + `Discussion.profile_ids`. Empty
 * vec = no profile bound (legacy behaviour).
 */
profile_ids: Array<string>,
/**
 * 0.8.5 — optional directive binding (rules-of-conduct at launch).
 * Mirrors `WorkflowStep.directive_ids` + `Discussion.directive_ids`.
 * Empty vec = no directive bound (legacy behaviour).
 */
directive_ids: Array<string>, tier: ModelTier,
/**
 * 0.8.10 — optional explicit model (+ effort / max_tokens), mirroring
 * `WorkflowStep.agent_settings`. `agent_settings.model` wins over `tier`
 * (see `runner::effective_model_flag`). Copied onto a workflow step by
 * `quick_prompt_hydrate`, and onto the launched discussion by the QP
 * launch path. `None` = resolve the model from `tier` as before.
 */
agent_settings?: AgentSettings | null,
/**
 * Optional human description of what this Quick Prompt does. Shown
 * in the batch-workflow picker so the user knows which QP fits their
 * use case. Empty string = legacy QP created before 2026-04-10.
 */
description: string, created_at: string, updated_at: string, };

/**
 * Self-contained envelope produced by `GET /api/quick-prompts/:id/export`.
 */
export type QuickPromptExportEnvelope = { kind: string, version: number, exported_at: string,
/**
 * Like the workflow envelope: `id`, `project_id`, `created_at`,
 * `updated_at` are present on the wire but reset at import.
 */
quick_prompt: QuickPrompt, };

export type QuickPromptVersion = { id: string, quick_prompt_id: string, version_index: number, name: string, icon: string, prompt_template: string, variables: Array<PromptVariable>, agent: AgentType, project_id: string | null, skill_ids: Array<string>, profile_ids: Array<string>, directive_ids: Array<string>, tier: ModelTier, description: string, created_at: string, };

export type QuickPromptVersionMetrics = { version_index: number,
/**
 * Number of launched discussions whose first-agent-reply lands
 * in this version's window. Pertinence Δs are only emitted when
 * `launches >= 3` (the noise floor).
 */
launches: number, avg_tokens: number,
/**
 * Mean wall-clock duration of the first agent reply, milliseconds.
 * `None` when no launch in this version has a captured
 * `duration_ms` (legacy rows or imported transcripts).
 */
avg_duration_ms: number | null,
/**
 * Mean USD cost of the first agent reply. `None` when no launch
 * has cost data (e.g. local Ollama runs).
 */
avg_cost_usd: number | null, };

export type RecentMessagePreview = { sort_order: number, role: string, agent_type: string | null, timestamp: string,
/**
 * Body trimmed to 400 chars so the response stays small. The
 * agent can `disc_get_message(idx)` to fetch full text.
 */
preview: string, };

export type RecoveryStatus = { configured: boolean, };

export type RemoteRepo = { name: string, full_name: string, clone_url: string, ssh_url: string, description: string | null, language: string | null, stargazers_count: number, updated_at: string, source: string, already_cloned: boolean, };

export type RepoSource = { id: string, label: string, provider: string, };

/**
 * Response for [`resume_interrupted`].
 */
export type ResumeRunResponse = { run_id: string, new_status: RunStatus, };

export type RetryConfig = { max_retries: number, backoff: string, };

export type RtkActivateRequest = {
/**
 * Agents the frontend wants RTK hooks on. The backend filters this
 * to only agents RTK supports before spawning.
 */
agents: Array<AgentType>, };

export type RtkActivateResponse = {
/**
 * Overall success — true if every RTK-supported agent invocation
 * exited 0. A single failure flips this to false so the frontend can
 * surface an error toast even when some agents succeeded.
 */
success: boolean,
/**
 * Concatenated stdout of every per-agent invocation, prefixed with
 * the agent name. Useful when the user wants to see what RTK did.
 */
stdout: string,
/**
 * Concatenated stderr. Empty when `success` is true.
 */
stderr: string,
/**
 * Per-agent outcomes — surfaces which agent failed when success is
 * partial. Empty when nothing ran (no compatible agent installed).
 */
per_agent: Array<RtkAgentActivation>, };

export type RtkAgentActivation = { agent_type: AgentType, success: boolean, stdout: string, stderr: string, };

export type RtkSavings = {
/**
 * `true` when we got a readable response from RTK. Frontend uses this
 * flag to decide whether to render the counter at all — when RTK is
 * absent or the CLI output shape changes, we degrade silently rather
 * than showing a zero that would look like "RTK saved nothing".
 */
available: boolean,
/**
 * Best-effort sum of tokens RTK reports as saved. 0 when `available`
 * is false.
 */
total_tokens_saved: number,
/**
 * Rough compression ratio in [0, 100]. 0 when `available` is false.
 */
ratio_percent: number,
/**
 * Number of compression samples RTK has on record.
 */
sample_count: number, };

export type RtkVersionInfo = {
/**
 * `true` when `rtk --version` ran and we got a numeric prefix back.
 * Hides the freshness pill cleanly when RTK is absent.
 */
available: boolean,
/**
 * Numeric prefix of `rtk --version`'s stdout, e.g. "0.37.2". `None`
 * when the call failed or the output couldn't be parsed.
 */
installed: string | null,
/**
 * Latest known stable version from our bumped-per-release registry.
 */
latest_known: string,
/**
 * True iff `installed < latest_known` under lenient semver. The
 * frontend renders the "update available" pill from this flag only —
 * keeps the freshness logic centralised in one Rust comparator.
 */
update_available: boolean,
/**
 * Copy-pasteable upgrade command (idempotent — RTK install.sh
 * upgrades in place).
 */
update_command: string, };

/**
 * 0.6.0 — payload for `POST /api/quick-apis/:id/run`. Lets the user
 * launch a saved QuickApi standalone (Run drawer in the Quick APIs page),
 * passing values for the declared `variables`.
 */
export type RunQuickApiRequest = {
/**
 * Map of variable name → user-entered value. Keys must match
 * `QuickApi.variables[*].name`. Missing keys for required variables
 * get the call rejected before any HTTP fires.
 */
variables?: Record<string, string>, };

/**
 * Response from `POST /api/quick-apis/:id/run`. Mirrors the
 * `/test-api-call` shape so the frontend can reuse the same UI.
 */
export type RunQuickApiResponse = { success: boolean, duration_ms: number,
/**
 * Parsed envelope (data/status/summary) on success, `None` on failure.
 */
envelope: JsonValue | null,
/**
 * Error message on failure, `None` on success.
 */
error: string | null, };

export type RunStatus = "Pending" | "Running" | "Success" | "Failed" | "Cancelled" | "WaitingApproval" | "StoppedByGuard" | "Interrupted";

export type SaveApiKeyRequest = { id?: string | null, name: string, provider: string, value: string, };

export type ScanConfig = { paths: Array<string>, ignore: Array<string>,
/**
 * Max depth when scanning for git repos (2–10, default 4)
 */
scan_depth: number, };

export type SendMessageRequest = { content: string, target_agent?: AgentType | null, };

export type ServerConfig = { host: string, port: number,
/**
 * Custom domain for CORS and TLS (e.g. "kronn.local")
 */
domain: string | null,
/**
 * 0.8.11 (B6) — optional webhook (Slack/Teams/generic JSON) fired when a
 * scheduled/triggered run ends in a non-success terminal state
 * (Failed / Interrupted / StoppedByGuard). Lets an autonomous cron that
 * dies at 6am surface immediately instead of being discovered by opening
 * the UI. Empty/None = no notification. Also settable via
 * `KRONN_FAILURE_NOTIFY_URL`.
 */
failure_notify_url: string | null,
/**
 * 0.8.11 (B7) — auto-purge workflow runs older than N days at boot.
 * `0` (default) = DISABLED: never delete run history automatically (a fast
 * cron's run table is 76% of the DB, but silently dropping the user's
 * history is worse than size). Set to e.g. 90 to bound growth; parent runs
 * still referenced by a retained child are always preserved.
 */
run_retention_days: number,
/**
 * Maximum concurrent agent processes (default: 5)
 */
max_concurrent_agents: number,
/**
 * Agent stall timeout in minutes — abort if no output for this long (default: 5)
 */
agent_stall_timeout_min: number,
/**
 * User identity — displayed in messages and used for future multi-user
 */
pseudo: string | null,
/**
 * Email for Gravatar avatar (optional, decoupled from git)
 */
avatar_email: string | null,
/**
 * Short bio — who the user is, their role, expertise. Injected at the start of first message in a discussion.
 */
bio: string | null,
/**
 * Global context injected into discussions. Markdown content — glossary,
 * company conventions, stack overview, etc. Supplements project-level
 * `ai/` context. Stored in config.toml.
 */
global_context: string | null,
/**
 * When to inject global_context:
 * - `"always"` (default) — every discussion
 * - `"no_project"` — only discussions without a project
 * - `"never"` — disabled
 */
global_context_mode: string,
/**
 * 0.8.7 anti-hallucination mode: `off` | `warn` | `enforce`.
 *
 * - `off` — feature disabled, nothing injected or linted.
 * - `warn` (default) — P1 sourcing directive injected + P2 lint (heuristic + mechanical `[src:]` verification) surfaced as a non-blocking pill.
 * - `enforce` — same as `warn` in 0.8.7; reserved for the Phase 3 write-time refusal of unverifiable citations.
 *
 * See `core::anti_halluc`. Mirrored into the process-global flag at load + save.
 */
anti_hallucination_mode: string,
/**
 * 0.9.0 — Continual Learning master toggle. **Default OFF (beta)**: the
 * feature writes agent-proposed learnings into injected truth files
 * (`docs/learnings.md` / user-context), so it ships opt-in to avoid a bug
 * polluting a user's docs. Gates capture (`learning_propose`), the
 * `kronn:section name="learnings"` doc pointer, and the UI badge/modal.
 * Validating/rejecting EXISTING pending candidates stays allowed when off
 * (drain, don't capture). See docs/research/continual-learning-0.9.0-spec.md §0.
 */
continual_learning_enabled: boolean,
/**
 * Debug mode — when true, the tracing subscriber is initialized at
 * `debug` level instead of `info`, producing significantly more
 * output on stdout. Lets users diagnose agent detection / project
 * scan issues themselves without needing to set `RUST_LOG` by hand.
 * Persisted in config.toml so it survives restarts. Toggleable from
 * the Settings UI or via `./kronn start --debug` (CLI flag wins for
 * the duration of that run).
 */
debug_mode: boolean,
/**
 * 0.8.6 phase 4 — default model tier applied to NEW creations
 * (discussions, QP drafts, workflow Agent steps) when the user
 * doesn't explicitly pick one in the form. STRICT semantic :
 * only consulted by creation flows on `componentDidMount` ; never
 * applied retroactively to existing items at execution time
 * (otherwise a user flipping the default to `Reasoning` would
 * silently 10x the cost of every legacy QP they launch).
 *
 * Persisted in `config.toml`. Defaults to `Default` for
 * backwards-compat — existing configs without the field keep
 * the prior hardcoded behaviour.
 */
default_model_tier: ModelTier,
/**
 * 0.8.6 phase 4 — default summary strategy applied to NEW
 * discussions. Flipped from `Auto` to `Off` because most modern
 * agents (Claude Code, Codex, Gemini-Pro) have large context
 * windows AND can pull older history on-demand via the
 * `disc_load_other` MCP tool — auto-summary just burns Economy
 * tokens for no win in those cases. The `Off` default makes
 * Kronn cheaper out of the box.
 *
 * Re-enable `Auto` (Settings) when running small-context agents
 * (Ollama 8B / Vibe / older models) that lack MCP access and
 * can't ask Kronn for older history themselves.
 *
 * Strict semantic — only consulted on NEW disc creation. Existing
 * discs keep their saved value (no retroactive change).
 */
default_summary_strategy: SummaryStrategy, };

export type ServerConfigPublic = { host: string, port: number, domain: string | null, max_concurrent_agents: number, agent_stall_timeout_min: number, auth_enabled: boolean, pseudo: string | null, avatar_email: string | null, bio: string | null, debug_mode: boolean,
/**
 * 0.8.6 phase 4 — default model tier for new disc/QP/WF agent steps.
 * Mirrored from `ServerConfig.default_model_tier` so the frontend
 * can pre-fill the tier picker on creation forms without an extra
 * round-trip. Strict semantic — never retroactive (see backing field
 * rustdoc).
 */
default_model_tier: ModelTier,
/**
 * 0.8.6 phase 4 — default summary strategy for new discussions.
 * `Off` by default in 0.8.6 onwards. UI surfaces an explanation of
 * when to re-enable (small-context agents without MCP access).
 */
default_summary_strategy: SummaryStrategy, };

export type SetAgentAccessRequest = { agent: AgentType, full_access: boolean, };

export type SetBriefingRequest = { notes?: string | null, };

export type SetRecoveryResponse = {
/**
 * The off-machine copy the user must save. With it + the passphrase, the
 * encryption key survives total loss of the machine / keychain / data dir.
 */
recovery_code: string, };

export type SetScanPathsRequest = { paths: Array<string>, };

export type SetupStatus = { is_first_run: boolean, current_step: SetupStep, agents_detected: Array<AgentDetection>, scan_paths_set: boolean, repos_detected: Array<DetectedRepo>, default_scan_path: string | null, };

export type SetupStep = "Agents" | "ScanPaths" | "Detection" | "Complete";

export type ShareDiscussionRequest = { contact_ids: Array<string>, };

export type Skill = { id: string, name: string, description: string, icon: string, category: SkillCategory, content: string, is_builtin: boolean,
/**
 * Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
 */
token_estimate: number,
/**
 * agentskills.io: SPDX license identifier or reference to bundled LICENSE file.
 */
license?: string | null,
/**
 * agentskills.io: space-delimited list of pre-approved tools (e.g. "Bash Read Grep").
 */
allowed_tools?: string | null,
/**
 * Optional auto-activation trigger regexes keyed by locale. When
 * the user types a message matching one of these patterns, the
 * frontend auto-adds this skill to the current discussion. The
 * `common` entry always applies; the locale-specific entries
 * apply when the discussion's language matches. See the YAML
 * frontmatter convention in `backend/src/skills/kronn-docs.md`.
 */
auto_triggers?: AutoTriggers | null,
/**
 * 0.7+ — true when the skill content was vendored from a third-party
 * open-source project (see `THIRD_PARTY_SKILLS.md` at repo root).
 * The frontend renders a "🔗 External" badge to make attribution
 * visible in-app. Set via the `external: true` frontmatter field on
 * builtin skills under `backend/src/skills/external/`.
 */
external?: boolean,
/**
 * 0.7+ — when `external` is true, points to the upstream project
 * (clickable in the UI for attribution). Set via the `source_url`
 * frontmatter field.
 */
source_url?: string | null, };

export type SkillCategory = "Language" | "Domain" | "Business";

/**
 * One extracted `[src: …]` marker plus its mechanical verdict.
 */
export type SourceCheck = {
/**
 * Inner content of the marker (after `src:`), trimmed.
 */
raw: string, kind: SourceKind, status: SourceStatus,
/**
 * Human-readable reason (shown in the pill drawer).
 */
detail: string, };

/**
 * What kind of source a `[src: …]` marker points at.
 */
export type SourceKind = "file" | "url" | "user" | "commit" | "api" | "code_comment" | "inferred" | "hypothesis" | "training_data" | "other";

/**
 * Result of mechanically verifying one `[src: …]` marker.
 */
export type SourceStatus = "verified" | "not_found" | "out_of_bounds" | "empty_ref" | "outside_project" | "unchecked" | "rejected";

export type StartBriefingResponse = { discussion_id: string, };

export type StepConditionRule = { contains: string, action: ConditionAction, };

export type StepMode = { "type": "Normal" };

/**
 * How a step's output is formatted and extracted.
 * `FreeText` (default): raw text, passed as-is via `{{previous_step.output}}`.
 * `Structured`: engine injects format instructions and extracts a JSON envelope
 *   (`{"data": ..., "status": "OK|NO_RESULTS|ERROR", "summary": "..."}`).
 *   Downstream steps can use `{{previous_step.data}}` and `{{previous_step.summary}}`.
 * `TypedSchema { schema }` (0.7.0 Phase 2): like Structured, but the
 *   `data` field is validated against a JSON-Schema subset provided
 *   by the workflow author. The schema is serialised into the prompt
 *   so the LLM produces conforming output, and the engine rejects
 *   non-conforming responses with a repair prompt. Used by Auto-Dev's
 *   `validate_ticket` step (output_schema with status enum + score range).
 */
export type StepOutputFormat = { "type": "FreeText" } | { "type": "Structured" } | { "type": "TypedSchema",
/**
 * JSON Schema (subset). Stored verbatim; the runner serialises
 * it into the prompt as-is so the LLM sees the exact shape it
 * must produce.
 */
schema: any, on_invalid: OnInvalid, };

export type StepResult = { step_name: string, status: RunStatus, output: string, tokens_used: number, duration_ms: number,
/**
 * 0.8.2 — Wall-clock timestamp at which the step started executing.
 * Optional for backward compatibility with runs written before this
 * field existed (front-end falls back to the legacy `runStart + sum
 * of prior durations` estimate when missing). The primary driver for
 * adding this was the Gate step's `duration_ms`: it used to record
 * only the executor render time (~0ms), so the time spent paused on
 * WaitingApproval was invisible to the live-elapsed counter for the
 * NEXT step (which then showed `now - runStart - 0ms` ≈ the full
 * pause duration). With `started_at`, the resume handler can compute
 * `duration_ms = now - started_at` on approval and surface the real
 * pause.
 */
started_at?: string | null,
/**
 * What happened after this step: null = continued normally, or the condition action triggered.
 */
condition_result?: string | null,
/**
 * For `output_format: Structured` steps only — did the agent actually
 * produce the `---STEP_OUTPUT---` envelope (possibly after repair)?
 * `Some(true)`  = envelope found, `.data/.summary/.status` populated.
 * `Some(false)` = Structured requested but extraction failed even after
 *                 repair, downstream `{{steps.X.data}}` won't resolve.
 * `None`        = FreeText step, the concept does not apply.
 */
envelope_detected?: boolean | null,
/**
 * Snapshot of the step's `step_type.type` at execution time
 * (`"Agent" | "ApiCall" | "Notify" | "BatchQuickPrompt" | "Custom"`).
 * Frozen on the run row so editing the workflow afterwards (changing
 * the step type, swapping the agent, retargeting the API plugin)
 * doesn't corrupt the historical record. `None` is tolerated for
 * rows written before this field existed — the frontend falls back
 * to "(legacy)" rather than crashing.
 */
step_kind?: string | null,
/**
 * Snapshot of `step.agent` for Agent / Custom steps. `None` for
 * non-agent steps (ApiCall, Notify, Batch). Lets the run-detail UI
 * say "Codex was used here" even after the workflow was edited to
 * run with a different agent.
 */
step_agent?: AgentType | null,
/**
 * 2026-06-13 — the model/tier actually RESOLVED for this Agent step at
 * run time (e.g. "opus", "sonnet · reasoning", "haiku · economy"). Stamped
 * from the step's tier + the user's model_tiers config, so the run-detail
 * UI shows the real model on EVERY agent step — including the per-item
 * fan-out routing (low→economy/high→reasoning), not just steps with an
 * explicit tier in their definition. `None` for non-agent steps.
 */
step_model?: string | null,
/**
 * Snapshot of the API plugin slug for ApiCall steps (`mcp-github`,
 * `api-chartbeat`, …). `None` otherwise.
 */
step_api_plugin_slug?: string | null,
/**
 * Snapshot of the resolved endpoint path for ApiCall steps. Stored
 * AFTER path-param substitution so reviewers see the actual URL
 * path that was hit (`/repos/anthropics/anthropic-cookbook/issues`
 * rather than the template `/repos/{owner}/{repo}/issues`). `None`
 * for non-API steps.
 */
step_api_endpoint_path?: string | null,
/**
 * 2026-06-10 (audit P1) — true when this result was produced by the
 * `on_failure` compensation chain, NOT the nominal step sequence.
 * Pre-fix, rollback results were appended to `step_results` with no
 * marker: a green `alert-ops` right after a red `fetch-ticket` read
 * as "the run continued past the failure". The UI renders these
 * under a dedicated ROLLBACK section. `default` keeps legacy rows
 * (and every nominal constructor) at `false`.
 */
is_rollback: boolean,
/**
 * 2026-06-11 (Phase 1) — for a `SubWorkflow` step: the id of the child
 * run it spawned, so the UI can drill from this step into the nested
 * run tree (`GET /runs/:id/tree`). `None` for every other step type and
 * for legacy rows. The inverse of `WorkflowRun.parent_run_id`.
 */
child_run_id?: string | null, };

export type StepType = { "type": "Agent" } | { "type": "ApiCall" } | { "type": "BatchQuickPrompt" } | { "type": "Notify" } | { "type": "Gate" } | { "type": "Exec" } | { "type": "BatchApiCall" } | { "type": "JsonData" } | { "type": "SubWorkflow" };

export type SummarizeRequest = {
/**
 * 0-based start index (inclusive). `None` = start of transcript.
 */
from?: number | null,
/**
 * 0-based end index (exclusive). `None` = up to the latest message.
 */
to?: number | null,
/**
 * Force regeneration even if the cached summary covers the same
 * range. Useful when the agent thinks the cached summary is stale.
 */
force_refresh?: boolean, };

export type SummarizeResponse = { summary: string, from_idx: number, to_idx: number, generated: boolean,
/**
 * Tokens spent generating the summary. `0` when served from cache.
 */
tokens_used: number, };

/**
 * Per-discussion summary strategy. Pre-fix the auto-summary loop fired
 * after every agent reply once a per-agent threshold was crossed (12/8/4
 * non-system messages). For big-context models or short threads that's
 * often a waste — user feedback on 2026-05-09 asked for an off switch.
 *
 * `OnDemand` is reserved for the future kronn-internal MCP tool surface
 * (`disc_summarize` callable by the agent itself); for now it behaves
 * like `Off` from the auto-fire perspective and only differs in that we
 * keep the cache mechanism alive so an explicit summarize call updates
 * `summary_cache`.
 */
export type SummaryStrategy = "Auto" | "OnDemand" | "Off";

export type TechDebtItem = { id: string, problem: string, area: string, severity: string, };

export type TestApiCallRequest = {
/**
 * The (partial) step configuration the user is building in the wizard.
 * Must at least declare `api_plugin_slug`, `api_config_id`, and
 * `api_endpoint_path`.
 */
step: WorkflowStep,
/**
 * Project context — plugin instances are scoped per project. Required.
 */
project_id: string, };

export type TestApiCallResponse = {
/**
 * Matches the `StepOutcome.result.status` after normalization —
 * `true` when the HTTP call succeeded and extract (if any) ran
 * without error. NO_RESULTS still counts as success here; the
 * wizard surfaces it via `envelope.status`.
 */
success: boolean,
/**
 * Milliseconds elapsed end-to-end.
 */
duration_ms: number,
/**
 * `{data, status, summary}` envelope (parsed from the step output).
 * On failure this is `null` and `error` holds the message.
 */
envelope: JsonValue | null,
/**
 * Error message when `success == false`. Same string that would
 * land in the step's output column if this were a real run.
 */
error: string | null, };

export type TestExtractRequest = {
/**
 * Sample JSON the user is refining the path against — either pasted
 * from docs or the response of a previous `test-api-call`.
 */
sample: JsonValue,
/**
 * JSONPath expression, e.g. `$.issues[*].key`.
 */
path: string,
/**
 * Optional fallback when the path matches nothing.
 */
fallback?: JsonValue | null,
/**
 * When true, empty extractions count as NO_RESULTS in the response.
 */
fail_on_empty?: boolean, };

export type TestExtractResponse = {
/**
 * Resolved value. `null` when the path matched nothing (unless
 * `fallback` was set, in which case fallback is returned).
 */
value: JsonValue,
/**
 * Human-readable type tag for the wizard preview: `"number"`,
 * `"string"`, `"boolean"`, `"array(N)"`, `"object"`, `"null"`.
 */
value_type: string,
/**
 * True when no match was found (even if a fallback rescued the
 * value). Drives the "0 results — will skip next step" hint.
 */
is_empty: boolean,
/**
 * Only set when the JSONPath is syntactically invalid — the wizard
 * shows this inline under the input.
 */
error: string | null, };

export type TestStepRequest = { step: WorkflowStep, project_id?: string | null,
/**
 * Mock previous step output (raw text or structured JSON)
 */
mock_previous_output?: string | null,
/**
 * Additional mock variables: {"issue.title": "...", "steps.collect.data": "..."}
 */
mock_variables?: { [key in string]: string } | null,
/**
 * Dry run: agent describes what it would do without executing any write actions
 */
dry_run?: boolean, };

/**
 * 0.8.6 — Body serialization format for `TokenExchange`.
 */
export type TokenExchangeBodyFormat = "Json" | "FormUrlEncoded";

/**
 * 0.8.6 — How to inject a freshly-exchanged token into the actual API
 * call. The 90% case is Bearer header — but some vendors (Anthropic
 * itself, some legacy SOAP gateways) put it elsewhere.
 */
export type TokenInjection = "BearerHeader" | { "CustomHeader": { name: string, } } | { "QueryParam": { name: string, } };

export type TokenOverride = { provider: string, token: string, };

export type TokensConfig = {
/**
 * Legacy fields — kept for backward compat when reading old config.toml
 */
anthropic?: string | null, openai?: string | null, google?: string | null,
/**
 * All API keys (new multi-key system)
 */
keys: Array<ApiKey>, disabled_overrides: Array<string>, };

export type TokenUsageSummary = { total_tokens: number, total_cost_usd: number, discussion_tokens: number, workflow_tokens: number, by_provider: Array<ProviderUsage>, by_project: Array<ProjectUsage>, top_discussions: Array<UsageEntry>, top_workflows: Array<UsageEntry>, daily_history: Array<DailyUsage>, };

export type TrackerSourceConfig = { "type": "GitHub", owner: string, repo: string, };

/**
 * 0.6.0 UX pass — optional payload for `POST /api/workflows/:id/trigger`.
 * `variables` carries user-entered values matching `Workflow.variables`
 * (manual launch). The keys become `{{var_name}}` in step prompts via
 * the run's `trigger_context`. Empty/missing body keeps the legacy
 * "trigger with no variables" flow working — back-compat for tracker
 * triggers that don't need variables.
 */
export type TriggerWorkflowRequest = { variables?: Record<string, string>, };

/**
 * `PUT /api/mcps/custom/:server_id` — 0.8.6 — update a Custom API
 * plugin's spec (name, base_url, description, docs_url, fields,
 * endpoints) WITHOUT re-creating the plugin. Closes the UX gap where
 * users had to delete-and-recreate to fix a typo or add endpoints
 * surfaced after a doc fetch.
 *
 * Critical invariants:
 * - `server_id` is preserved across the update. The slug is baked
 *   into the id at creation (`custom-{slug}-{nano}`); renaming the
 *   plugin must NOT mutate it, otherwise every `McpConfig.server_id`
 *   referencing it and every workflow `ApiCall` step's
 *   `api_plugin_slug` referencing it would break silently.
 * - `source` + `transport` are preserved from the existing row (no
 *   weird "Manual → Registry" flips through the back door).
 * - Encrypted env stored per-config in `mcp_configs` is NOT touched
 *   here. Effects:
 *     * Add a field via this endpoint → existing configs are still
 *       OK; the user opens "edit env" to fill the new key.
 *     * Remove a field → orphan env entries persist in the
 *       `mcp_configs` row but stop being surfaced (since the spec
 *       no longer declares the key). Harmless; can be GC'd later.
 * - Endpoint deletion = endpoint disappears from `ApiSpec.endpoints`
 *   → any existing workflow `ApiCall` step referencing it will fail
 *   at run-time with the existing "endpoint not in allowlist"
 *   diagnostic. Loud-and-clear failure, not silent corruption.
 *
 * 0.8.6 (#60) — response wrapper that surfaces orphan env keys (slugs
 * that vanished from `api_spec.config_keys` but still exist in at
 * least one linked config's encrypted env). The frontend reads this
 * and offers a one-click cleanup so the user doesn't ship orphan
 * secrets to disk via the next host_sync pass.
 */
export type UpdateCustomSpecResponse = { server: McpServer,
/**
 * Keys that were removed from the spec (or renamed) but still
 * exist in at least one linked config's stored env. Sorted alpha
 * for deterministic UI rendering. Empty when no rename / removal
 * happened (most common case).
 */
orphan_env_keys: Array<string>, };

export type UpdateDiscussionRequest = { title?: string | null, archived?: boolean | null, pinned?: boolean | null, skill_ids?: Array<string> | null, profile_ids?: Array<string> | null, directive_ids?: Array<string> | null,
/**
 * Change project: Some(Some("id")) = set, Some(None) = unset, absent = no change
 */
project_id?: string | null | null,
/**
 * Change model tier for this discussion.
 */
tier?: ModelTier | null,
/**
 * Switch the primary agent for this discussion.
 */
agent?: AgentType | null,
/**
 * Change the auto-summary policy. Persists in `discussions.summary_strategy`.
 */
summary_strategy?: SummaryStrategy | null, };

export type UpdateMcpConfigRequest = { label?: string | null, env?: Record<string, string> | null, args_override?: Array<string> | null, is_global?: boolean | null, include_general?: boolean | null, host_sync?: HostSyncMode | null, };

export type UpdateMcpContextRequest = { content: string, };

export type UpdateWorkflowRequest = { name?: string | null, project_id?: string | null | null, trigger?: WorkflowTrigger | null, steps?: Array<WorkflowStep> | null, actions?: Array<WorkflowAction> | null, safety?: WorkflowSafety | null, workspace_config?: WorkspaceConfig | null, concurrency_limit?: number | null, guards?: WorkflowGuards | null,
/**
 * Replace the artifact map entirely when present. To clear all
 * declarations, send `Some({})`. Omit the field to leave existing
 * declarations untouched.
 */
artifacts?: Record<string, ArtifactSpec> | null,
/**
 * Replace the rollback chain entirely when present. To clear it,
 * send `Some([])`. Omit to leave the existing chain untouched.
 */
on_failure?: Array<WorkflowStep> | null,
/**
 * Replace the Exec allowlist entirely when present. Send
 * `Some([])` to disable Exec steps; omit to leave it untouched.
 */
exec_allowlist?: Array<string> | null,
/**
 * Replace launch-time variables entirely when present. `Some([])`
 * to clear, omit to keep existing.
 */
variables?: Array<PromptVariable> | null, enabled?: boolean | null,
/**
 * Pin/unpin as favorite; omit to leave untouched.
 */
pinned?: boolean | null, };

/**
 * Response after uploading a context file.
 */
export type UploadContextFileResponse = { file: ContextFile,
/**
 * Suggested skill IDs based on file extension
 */
suggested_skills: Array<string>, };

/**
 * A ranked usage entry (for top N lists)
 */
export type UsageEntry = { id: string, name: string, tokens_used: number, cost_usd: number, };

/**
 * Per-model cost within a row — lets the frontend roll up by agent
 * (model name prefix → Claude / Codex / Gemini …) for the breakdown chart.
 */
export type UsageModelBreakdown = { model_name: string, cost: number, total_tokens: number, };

/**
 * A full usage report for one period kind.
 */
export type UsageReport = {
/**
 * `daily` | `weekly` | `monthly`.
 */
period_kind: string, rows: Array<UsageRow>, totals: UsageTotals,
/**
 * Distinct agents that appear across the rows (for header chips).
 */
agents_detected: Array<string>, };

/**
 * One row of a usage report (a date / week / month, possibly per-agent).
 */
export type UsageRow = {
/**
 * The period label — a date (`2026-05-27`), week, month, or session id.
 */
period: string,
/**
 * Agent slug as ccusage reports it (`all`, `claude`, `codex`, `gemini`, …).
 */
agent: string, models_used: Array<string>,
/**
 * Per-model cost split, for agent-level rollup on the frontend.
 */
model_breakdowns: Array<UsageModelBreakdown>, input_tokens: number, output_tokens: number, cache_creation_tokens: number, cache_read_tokens: number, total_tokens: number, total_cost: number, };

/**
 * Aggregate totals across all rows.
 */
export type UsageTotals = { input_tokens: number, output_tokens: number, cache_creation_tokens: number, cache_read_tokens: number, total_tokens: number, total_cost: number, };

export type VersionCheck = { current: string, latest: string | null, release_url: string | null, up_to_date: boolean, };

export type WaitForPeerMessage = { sort_order: number, role: string, agent_type: string | null, content: string, timestamp: string,
/**
 * Author pseudo for messages that arrived from a PEER instance (federated)
 * or a human; `None` for our own local appends. Lets the wait correctly
 * treat a same-`agent_type` peer (e.g. another ClaudeCode instance) as a
 * real peer instead of filtering it out as "self".
 */
author_pseudo: string | null, };

export type WaitForPeerResponse = {
/**
 * `true` when the loop hit the timeout without any new messages.
 * Lets the caller (the agent's MCP tool) decide whether to retry
 * or surface "no activity in the last 60s" to the user.
 */
timed_out: boolean,
/**
 * New messages since `since_sort_order` (empty when `timed_out=true`).
 */
messages: Array<WaitForPeerMessage>,
/**
 * Highest sort_order in the returned batch (or the input
 * `since_sort_order` when timed out). Lets the agent advance its
 * `since` cursor without inspecting the messages.
 */
latest_sort_order: number,
/**
 * stab-3 — server-computed pacing: apply `next_delay_seconds` before
 * the next wait, verbatim. Hot (short interval) while a User message is
 * within the attention lease; otherwise the next DETERMINISTIC step of
 * the cold backoff ramp, derived from the elapsed silence.
 */
pacing: PacingState,
/**
 * Presence-gap fix — when `timed_out`, the RFC3339 instant this session
 * intends to poll again (`now + pacing.next_delay_seconds`). Consumed by
 * the MCP CALLER (to schedule its next wait); the participants UI does
 * NOT read this field — it derives "dormant" from the paired `waiting`
 * activity (generic label, no countdown). `None` on a delivery (the
 * caller replies now, not later).
 */
next_poll_at: string | null, };

export type Workflow = { id: string, name: string, project_id: string | null, trigger: WorkflowTrigger, steps: Array<WorkflowStep>, actions: Array<WorkflowAction>, safety: WorkflowSafety, workspace_config: WorkspaceConfig | null, concurrency_limit: number | null,
/**
 * Execution limits (timeout, LLM calls cap, loop detection). 0.7.0 —
 * Phase 1 of the Auto-Dev workflow expansion. `None` = use the soft
 * backend defaults (120 min wall-clock, 100 LLM calls, 10 revisits
 * per step) so existing workflows get the safety net automatically.
 * Explicit `Some(WorkflowGuards { ... })` lets users override per
 * workflow without touching server config.
 */
guards?: WorkflowGuards | null,
/**
 * 0.7.0 Phase 3 — declared artifacts the workflow's steps may write.
 * Map key = artifact name (referenced in steps as `{{artifacts.<name>}}`).
 * Value = relative path inside the run's workspace where Kronn
 * persists whatever the agent emits in `---ARTIFACT:<name>---...---END_ARTIFACT---`.
 * Empty by default (rétro-compat). Reading an undeclared artifact
 * from a template renders empty string — no hard error so partial
 * pipelines (artifact only set on round 2+ of a loop) keep flowing.
 */
artifacts?: Record<string, ArtifactSpec>,
/**
 * 0.7.0 Phase 7 — compensating steps run when the main pipeline ends
 * in `RunStatus::Failed`. Empty by default (rétro-compat). NOT fired on
 * Cancelled / StoppedByGuard / Gate-Reject — those are intentional
 * stops, the operator doesn't want any further automation. Each
 * rollback step sees the regular template context PLUS
 * `{{failed_step.name}}` and `{{failed_step.output}}` so the
 * rollback can react to what specifically broke. If a rollback step
 * itself fails, subsequent rollback steps are skipped (no recursive
 * compensation) — the run remains `Failed`.
 */
on_failure?: Array<WorkflowStep>,
/**
 * 0.7.0 Phase 5 — allowlist of binaries that `StepType::Exec` is
 * permitted to invoke for this workflow. Empty list = `Exec` steps
 * are completely disabled (default: safe). Match is exact on the
 * binary name (no glob, no regex, no path), so `npm` and
 * `/usr/local/bin/npm` are different — only the bare name passes.
 * Validate-time error when an Exec step's `exec_command` isn't in
 * this list.
 */
exec_allowlist?: Array<string>,
/**
 * 0.6.0 UX pass — variables prompted at manual launch time (mirrors
 * `QuickPrompt.variables`). When the user clicks "Lancer" on a
 * workflow with `trigger == Manual` and `!variables.is_empty()`,
 * the launcher shows a form asking for one value per variable;
 * the values are merged into the run's `trigger_context` so they
 * resolve as `{{var_name}}` in step prompts. Empty for trigger-
 * driven workflows that get their context from the trigger
 * (issue.* / cron payload). Required variables fail launch when
 * the value is empty.
 */
variables?: Array<PromptVariable>, enabled: boolean,
/**
 * User-pinned / favorite workflow — surfaces first in the Workflows
 * page list, same affordance as `Discussion::pinned`.
 */
pinned: boolean, created_at: string, updated_at: string, };

export type WorkflowAction = { "type": "CreatePr", title_template: string, body_template: string, branch_template: string, } | { "type": "CommentIssue", body_template: string, } | { "type": "UpdateTrackerStatus", status: string, } | { "type": "CreateIssue", title_template: string, body_template: string, };

/**
 * Self-contained envelope produced by `GET /api/workflows/:id/export`.
 * Designed to be saved to disk, mailed, attached to a Github issue, etc.
 * `version: 1` is the current shape; future incompatible changes bump
 * the version and add a migration path at import time.
 */
export type WorkflowExportEnvelope = {
/**
 * Discriminator: always `"kronn.workflow"` for this envelope.
 */
kind: string,
/**
 * Schema version. Bumped on incompatible changes.
 */
version: number,
/**
 * ISO-8601 timestamp of the export, for audit and human readability.
 */
exported_at: string,
/**
 * The workflow definition. `id`, `project_id`, `created_at`,
 * `updated_at`, `enabled` are kept in the wire format (so a
 * roundtrip is lossless to inspect) but DROPPED at import — the
 * importer mints fresh values for those fields.
 */
workflow: Workflow,
/**
 * QPs referenced by `BatchQuickPrompt` steps. Bundled so the
 * importer doesn't need to fetch them separately. Empty when no
 * step references a QP.
 */
referenced_quick_prompts?: Array<QuickPrompt>,
/**
 * #10 — sub-workflows referenced (transitively) by `SubWorkflow` steps,
 * bundled so a clone/import recreates the whole parent+child graph in one
 * atomic operation and remaps `sub_workflow_id` to the fresh child ids.
 * Empty when the workflow has no SubWorkflow steps. Excludes the root.
 */
referenced_workflows?: Array<Workflow>, };

/**
 * Per-workflow execution limits enforced by the runner. Each field is
 * optional: `None` means "use the runner's soft default". 0 / negative
 * values are rejected at save time (`api::workflows::validate_guards`).
 */
export type WorkflowGuards = {
/**
 * Wall-clock max duration of the run from `WorkflowRun.started_at`.
 * Includes time spent in `WaitingApproval` (Phase 2 GATE) UNLESS the
 * runner is later updated to pause the timer there. Triggers
 * `RunStatus::StoppedByGuard` + `RunEvent::GuardTriggered { kind: Timeout }`.
 */
timeout_seconds?: number | null,
/**
 * Hard cap on the number of LLM-spending steps. `Agent` counts as 1,
 * `BatchQuickPrompt` counts as N (post-fan-out, after items are
 * resolved), `ApiCall` and `Notify` count as 0. Prevents a Goto
 * loop or a misconfigured workflow from burning a budget overnight.
 */
max_llm_calls?: number | null,
/**
 * Max number of times the runner is allowed to revisit the same
 * step (via `ConditionAction::Goto`). Per-step counter, not total
 * iterations — a 100-step linear workflow won't trigger this.
 * Defaults to 10. Triggers `RunStatus::StoppedByGuard`.
 */
loop_detection_max_revisits?: number | null, };

export type WorkflowRun = { id: string, workflow_id: string, status: RunStatus, trigger_context: any, step_results: Array<StepResult>, tokens_used: number, workspace_path: string | null, started_at: string, finished_at: string | null,
/**
 * Linear workflow run vs batch fan-out. Default "linear" for backward
 * compatibility with existing runs created before Phase 1b.
 */
run_type: string,
/**
 * For batch runs: target number of child discussions. 0 for linear runs.
 */
batch_total: number,
/**
 * For batch runs: number of successfully-completed child discussions.
 */
batch_completed: number,
/**
 * For batch runs: number of child discussions that ended with an error.
 */
batch_failed: number,
/**
 * For batch runs: display name shown in the sidebar group header.
 * Example: "Cadrage to-Frame — 10 avr 14:00".
 */
batch_name: string | null,
/**
 * Link a child batch run back to the linear workflow run that spawned it
 * via a `BatchQuickPrompt` step. `None` for top-level runs (both linear
 * runs and manual batch runs triggered from the UI).
 */
parent_run_id: string | null,
/**
 * 0.7.0 Phase 6 — durable state map carried across iterations and
 * resume cycles (Gate, restart). Agents write entries by emitting
 * `---STATE:<key>=<value>---` blocks in their output (parsed
 * alongside artifacts); steps read them via `{{state.<key>}}`.
 * Used for retry counters, accumulated verdicts, and any other
 * cross-iteration memory that doesn't belong in step outputs.
 * Empty by default. Persisted as a JSON object on the run row.
 */
state?: Record<string, string>,
/**
 * 0.7.0 — branches preserved by the runner during worktree cleanup
 * because their HEAD held commits not on any known base ref. The UI
 * surfaces them on the run detail page so the operator can recover
 * the work even when the agent's push step failed (pre-push hook
 * blocked, no auth, network down, …).
 */
produced_branches?: Array<ProducedBranch>,
/**
 * Provenance enrichment (DERIVED, not persisted). When this run is a
 * sub-workflow child (`parent_run_id` set), these resolve the parent run's
 * workflow id + name + tick time so the UI can render
 * "↳ depuis <parent> · <date>" (clickable) without an extra round-trip.
 * Populated ONLY by `list_runs` / `get_run` via a single batch JOIN;
 * `None` on insert, on top-level runs, and when the parent was deleted.
 */
parent_workflow_id?: string | null, parent_workflow_name?: string | null, parent_run_started_at?: string | null, };

export type WorkflowRunSummary = { id: string, status: RunStatus, started_at: string, finished_at: string | null, tokens_used: number, };

export type WorkflowSafety = { sandbox: boolean, max_files?: number | null, max_lines?: number | null, require_approval: boolean, };

export type WorkflowStep = { name: string, step_type: StepType, description?: string | null, agent: AgentType, prompt_template: string, mode: StepMode, output_format: StepOutputFormat, mcp_config_ids?: Array<string>, agent_settings?: AgentSettings | null, on_result?: Array<StepConditionRule>,
/**
 * #8 — first-class handling of a step STALL/timeout. When the step exhausts
 * its attempts on a stall (no output for `stall_timeout_secs`), the runner
 * applies this action (Goto a recovery/notify step, or Stop gracefully)
 * instead of failing the whole run. `None` = legacy behaviour (the stall
 * fails the step and tips into the rollback chain). Stored in `steps_json`
 * alongside `on_result` — no migration; absent on older workflows.
 */
on_timeout?: ConditionAction | null, stall_timeout_secs?: number | null, retry?: RetryConfig | null, delay_after_secs?: number | null, skill_ids?: Array<string>, profile_ids?: Array<string>, directive_ids?: Array<string>,
/**
 * Id of the Quick Prompt to fan out. Required for BatchQuickPrompt steps.
 */
batch_quick_prompt_id?: string | null,
/**
 * Template expression that resolves to the list of items. Each item
 * becomes one child discussion. Examples:
 * - `"{{steps.fetch_tickets.data.tickets}}"` — structured JSON array
 * - `"{{steps.fetch_tickets.output}}"` — raw text (parsed as one id per line)
 *
 * Required for BatchQuickPrompt steps.
 */
batch_items_from?: string | null,
/**
 * If true (default), the linear workflow run waits for all child
 * discussions to finish before moving to the next step. Uses the existing
 * `BatchRunFinished` WS broadcast as the wake signal — no polling.
 * If false, the batch is fired and the linear run advances immediately.
 */
batch_wait_for_completion?: boolean | null,
/**
 * Safety cap for the number of items spawned by this step. Falls back to
 * the global 50-item cap enforced by `create_batch_run` when unset.
 */
batch_max_items?: number | null,
/**
 * Workspace mode for each batch child discussion: `"Direct"` (default)
 * or `"Isolated"` for per-disc git worktrees. Isolated is required when
 * the agents will write code in parallel — otherwise they clobber each
 * other in the main working tree. Requires the workflow to have a
 * project_id, otherwise the step fails early.
 */
batch_workspace_mode?: string | null,
/**
 * Chain additional Quick Prompts after the initial one inside each
 * child discussion. Each QP is auto-sent as a User message once the
 * previous agent response completes, and the agent is re-fired.
 * The batch progress counter only increments after the ENTIRE chain
 * (initial QP + all chained QPs) finishes for a given discussion.
 * Example: `["qp-review", "qp-summary"]` after the primary `batch_quick_prompt_id`.
 */
batch_chain_prompt_ids?: Array<string>,
/**
 * Concurrent fan-out cap for `StepType::BatchApiCall` (HTTP path only).
 * `None` falls back to a conservative default (5). HTTP can scale much
 * higher than agent runs (no LLM, just network) but providers rate-limit
 * — Jira/GitHub typically OK up to 10-20 in parallel, beyond that you
 * risk 429s. Distinct from BatchQuickPrompt (which goes through the
 * global agent_semaphore).
 */
batch_concurrent_limit?: number | null,
/**
 * 0.6.0 — when set on a `BatchApiCall` step, the executor loads the
 * referenced `QuickApi` from the DB at run-time and uses its API
 * config (plugin, endpoint, method, body, etc.) instead of the
 * step's own inline `api_*` fields. Mirror of `batch_quick_prompt_id`.
 * `None` keeps inline-config behaviour. 0.7+ — étendu à `StepType::ApiCall`
 * (single, non-batch) avec la même sémantique per-field override.
 */
quick_api_id?: string | null,
/**
 * 0.7+ — référence vers un `QuickPrompt` saved. Quand set sur un step
 * `Agent`, le runner charge le QP au run-time et utilise son
 * `prompt_template`, son `agent`, son `tier`, et ses `skill_ids` ; les
 * fields renseignés sur le step écrasent ceux du QP (per-field override).
 * Permet de définir un prompt canonique côté Quick Prompts et de le
 * réutiliser dans N workflows. Pas de variables au niveau step :
 * les `{{var}}` du QP sont résolus avec le `TemplateContext` du
 * workflow (launch variables / state / steps.X / etc.). Mirror du
 * pattern `quick_api_id` pour les ApiCall.
 */
quick_prompt_id?: string | null,
/**
 * Webhook configuration for `StepType::Notify`. URL and body support
 * the same `{{steps.X.output}}` / `{{steps.X.data}}` templates as
 * agent prompts.
 */
notify_config?: NotifyConfig | null,
/**
 * Registry slug of the plugin to invoke (e.g. `"chartbeat"`, `"jira"`).
 * The slug resolves to an `ApiSpec` in the plugin registry; the request
 * base URL comes from that spec and is NEVER templated from the step.
 */
api_plugin_slug?: string | null,
/**
 * `McpConfig.id` of the specific credential set to use. The plugin can
 * be configured multiple times per project (e.g. two Jira instances);
 * this picks one. Decrypted env lives in the DB row and is loaded at
 * step execution via `collect_active_api_plugins`.
 */
api_config_id?: string | null,
/**
 * Endpoint path as declared in `ApiSpec.endpoints[].path` — prefix-
 * matched against the allowlist in the executor so a step can't reach
 * arbitrary paths under the plugin's `base_url`.
 */
api_endpoint_path?: string | null,
/**
 * HTTP method override. Defaults to the method of the endpoint in the
 * plugin registry. Uppercase: `GET | POST | PUT | PATCH | DELETE`.
 */
api_method?: string | null,
/**
 * Path-segment parameters (e.g. `/repos/{owner}/{repo}` → `{owner}` and
 * `{repo}`). The executor scans `api_endpoint_path` for `{key}` tokens
 * and substitutes each match with the value from this map at request
 * time. Values support `{{steps.X.data}}` templates so a previous
 * fetch can drive the segment dynamically. Tokens with no entry stay
 * literal — the request will fail because `/repos/{owner}/...` is not
 * a real GitHub path. This way the spec-declared template stays in
 * `api_endpoint_path` (round-trip safe across re-edits) while the
 * concrete values live separately.
 */
api_path_params?: { [key in string]: string } | null,
/**
 * Query-string parameters. Values support `{{steps.X.data}}` templates.
 * Rendered values are percent-encoded AFTER template expansion to
 * prevent injection (`&` / `=` in a templated value would corrupt the
 * query otherwise).
 */
api_query?: { [key in string]: string } | null,
/**
 * Extra headers (auth headers come from the plugin spec, not here).
 * String values templatable; keys are literal.
 */
api_headers?: { [key in string]: string } | null,
/**
 * JSON body for POST/PUT/PATCH. Rendered by walking the `Value` tree
 * and interpolating string leaves only — no string-level interpolation,
 * which would allow JSON injection via templated content.
 */
api_body?: JsonValue | null,
/**
 * How to extract a value from the JSON response. The extracted `data`
 * is what downstream steps read via `{{steps.X.data}}`; batch QP steps
 * expect an array and fail-fast if it's a scalar.
 */
api_extract?: ExtractSpec | null,
/**
 * Pagination strategy. `Auto` (default-ish) inspects the response for
 * `nextPageToken` / `startAt`+`total` / `page` and walks accordingly,
 * concatenating arrays. Hard-capped at 50 pages to prevent runaway.
 */
api_pagination?: PaginationSpec | null,
/**
 * Per-request timeout in milliseconds. Defaults to 30 000 ms.
 */
api_timeout_ms?: number | null,
/**
 * Max retries on 5xx / 429 with exponential backoff. Defaults to 2.
 * Idempotent GETs retry freely; endpoints flagged `side_effect: true`
 * in the plugin spec skip retry entirely.
 */
api_max_retries?: number | null,
/**
 * Context variable name under which the extracted data is stored.
 * Downstream steps reference it as `{{steps.<output_var>.data}}`.
 * Defaults to the step's `name` field when unset.
 */
api_output_var?: string | null,
/**
 * Markdown message shown to the operator on the run-detail page.
 * Templates supported. Empty string falls back to a default
 * "Décision humaine requise" placeholder in the UI.
 */
gate_message?: string | null,
/**
 * Step name to jump to when the operator picks "Request Changes".
 * `None` → falls back to the previous step (one step back), which
 * matches the Auto-Dev `pause_pre_merge` → `goto: implement` pattern.
 * Set explicitly to a step name for non-default targets.
 */
gate_request_changes_target?: string | null,
/**
 * 0.7.0 P1-1 — optional webhook URL to POST when the run enters
 * `WaitingApproval` on this gate. Best-effort fire-and-forget;
 * failures are logged but never block the run. Templated, so
 * users can drop `{{state.slack_url}}` etc. Body :
 * `{run_id, workflow_id, workflow_name, step_name, message}`.
 * The "ping ops when a Gate fires" use case Cyndie + Antony
 * flagged as blocker for team-wide deployment.
 */
gate_notify_url?: string | null,
/**
 * 0.8.6 (#25) — `true` means create a git commit checkpoint before
 * pausing the run on this Gate. The SHA is stored in
 * `WorkflowRun.state["checkpoint:<step.name>"]`. On Goto from this
 * gate's `gate_request_changes_target`, the runner `git reset
 * --hard` to that SHA before re-running the target — makes
 * Gate→implement loops idempotent (re-implement on a clean tree,
 * not on top of the previous cycle's noise). Defaults to `false`
 * (no behaviour change for existing workflows). Skipped silently
 * in `Isolated` worktree mode (the worktree already has its own
 * branch). Skipped + warned on non-git project_path.
 */
gate_checkpoint_before?: boolean | null,
/**
 * 0.8.6 (#26) — opt-in countdown in seconds. When set on a Gate
 * step and the run enters `WaitingApproval`, the runner spawns a
 * background task that auto-approves the gate after this delay
 * if no human has decided. `None` = manual forever (default).
 * Valid range : `1..=86400` (1s to 24h). Out-of-range values are
 * rejected at workflow save time. Use cases : low-stakes
 * validation gates, nocturnal AutoPilot runs. NEVER set on a
 * destructive merge / deploy gate.
 */
gate_auto_approve_after_secs?: number | null,
/**
 * Binary to execute. Must match an entry in `Workflow.exec_allowlist`
 * exactly (no glob, no regex). NOT templated — locked at save time.
 */
exec_command?: string | null,
/**
 * Arguments passed verbatim to the binary. Each entry is one argv
 * element. Templates `{{steps.X}}` are rendered, but the result
 * becomes a literal argument — no shell metachar interpretation.
 */
exec_args?: Array<string>,
/**
 * Per-step timeout in seconds. Defaults to 300s (5 min) if unset.
 * Hard-capped at 1800s (30 min) at validate time.
 */
exec_timeout_secs?: number | null,
/**
 * 0.8.2 — Optional setup command that runs IMMEDIATELY BEFORE the
 * main `exec_command`. Designed for the worktree-dependency-install
 * pattern: `composer install` / `pnpm install` / etc., so the main
 * command (e.g. `make test`) has the artifacts it needs even though
 * the worktree starts with only git-tracked files (no `vendor/`,
 * no `node_modules/`, no `target/`).
 *
 * Same allowlist + char-validation + timeout rules as `exec_command`.
 * Templates `{{steps.X}}` resolve normally. If the setup fails
 * (non-zero exit), the step fails IMMEDIATELY without running the
 * main command — the user sees the setup's stderr.
 *
 * When `None`, the executor skips straight to `exec_command` (no
 * extra subprocess overhead). Backward-compatible default.
 */
exec_setup_command?: string | null,
/**
 * Argv for `exec_setup_command`. Same literal-argv semantics as
 * `exec_args` (no shell, no metachar interpretation). Use
 * `[\"-c\", \"<oneliner>\"]` if you need to wrap a shell line.
 */
exec_setup_args?: Array<string>,
/**
 * 0.8.8 — Optional data piped to the MAIN command's **stdin**.
 * Templated like `exec_args` (`{{steps.X.data_json}}` etc.) and
 * rendered to a literal string, but unlike argv it is NOT subject to
 * the OS `ARG_MAX` ceiling (~128 KB on Linux) — so a large reshaped
 * payload (e.g. an enriched Jira backlog) can be fed to a `jq` /
 * reshape / node script without exploding the argument list. When
 * `None`, stdin stays `/dev/null` (backward-compatible). Applies to
 * the main `exec_command` only, not `exec_setup_command`.
 */
exec_stdin?: string | null,
/**
 * Payload JSON émis par le step. Validé au save (parse JSON valide,
 * taille raisonnable). Aucun templating au runtime — la valeur est
 * retournée telle quelle, ce qui permet à un downstream batch de la
 * consommer via `{{steps.<name>.data}}` exactement comme une réponse
 * API. Si tu as besoin de `{{var}}` dans le payload, mets ça dans
 * un Agent step ou un ApiCall.
 */
json_data_payload?: JsonValue | null,
/**
 * 2026-06-11 (Phase 1) — for `StepType::SubWorkflow`: the id of the
 * workflow to run as a nested child. Required for that step type
 * (enforced at save). `None` for every other step type. Mirrors the
 * `batch_quick_prompt_id` / `quick_api_id` "reference another entity by
 * id" pattern, keeping `StepType` a bare tag.
 */
sub_workflow_id?: string | null,
/**
 * 2026-06-12 (Phase 3b MVP) — for `StepType::SubWorkflow`: when set, the
 * child workflow is executed ONCE PER ITEM of the JSON array stored in
 * this workspace-relative file (e.g. `.kronn/tasks.json`, written by a
 * triage step). Sequential fan-out in the SHARED parent worktree: before
 * each child run the engine writes the item to `.kronn/current_task.json`
 * so the child's prompts read ONLY their slice (scoped context = fewer
 * tokens + more deterministic). `None` = single child run (Phase 1/2).
 */
sub_workflow_foreach_file?: string | null,
/**
 * 2026-06-13 — "Multi-agent review" advanced option on an Agent step.
 * When set, the step runs its own agent normally, THEN opens a shared
 * Kronn discussion and invites a SECOND agent (a different model family,
 * e.g. Codex reviewing Claude's plan) to debate the output until they
 * reach agreement — instead of a successive `Goto` re-run loop that
 * re-reads everything from scratch each round. Cheaper (the reviewer
 * reads the artifact once, then only the conversation delta) and a real
 * back-and-forth rather than a file relay. `None` = plain Agent step.
 */
multi_agent_review?: MultiAgentReviewConfig | null, };

export type WorkflowSuggestion = { id: string, title: string, description: string, reason: string, required_mcps: Array<string>, audience: string, complexity: string, trigger: WorkflowTrigger, steps: Array<WorkflowStep>, };

export type WorkflowSummary = { id: string, name: string, project_id: string | null, project_name: string | null, trigger_type: string, step_count: number,
/**
 * Number of steps missing required config (unwired API plugin/endpoint,
 * batch QP ref, agent prompt, …). Lets the card flag a freshly
 * AI-generated workflow that still needs wiring, without fetching its
 * full step list. 0 = ready to run.
 */
misconfigured_step_count: number, enabled: boolean,
/**
 * User-pinned / favorite — the list surfaces pinned workflows first.
 */
pinned: boolean, last_run: WorkflowRunSummary | null, created_at: string, };

export type WorkflowTrigger = { "type": "Cron", schedule: string, } | { "type": "Tracker", source: TrackerSourceConfig, query: string, labels: Array<string>, interval: string, } | { "type": "Manual" };

export type WorkspaceConfig = { hooks: WorkspaceHooks,
/**
 * When true, a run MUST get its own git worktree — if `Workspace::create`
 * fails, the run is aborted instead of silently falling back to the main
 * checkout. Set on code-pushing presets (Ticket→PR, AutoDev…) where
 * running agents that `git push` / mutate files in the developer's main
 * working tree is dangerous. Default false → legacy warn-and-fallback
 * (fine for read-only audit/briefing workflows on non-git projects).
 */
require_isolation: boolean, };

export type WorkspaceHooks = { after_create?: string | null, before_run?: string | null, after_run?: string | null, before_remove?: string | null, };

/**
 * Real-time message exchanged between Kronn instances via WebSocket.
 */
export type WsMessage = { "type": "presence", from_pseudo: string, from_invite_code: string, online: boolean, } | { "type": "ping", timestamp: number, } | { "type": "pong", timestamp: number, } | { "type": "chat_message", shared_discussion_id: string, message_id: string, from_pseudo: string, from_avatar_email: string | null, from_invite_code: string, content: string, timestamp: number,
/**
 * Author role + agent identity, so a federated AGENT reply lands as
 * an Agent message (with its CLI name) on the peer instead of a
 * generic "User". `#[serde(default)]` keeps frames from an older peer
 * (no field on the wire) decoding to the historical behaviour
 * (role=User, agent_type=None).
 */
role: MessageRole, agent_type: AgentType | null, } | { "type": "discussion_invite", shared_discussion_id: string, title: string, from_pseudo: string, from_invite_code: string, } | { "type": "disc_sync_request", shared_discussion_id: string,
/**
 * Unix-millis of the newest message the requester already has for this
 * shared disc (0 if none). The responder sends everything strictly
 * newer.
 */
since_timestamp: number, } | { "type": "file_attached", shared_discussion_id: string, message_id: string, file_id: string, filename: string, mime_type: string, size: number,
/**
 * The HOST's invite code — the receiver resolves it to a contact URL to
 * fetch the binary from.
 */
from_invite_code: string,
/**
 * F15+ — UI hint. The receiver emits this LOCALLY twice: `pending:true`
 * the moment the announcement arrives (front shows "downloading…") and
 * `pending:false` once the binary is stored (front shows the file). The
 * cross-wire announcement carries `false` (default); only the receiver's
 * own local emits use it. `#[serde(default)]` for old-peer compat.
 */
pending: boolean, } | { "type": "batch_run_finished", run_id: string,
/**
 * Id of the child discussion whose completion triggered the final tick.
 * The frontend uses it to clear its per-disc `sendingMap` spinner, since
 * batch children are fire-and-forget (no SSE stream consumer on the client
 * to drive the usual cleanup path).
 */
discussion_id: string, batch_name: string | null, batch_total: number, batch_completed: number, batch_failed: number, } | { "type": "batch_run_progress", run_id: string,
/**
 * Id of the child discussion that just completed — frontend uses it to
 * clear the per-disc sendingMap indicator.
 */
discussion_id: string, batch_total: number, batch_completed: number, batch_failed: number, } | { "type": "batch_run_child_started", run_id: string,
/**
 * Id of the child discussion whose agent run is starting.
 */
discussion_id: string, } | { "type": "batch_run_child_queued", run_id: string, discussion_id: string, } | { "type": "workflow_run_updated", run_id: string, workflow_id: string, status: string,
/**
 * Index of the currently-running (or just-completed) step. -1 when
 * the run starts and no step is in flight yet.
 */
step_index: number, total_steps: number,
/**
 * Step name at `step_index`, or null when between steps.
 */
current_step: string | null, } | { "type": "partial_response_recovered", discussion_ids: Array<string>, } | { "type": "agent_runs_interrupted", discussion_ids: Array<string>, } | { "type": "audit_finished", project_id: string, status: string, last_completed_step: number, total_steps: number, warned_steps: Array<number>, discussion_id: string | null, };
