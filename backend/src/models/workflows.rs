// Workflow definitions, runs, batch runs, and the artifact / guard / safety types.
//
// The workflow ecosystem accumulates a lot — keeping it as one file is
// easier than splitting WorkflowStep / WorkflowRun / BatchRun into
// three more sub-modules: they share helpers (StepType / WorkflowGuards)
// and are always touched together when adding a new step kind.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentType, ModelTier, PromptVariable};


#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub project_id: Option<String>,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
    pub actions: Vec<WorkflowAction>,
    pub safety: WorkflowSafety,
    pub workspace_config: Option<WorkspaceConfig>,
    pub concurrency_limit: Option<u32>,
    /// Execution limits (timeout, LLM calls cap, loop detection). 0.7.0 —
    /// Phase 1 of the Auto-Dev workflow expansion. `None` = use the soft
    /// backend defaults (120 min wall-clock, 100 LLM calls, 10 revisits
    /// per step) so existing workflows get the safety net automatically.
    /// Explicit `Some(WorkflowGuards { ... })` lets users override per
    /// workflow without touching server config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guards: Option<WorkflowGuards>,
    /// 0.7.0 Phase 3 — declared artifacts the workflow's steps may write.
    /// Map key = artifact name (referenced in steps as `{{artifacts.<name>}}`).
    /// Value = relative path inside the run's workspace where Kronn
    /// persists whatever the agent emits in `---ARTIFACT:<name>---...---END_ARTIFACT---`.
    /// Empty by default (rétro-compat). Reading an undeclared artifact
    /// from a template renders empty string — no hard error so partial
    /// pipelines (artifact only set on round 2+ of a loop) keep flowing.
    #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, ArtifactSpec>")]
    pub artifacts: ::std::collections::HashMap<String, ArtifactSpec>,
    /// 0.7.0 Phase 7 — compensating steps run when the main pipeline ends
    /// in `RunStatus::Failed`. Empty by default (rétro-compat). NOT fired on
    /// Cancelled / StoppedByGuard / Gate-Reject — those are intentional
    /// stops, the operator doesn't want any further automation. Each
    /// rollback step sees the regular template context PLUS
    /// `{{failed_step.name}}` and `{{failed_step.output}}` so the
    /// rollback can react to what specifically broke. If a rollback step
    /// itself fails, subsequent rollback steps are skipped (no recursive
    /// compensation) — the run remains `Failed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_failure: Vec<WorkflowStep>,
    /// 0.7.0 Phase 5 — allowlist of binaries that `StepType::Exec` is
    /// permitted to invoke for this workflow. Empty list = `Exec` steps
    /// are completely disabled (default: safe). Match is exact on the
    /// binary name (no glob, no regex, no path), so `npm` and
    /// `/usr/local/bin/npm` are different — only the bare name passes.
    /// Validate-time error when an Exec step's `exec_command` isn't in
    /// this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_allowlist: Vec<String>,
    /// 0.6.0 UX pass — variables prompted at manual launch time (mirrors
    /// `QuickPrompt.variables`). When the user clicks "Lancer" on a
    /// workflow with `trigger == Manual` and `!variables.is_empty()`,
    /// the launcher shows a form asking for one value per variable;
    /// the values are merged into the run's `trigger_context` so they
    /// resolve as `{{var_name}}` in step prompts. Empty for trigger-
    /// driven workflows that get their context from the trigger
    /// (issue.* / cron payload). Required variables fail launch when
    /// the value is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<PromptVariable>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Declared artifact in a workflow. Phase-3 minimal model — only
/// path + optional format hint. Path is resolved relative to the run's
/// workspace; absolute paths and `..` traversal are rejected at
/// validate-time (`validate_artifact_specs`).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ArtifactSpec {
    /// Workspace-relative path (e.g. `.kronn/plan.md`).
    pub path: String,
    /// Hint for the UI — `"markdown"`, `"yaml"`, `"json"`, `"text"` —
    /// informational only, the engine doesn't enforce a format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Per-workflow execution limits enforced by the runner. Each field is
/// optional: `None` means "use the runner's soft default". 0 / negative
/// values are rejected at save time (`api::workflows::validate_guards`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct WorkflowGuards {
    /// Wall-clock max duration of the run from `WorkflowRun.started_at`.
    /// Includes time spent in `WaitingApproval` (Phase 2 GATE) UNLESS the
    /// runner is later updated to pause the timer there. Triggers
    /// `RunStatus::StoppedByGuard` + `RunEvent::GuardTriggered { kind: Timeout }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Hard cap on the number of LLM-spending steps. `Agent` counts as 1,
    /// `BatchQuickPrompt` counts as N (post-fan-out, after items are
    /// resolved), `ApiCall` and `Notify` count as 0. Prevents a Goto
    /// loop or a misconfigured workflow from burning a budget overnight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_calls: Option<u32>,

    /// Max number of times the runner is allowed to revisit the same
    /// step (via `ConditionAction::Goto`). Per-step counter, not total
    /// iterations — a 100-step linear workflow won't trigger this.
    /// Defaults to 10. Triggers `RunStatus::StoppedByGuard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_detection_max_revisits: Option<usize>,
}

/// Soft backend defaults applied when `Workflow.guards` is `None` or any
/// individual field is `None`. Acts as a kill-switch against runaway runs
/// without forcing every user to configure the limits manually.
pub const DEFAULT_GUARD_TIMEOUT_SECS: u64 = 7200;       // 2 hours
pub const DEFAULT_GUARD_MAX_LLM_CALLS: u32 = 100;
pub const DEFAULT_GUARD_LOOP_MAX_REVISITS: usize = 10;

impl WorkflowGuards {
    /// Resolve the effective limits, falling back to backend defaults.
    /// Always returns concrete values — never `None`.
    pub fn resolved(&self) -> ResolvedGuards {
        ResolvedGuards {
            timeout_seconds: self.timeout_seconds.unwrap_or(DEFAULT_GUARD_TIMEOUT_SECS),
            max_llm_calls: self.max_llm_calls.unwrap_or(DEFAULT_GUARD_MAX_LLM_CALLS),
            loop_detection_max_revisits: self.loop_detection_max_revisits
                .unwrap_or(DEFAULT_GUARD_LOOP_MAX_REVISITS),
        }
    }

    /// Resolve from an `Option<WorkflowGuards>` — `None` (no overrides at
    /// all) yields full defaults.
    pub fn resolve_optional(opt: Option<&WorkflowGuards>) -> ResolvedGuards {
        opt.map(|g| g.resolved()).unwrap_or_else(|| WorkflowGuards::default().resolved())
    }
}

/// All-fields-resolved variant used internally by the runner so the
/// guard-check code never has to deal with `Option`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedGuards {
    pub timeout_seconds: u64,
    pub max_llm_calls: u32,
    pub loop_detection_max_revisits: usize,
}

/// Which guard tripped — surfaced verbatim in the SSE `GuardTriggered`
/// event so the frontend can render the right badge / toast / explainer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum GuardKind {
    Timeout,
    MaxLlmCalls,
    LoopDetection { step_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum WorkflowTrigger {
    Cron {
        schedule: String,
    },
    Tracker {
        source: TrackerSourceConfig,
        query: String,
        labels: Vec<String>,
        interval: String,
    },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum TrackerSourceConfig {
    GitHub {
        owner: String,
        repo: String,
    },
}

/// How a step's output is formatted and extracted.
/// `FreeText` (default): raw text, passed as-is via `{{previous_step.output}}`.
/// `Structured`: engine injects format instructions and extracts a JSON envelope
///   (`{"data": ..., "status": "OK|NO_RESULTS|ERROR", "summary": "..."}`).
///   Downstream steps can use `{{previous_step.data}}` and `{{previous_step.summary}}`.
/// `TypedSchema { schema }` (0.7.0 Phase 2): like Structured, but the
///   `data` field is validated against a JSON-Schema subset provided
///   by the workflow author. The schema is serialised into the prompt
///   so the LLM produces conforming output, and the engine rejects
///   non-conforming responses with a repair prompt. Used by Auto-Dev's
///   `validate_ticket` step (output_schema with status enum + score range).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepOutputFormat {
    #[default]
    FreeText,
    Structured,
    /// 0.7.0 Phase 2 — like `Structured` but the `data` field is
    /// constrained by a JSON-Schema subset (`type`, `properties`,
    /// `required`, `enum`, `items`, `min`/`max`, `minLength`/`maxLength`).
    /// The schema is serialised into the prompt and validated post-extract.
    /// Mismatches trigger a single repair prompt (same pattern as the
    /// vanilla `Structured` envelope flow).
    ///
    /// 0.8.3 — `on_invalid` controls what happens after repair × 1 still
    /// fails validation. Default (`Continue`) keeps the pre-0.8.3
    /// behavior: warn + use raw output, downstream steps deal with the
    /// garbage. `Fail` fails the step (and the run, unless guarded) with
    /// the validation error as `output`. Use `Fail` for high-stakes
    /// steps like Feasibility-Gated triage where downstream steps
    /// depend on the structured contract holding.
    TypedSchema {
        /// JSON Schema (subset). Stored verbatim; the runner serialises
        /// it into the prompt as-is so the LLM sees the exact shape it
        /// must produce.
        #[ts(type = "any")]
        schema: serde_json::Value,
        #[serde(default)]
        on_invalid: OnInvalid,
    },
}

/// What happens when `TypedSchema` validation still fails after a
/// single repair attempt. `Continue` = 0.7.0 behavior (warn + raw),
/// `Fail` = 0.8.3 strict mode for contract steps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum OnInvalid {
    /// Warn, keep the raw output, let downstream steps deal with it.
    /// Non-breaking default — every existing workflow keeps working.
    #[default]
    Continue,
    /// Mark the step `Failed` with the validation error as `output`.
    /// Used by Feasibility-Gated triage so the implement step never
    /// sees an invalid manifest.
    Fail,
}

// 0.8.6 — `Default` is required by the agent-API broker
// (`src/api/agent_api.rs::agent_api_call`) which builds a synthetic
// `WorkflowStep` for ApiCall execution from a thin agent-supplied
// payload. Every field already has serde / type defaults so the
// derive is a free upgrade; no nullable/Option field changed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowStep {
    pub name: String,
    #[serde(default)]
    pub step_type: StepType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    // 0.8.5 — agent / prompt_template / mode are only meaningful for
    // LLM-driven steps (Agent, BatchQuickPrompt). For ApiCall / Exec /
    // Gate / Notify / JsonData / BatchApiCall steps the wizard sent
    // empty / placeholder values just to satisfy serde, and a sloppy
    // payload missing one of them returned a 422 with no actionable
    // info ("missing field `prompt_template`"). With serde defaults
    // those fields become optional in JSON; runtime validation still
    // enforces them per step_type (see workflow runner dispatch).
    #[serde(default)]
    pub agent: AgentType,
    #[serde(default)]
    pub prompt_template: String,
    #[serde(default)]
    pub mode: StepMode,
    #[serde(default)]
    pub output_format: StepOutputFormat,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_config_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_settings: Option<AgentSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_result: Vec<StepConditionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directive_ids: Vec<String>,

    // ─── BatchQuickPrompt fields ─────────────────────────────────────────
    // All Option<> so existing Agent/ApiCall steps deserialize unchanged.
    // Only meaningful when `step_type == BatchQuickPrompt`.

    /// Id of the Quick Prompt to fan out. Required for BatchQuickPrompt steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_quick_prompt_id: Option<String>,

    /// Template expression that resolves to the list of items. Each item
    /// becomes one child discussion. Examples:
    /// - `"{{steps.fetch_tickets.data.tickets}}"` — structured JSON array
    /// - `"{{steps.fetch_tickets.output}}"` — raw text (parsed as one id per line)
    ///
    /// Required for BatchQuickPrompt steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_items_from: Option<String>,

    /// If true (default), the linear workflow run waits for all child
    /// discussions to finish before moving to the next step. Uses the existing
    /// `BatchRunFinished` WS broadcast as the wake signal — no polling.
    /// If false, the batch is fired and the linear run advances immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_wait_for_completion: Option<bool>,

    /// Safety cap for the number of items spawned by this step. Falls back to
    /// the global 50-item cap enforced by `create_batch_run` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_max_items: Option<u32>,

    /// Workspace mode for each batch child discussion: `"Direct"` (default)
    /// or `"Isolated"` for per-disc git worktrees. Isolated is required when
    /// the agents will write code in parallel — otherwise they clobber each
    /// other in the main working tree. Requires the workflow to have a
    /// project_id, otherwise the step fails early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_workspace_mode: Option<String>,

    /// Chain additional Quick Prompts after the initial one inside each
    /// child discussion. Each QP is auto-sent as a User message once the
    /// previous agent response completes, and the agent is re-fired.
    /// The batch progress counter only increments after the ENTIRE chain
    /// (initial QP + all chained QPs) finishes for a given discussion.
    /// Example: `["qp-review", "qp-summary"]` after the primary `batch_quick_prompt_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub batch_chain_prompt_ids: Vec<String>,

    /// Concurrent fan-out cap for `StepType::BatchApiCall` (HTTP path only).
    /// `None` falls back to a conservative default (5). HTTP can scale much
    /// higher than agent runs (no LLM, just network) but providers rate-limit
    /// — Jira/GitHub typically OK up to 10-20 in parallel, beyond that you
    /// risk 429s. Distinct from BatchQuickPrompt (which goes through the
    /// global agent_semaphore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_concurrent_limit: Option<u32>,

    /// 0.6.0 — when set on a `BatchApiCall` step, the executor loads the
    /// referenced `QuickApi` from the DB at run-time and uses its API
    /// config (plugin, endpoint, method, body, etc.) instead of the
    /// step's own inline `api_*` fields. Mirror of `batch_quick_prompt_id`.
    /// `None` keeps inline-config behaviour. 0.7+ — étendu à `StepType::ApiCall`
    /// (single, non-batch) avec la même sémantique per-field override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_api_id: Option<String>,

    /// 0.7+ — référence vers un `QuickPrompt` saved. Quand set sur un step
    /// `Agent`, le runner charge le QP au run-time et utilise son
    /// `prompt_template`, son `agent`, son `tier`, et ses `skill_ids` ; les
    /// fields renseignés sur le step écrasent ceux du QP (per-field override).
    /// Permet de définir un prompt canonique côté Quick Prompts et de le
    /// réutiliser dans N workflows. Pas de variables au niveau step :
    /// les `{{var}}` du QP sont résolus avec le `TemplateContext` du
    /// workflow (launch variables / state / steps.X / etc.). Mirror du
    /// pattern `quick_api_id` pour les ApiCall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_prompt_id: Option<String>,

    // ─── Notify fields ───────────────────────────────────────────────────
    // Only meaningful when `step_type == Notify`. Webhook-based workflow
    // finalizer: posts to an external URL with a rendered body. Zero agent
    // tokens consumed — direct HTTP from Rust.

    /// Webhook configuration for `StepType::Notify`. URL and body support
    /// the same `{{steps.X.output}}` / `{{steps.X.data}}` templates as
    /// agent prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_config: Option<NotifyConfig>,

    // ─── ApiCall fields (désagentification — 0.5.2) ──────────────────────
    // Only meaningful when `step_type == ApiCall`. Calls a Kronn-configured
    // API plugin directly from the Rust engine — zero agent tokens. Params
    // support the same `{{steps.X.data}}` templates as agent prompts.
    // See `docs/operations/deagent-apicall.md` for the full contract.

    /// Registry slug of the plugin to invoke (e.g. `"chartbeat"`, `"jira"`).
    /// The slug resolves to an `ApiSpec` in the plugin registry; the request
    /// base URL comes from that spec and is NEVER templated from the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_plugin_slug: Option<String>,

    /// `McpConfig.id` of the specific credential set to use. The plugin can
    /// be configured multiple times per project (e.g. two Jira instances);
    /// this picks one. Decrypted env lives in the DB row and is loaded at
    /// step execution via `collect_active_api_plugins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_config_id: Option<String>,

    /// Endpoint path as declared in `ApiSpec.endpoints[].path` — prefix-
    /// matched against the allowlist in the executor so a step can't reach
    /// arbitrary paths under the plugin's `base_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_endpoint_path: Option<String>,

    /// HTTP method override. Defaults to the method of the endpoint in the
    /// plugin registry. Uppercase: `GET | POST | PUT | PATCH | DELETE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_method: Option<String>,

    /// Path-segment parameters (e.g. `/repos/{owner}/{repo}` → `{owner}` and
    /// `{repo}`). The executor scans `api_endpoint_path` for `{key}` tokens
    /// and substitutes each match with the value from this map at request
    /// time. Values support `{{steps.X.data}}` templates so a previous
    /// fetch can drive the segment dynamically. Tokens with no entry stay
    /// literal — the request will fail because `/repos/{owner}/...` is not
    /// a real GitHub path. This way the spec-declared template stays in
    /// `api_endpoint_path` (round-trip safe across re-edits) while the
    /// concrete values live separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path_params: Option<std::collections::HashMap<String, String>>,

    /// Query-string parameters. Values support `{{steps.X.data}}` templates.
    /// Rendered values are percent-encoded AFTER template expansion to
    /// prevent injection (`&` / `=` in a templated value would corrupt the
    /// query otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_query: Option<std::collections::HashMap<String, String>>,

    /// Extra headers (auth headers come from the plugin spec, not here).
    /// String values templatable; keys are literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_headers: Option<std::collections::HashMap<String, String>>,

    /// JSON body for POST/PUT/PATCH. Rendered by walking the `Value` tree
    /// and interpolating string leaves only — no string-level interpolation,
    /// which would allow JSON injection via templated content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_body: Option<serde_json::Value>,

    /// How to extract a value from the JSON response. The extracted `data`
    /// is what downstream steps read via `{{steps.X.data}}`; batch QP steps
    /// expect an array and fail-fast if it's a scalar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_extract: Option<ExtractSpec>,

    /// Pagination strategy. `Auto` (default-ish) inspects the response for
    /// `nextPageToken` / `startAt`+`total` / `page` and walks accordingly,
    /// concatenating arrays. Hard-capped at 50 pages to prevent runaway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_pagination: Option<PaginationSpec>,

    /// Per-request timeout in milliseconds. Defaults to 30 000 ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_timeout_ms: Option<u64>,

    /// Max retries on 5xx / 429 with exponential backoff. Defaults to 2.
    /// Idempotent GETs retry freely; endpoints flagged `side_effect: true`
    /// in the plugin spec skip retry entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_max_retries: Option<u8>,

    /// Context variable name under which the extracted data is stored.
    /// Downstream steps reference it as `{{steps.<output_var>.data}}`.
    /// Defaults to the step's `name` field when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_output_var: Option<String>,

    // ─── Gate fields (0.7.0 Phase 4 — human-in-the-loop) ─────────────
    // Only meaningful when `step_type == Gate`. The runner stops the
    // run with `RunStatus::WaitingApproval`; a human decides via the
    // dashboard. Templates resolve at gate-execution time so the
    // operator sees the actual values, not the literal `{{X}}`.

    /// Markdown message shown to the operator on the run-detail page.
    /// Templates supported. Empty string falls back to a default
    /// "Décision humaine requise" placeholder in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_message: Option<String>,

    /// Step name to jump to when the operator picks "Request Changes".
    /// `None` → falls back to the previous step (one step back), which
    /// matches the Auto-Dev `pause_pre_merge` → `goto: implement` pattern.
    /// Set explicitly to a step name for non-default targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_request_changes_target: Option<String>,

    /// 0.7.0 P1-1 — optional webhook URL to POST when the run enters
    /// `WaitingApproval` on this gate. Best-effort fire-and-forget;
    /// failures are logged but never block the run. Templated, so
    /// users can drop `{{state.slack_url}}` etc. Body :
    /// `{run_id, workflow_id, workflow_name, step_name, message}`.
    /// The "ping ops when a Gate fires" use case Cyndie + Antony
    /// flagged as blocker for team-wide deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_notify_url: Option<String>,

    // ─── Exec fields (0.7.0 Phase 5 — direct shell, no LLM) ──────────────
    // Only meaningful when `step_type == Exec`. Defence in depth:
    //   1. `command` is the binary name verbatim — match-tested against
    //      `Workflow.exec_allowlist` at save time (rejected if absent).
    //   2. NEVER passed through a shell (`sh -c`, `bash -c`) — spawned
    //      directly via `crate::core::cmd::async_cmd` with args as
    //      separate argv elements. So pipes, redirections, glob
    //      expansion DO NOT apply.
    //   3. Args ARE templated (`{{steps.X.summary}}` etc.) but the
    //      rendered value is passed as a single literal argument — even
    //      if it contains `; rm -rf /`, the OS receives one argv string,
    //      not a shell command.
    //   4. `command` itself is NOT templated — locked at save time.

    /// Binary to execute. Must match an entry in `Workflow.exec_allowlist`
    /// exactly (no glob, no regex). NOT templated — locked at save time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_command: Option<String>,

    /// Arguments passed verbatim to the binary. Each entry is one argv
    /// element. Templates `{{steps.X}}` are rendered, but the result
    /// becomes a literal argument — no shell metachar interpretation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_args: Vec<String>,

    /// Per-step timeout in seconds. Defaults to 300s (5 min) if unset.
    /// Hard-capped at 1800s (30 min) at validate time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_timeout_secs: Option<u32>,

    /// 0.8.2 — Optional setup command that runs IMMEDIATELY BEFORE the
    /// main `exec_command`. Designed for the worktree-dependency-install
    /// pattern: `composer install` / `pnpm install` / etc., so the main
    /// command (e.g. `make test`) has the artifacts it needs even though
    /// the worktree starts with only git-tracked files (no `vendor/`,
    /// no `node_modules/`, no `target/`).
    ///
    /// Same allowlist + char-validation + timeout rules as `exec_command`.
    /// Templates `{{steps.X}}` resolve normally. If the setup fails
    /// (non-zero exit), the step fails IMMEDIATELY without running the
    /// main command — the user sees the setup's stderr.
    ///
    /// When `None`, the executor skips straight to `exec_command` (no
    /// extra subprocess overhead). Backward-compatible default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_setup_command: Option<String>,
    /// Argv for `exec_setup_command`. Same literal-argv semantics as
    /// `exec_args` (no shell, no metachar interpretation). Use
    /// `[\"-c\", \"<oneliner>\"]` if you need to wrap a shell line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_setup_args: Vec<String>,

    // ─── JsonData fields (0.7+ — déterministe data source) ───────────────
    // Only meaningful when `step_type == JsonData`. Zéro token, zéro
    // réseau. Le runner sérialise `json_data_payload` dans une envelope
    // Structured et la passe au step suivant. Cas d'usage : alimenter un
    // BatchQuickPrompt sans API derrière. Voir json_data_step.rs.

    /// Payload JSON émis par le step. Validé au save (parse JSON valide,
    /// taille raisonnable). Aucun templating au runtime — la valeur est
    /// retournée telle quelle, ce qui permet à un downstream batch de la
    /// consommer via `{{steps.<name>.data}}` exactement comme une réponse
    /// API. Si tu as besoin de `{{var}}` dans le payload, mets ça dans
    /// un Agent step ou un ApiCall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_data_payload: Option<serde_json::Value>,
}

/// Extraction specification for an `ApiCall` step's JSON response.
/// Implements RFC 9535 JSONPath via the `serde_json_path` crate.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ExtractSpec {
    /// JSONPath expression, e.g. `$.issues[*].key` or `$.data.viewer.zones[0].zoneTag`.
    /// Evaluated against the full response (after pagination concat if enabled).
    pub path: String,

    /// Default value when the path resolves to nothing. Keeps workflows
    /// alive on empty results. Omit = `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<serde_json::Value>,

    /// If true and the extraction returns `null` / empty array, the step
    /// reports `status: NO_RESULTS` so `on_result` conditions (Skip, Stop,
    /// Goto) can fire. Default false → status OK even on empty data.
    #[serde(default)]
    pub fail_on_empty: bool,
}

/// Pagination strategy for an `ApiCall` step. `Auto` covers the three most
/// common REST patterns; explicit variants let advanced users hardcode the
/// cursor/offset paths for non-standard APIs (Cloudflare GraphQL for ex.).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(tag = "type")]
pub enum PaginationSpec {
    /// No pagination — issue the request once and return the response.
    None,

    /// Auto-detect: inspects the response for `nextPageToken` (cursor),
    /// `startAt`+`total`+`maxResults` (offset), or `page`+`has_more` (page).
    /// Walks until exhausted or `max_pages` reached.
    Auto {
        /// Safety cap. Defaults to 50 when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Classic offset pagination: `GET …?start_param=0&limit_param=100`,
    /// increments `start` by `limit` until `len(items) < limit` or
    /// `total_path` reports all consumed.
    Offset {
        start_param: String,
        limit_param: String,
        limit: u32,
        /// JSONPath to the total count in the response, e.g. `$.total`.
        total_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Cursor-based: response exposes a next-cursor path that feeds back
    /// into `cursor_param` on the next call. Terminates when the path
    /// resolves to null/absent.
    Cursor {
        cursor_param: String,
        /// JSONPath to the next cursor value, e.g. `$.pageInfo.endCursor`.
        next_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Page number: increment `page_param` from 1, stop when `has_more_path`
    /// is false or there are no more results.
    Page {
        page_param: String,
        page_size_param: String,
        page_size: u32,
        /// JSONPath to a boolean / truthy "has more" indicator.
        has_more_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },
}

/// Configuration for a `StepType::Notify` webhook step. Rendered at run-time
/// (URL + body support template expressions like `{{previous_step.summary}}`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NotifyConfig {
    /// Target URL. Supports template variables.
    pub url: String,
    /// HTTP method — "POST" (default), "PUT", "GET". Only these three are
    /// accepted; anything else fails at execution time.
    #[serde(default = "default_notify_method")]
    pub method: String,
    /// Custom headers. Case-insensitive on the wire — we send them as given.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: std::collections::HashMap<String, String>,
    /// Request body. Templated. Sent as-is — set `Content-Type: application/json`
    /// in `headers` if the body is JSON. Ignored for GET.
    #[serde(default)]
    pub body_template: String,
}

fn default_notify_method() -> String { "POST".to_string() }

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepMode {
    // 0.8.5 — `Normal` is the only variant today and is `Default` so
    // `WorkflowStep` can mark `mode` as `#[serde(default)]`. Required at
    // the type level for forward-compat (we plan a `Slash` variant for
    // /slash-command steps in 0.9.0) but optional in JSON for clients
    // that don't care (ApiCall, Exec, …).
    #[default]
    Normal,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepType {
    #[default]
    Agent,
    ApiCall,
    /// Fan out a Quick Prompt over a list of items (rendered from a previous
    /// step's output) — spawns N child discussions via the shared `create_batch_run`
    /// helper and optionally waits for all of them to finish before moving on.
    /// Phase 2 batch workflows (2026-04-10).
    BatchQuickPrompt,
    /// Direct webhook/HTTP call — zero agent tokens. Used as a finalizer
    /// (send completion notification, trigger downstream pipeline) or as
    /// a mechanical data step (create GitHub issue, post to Slack) without
    /// spawning an LLM. Shipped 0.3.5.
    Notify,
    /// 0.7.0 Phase 4 — human-in-the-loop pause. The run halts at this
    /// step with `RunStatus::WaitingApproval`; a human operator decides
    /// from the UI (`POST /api/workflows/runs/:id/decide`) whether the
    /// workflow continues, jumps to another step (request_changes), or
    /// stops. Zero tokens — no LLM is spawned for the gate itself.
    Gate,
    /// 0.7.0 Phase 5 — direct shell execution. Zero tokens, runs a
    /// pre-allowlisted binary in the workflow's workspace. Defence in
    /// depth: binary must be in `Workflow.exec_allowlist`, NEVER goes
    /// through `sh -c`, args are templated but passed as literal argv
    /// (no shell interpretation), workdir locked to the workspace,
    /// timeout-bounded. Typical use: `cargo test`, `npm run build`,
    /// `make deploy`.
    Exec,
    /// 0.6.0 — fan out an API call over a list of items, in parallel,
    /// **with zero LLM tokens**. The mechanical counterpart of
    /// `BatchQuickPrompt`: same fan-out semantics, but each child fires
    /// a templated HTTP request via the configured plugin instead of
    /// spawning an agent. Used by Feature Planner to bulk-create Jira
    /// tickets (one POST /issue per planned sub-task) without paying
    /// the per-ticket agent loop. Concurrency capped by
    /// `batch_concurrent_limit` (default 5) to avoid hammering the
    /// upstream API.
    BatchApiCall,
    /// 0.7+ — déterministe data source : émet un payload JSON littéral
    /// stocké dans le step (`json_data_payload`). Zéro token, zéro réseau.
    /// Cas d'usage : workflow batch sur une liste figée (ex: 10 hosts
    /// hardcodés alimentent un BatchQuickPrompt sans avoir à monter une
    /// API). Aussi utile comme fixture de dev — on construit le pipeline
    /// sur du JsonData puis on remplace par un `ApiCall` quand la vraie
    /// source est prête. Output toujours `Structured` : envelope
    /// `{data: payload, status: "OK", summary: "JSON data (N items)"}`.
    JsonData,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSettings {
    /// Explicit model override (expert mode). Takes priority over tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Abstract tier selection. Resolved to a concrete --model flag per agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StepConditionRule {
    pub contains: String,
    pub action: ConditionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum ConditionAction {
    Stop,
    Skip,
    /// Jump back (or forward) to `step_name`. Optional `max_iterations`
    /// scopes the loop: after the same Goto fires that many times,
    /// the runner falls through instead of jumping (run continues
    /// past the loop). Without `max_iterations` the workflow-level
    /// `loop_detection_max_revisits` guard remains the only safety
    /// net — fine for short loops, but per-loop scoping is cleaner
    /// when you have several independent loops in the same workflow.
    /// 0.7.0 Phase 6.
    Goto {
        step_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_iterations: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RetryConfig {
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub backoff: String,
}

fn default_backoff() -> String {
    "exponential".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum WorkflowAction {
    CreatePr {
        title_template: String,
        body_template: String,
        branch_template: String,
    },
    CommentIssue {
        body_template: String,
    },
    UpdateTrackerStatus {
        status: String,
    },
    CreateIssue {
        title_template: String,
        body_template: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSafety {
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
    #[serde(default)]
    pub require_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub hooks: WorkspaceHooks,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_remove: Option<String>,
}

// ─── Workflow Runs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub status: RunStatus,
    #[ts(type = "any")]
    pub trigger_context: Option<serde_json::Value>,
    pub step_results: Vec<StepResult>,
    pub tokens_used: u64,
    pub workspace_path: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Linear workflow run vs batch fan-out. Default "linear" for backward
    /// compatibility with existing runs created before Phase 1b.
    #[serde(default = "default_run_type")]
    pub run_type: String,
    /// For batch runs: target number of child discussions. 0 for linear runs.
    #[serde(default)]
    pub batch_total: u32,
    /// For batch runs: number of successfully-completed child discussions.
    #[serde(default)]
    pub batch_completed: u32,
    /// For batch runs: number of child discussions that ended with an error.
    #[serde(default)]
    pub batch_failed: u32,
    /// For batch runs: display name shown in the sidebar group header.
    /// Example: "Cadrage to-Frame — 10 avr 14:00".
    #[serde(default)]
    pub batch_name: Option<String>,
    /// Link a child batch run back to the linear workflow run that spawned it
    /// via a `BatchQuickPrompt` step. `None` for top-level runs (both linear
    /// runs and manual batch runs triggered from the UI).
    #[serde(default)]
    pub parent_run_id: Option<String>,
    /// 0.7.0 Phase 6 — durable state map carried across iterations and
    /// resume cycles (Gate, restart). Agents write entries by emitting
    /// `---STATE:<key>=<value>---` blocks in their output (parsed
    /// alongside artifacts); steps read them via `{{state.<key>}}`.
    /// Used for retry counters, accumulated verdicts, and any other
    /// cross-iteration memory that doesn't belong in step outputs.
    /// Empty by default. Persisted as a JSON object on the run row.
    #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string>")]
    pub state: ::std::collections::HashMap<String, String>,
    /// 0.7.0 — branches preserved by the runner during worktree cleanup
    /// because their HEAD held commits not on any known base ref. The UI
    /// surfaces them on the run detail page so the operator can recover
    /// the work even when the agent's push step failed (pre-push hook
    /// blocked, no auth, network down, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produced_branches: Vec<ProducedBranch>,
}

/// One preserved branch on a workflow run. Mirrors `workspace::PreservedBranch`
/// but lives on the model side so it can serialize to JSON for storage and
/// type-export for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProducedBranch {
    pub branch_name: String,
    pub head_sha: String,
    pub ahead: u32,
    /// True when the branch had an upstream tracking ref at cleanup time —
    /// i.e. the agent at least *tried* to push (and may have partially
    /// succeeded; check `ahead` for unpushed commits).
    pub pushed_upstream: bool,
}

fn default_run_type() -> String { "linear".to_string() }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    WaitingApproval,
    /// 0.7.0 — terminal state distinct from `Failed`: the run hit a
    /// `WorkflowGuards` limit (timeout, max LLM calls, loop detection).
    /// UX surfaces this with a shield icon (orange, not red) so users
    /// can tell a self-protected stop from a real failure.
    StoppedByGuard,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StepResult {
    pub step_name: String,
    pub status: RunStatus,
    pub output: String,
    pub tokens_used: u64,
    pub duration_ms: u64,
    /// 0.8.2 — Wall-clock timestamp at which the step started executing.
    /// Optional for backward compatibility with runs written before this
    /// field existed (front-end falls back to the legacy `runStart + sum
    /// of prior durations` estimate when missing). The primary driver for
    /// adding this was the Gate step's `duration_ms`: it used to record
    /// only the executor render time (~0ms), so the time spent paused on
    /// WaitingApproval was invisible to the live-elapsed counter for the
    /// NEXT step (which then showed `now - runStart - 0ms` ≈ the full
    /// pause duration). With `started_at`, the resume handler can compute
    /// `duration_ms = now - started_at` on approval and surface the real
    /// pause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// What happened after this step: null = continued normally, or the condition action triggered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_result: Option<String>,
    /// For `output_format: Structured` steps only — did the agent actually
    /// produce the `---STEP_OUTPUT---` envelope (possibly after repair)?
    /// `Some(true)`  = envelope found, `.data/.summary/.status` populated.
    /// `Some(false)` = Structured requested but extraction failed even after
    ///                 repair, downstream `{{steps.X.data}}` won't resolve.
    /// `None`        = FreeText step, the concept does not apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_detected: Option<bool>,
    /// Snapshot of the step's `step_type.type` at execution time
    /// (`"Agent" | "ApiCall" | "Notify" | "BatchQuickPrompt" | "Custom"`).
    /// Frozen on the run row so editing the workflow afterwards (changing
    /// the step type, swapping the agent, retargeting the API plugin)
    /// doesn't corrupt the historical record. `None` is tolerated for
    /// rows written before this field existed — the frontend falls back
    /// to "(legacy)" rather than crashing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_kind: Option<String>,
    /// Snapshot of `step.agent` for Agent / Custom steps. `None` for
    /// non-agent steps (ApiCall, Notify, Batch). Lets the run-detail UI
    /// say "Codex was used here" even after the workflow was edited to
    /// run with a different agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_agent: Option<AgentType>,
    /// Snapshot of the API plugin slug for ApiCall steps (`mcp-github`,
    /// `api-chartbeat`, …). `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_api_plugin_slug: Option<String>,
    /// Snapshot of the resolved endpoint path for ApiCall steps. Stored
    /// AFTER path-param substitution so reviewers see the actual URL
    /// path that was hit (`/repos/anthropics/anthropic-cookbook/issues`
    /// rather than the template `/repos/{owner}/{repo}/issues`). `None`
    /// for non-API steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_api_endpoint_path: Option<String>,
}


// ─── Workflow API requests ────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub project_id: Option<String>,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub actions: Vec<WorkflowAction>,
    #[serde(default)]
    pub safety: Option<WorkflowSafety>,
    #[serde(default)]
    pub workspace_config: Option<WorkspaceConfig>,
    #[serde(default)]
    pub concurrency_limit: Option<u32>,
    #[serde(default)]
    pub guards: Option<WorkflowGuards>,
    #[serde(default)]
    #[ts(type = "Record<string, ArtifactSpec>")]
    pub artifacts: ::std::collections::HashMap<String, ArtifactSpec>,
    #[serde(default)]
    pub on_failure: Vec<WorkflowStep>,
    #[serde(default)]
    pub exec_allowlist: Vec<String>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    /// 0.8.5 — optional initial state. Default `true` for back-compat
    /// (every UI-driven create stays enabled by default). The MCP
    /// `workflow_create_draft` tool sets this to `false` so an
    /// agent-spawned workflow lands in the user's Workflows page in a
    /// disabled state, ready for review + manual enable. Avoids the
    /// "agent just created a cron workflow that fires unattended"
    /// failure mode while still letting agents accelerate the
    /// adoption of Kronn by drafting common patterns autonomously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    pub project_id: Option<Option<String>>,
    pub trigger: Option<WorkflowTrigger>,
    pub steps: Option<Vec<WorkflowStep>>,
    pub actions: Option<Vec<WorkflowAction>>,
    pub safety: Option<WorkflowSafety>,
    pub workspace_config: Option<WorkspaceConfig>,
    pub concurrency_limit: Option<u32>,
    pub guards: Option<WorkflowGuards>,
    /// Replace the artifact map entirely when present. To clear all
    /// declarations, send `Some({})`. Omit the field to leave existing
    /// declarations untouched.
    #[serde(default)]
    #[ts(type = "Record<string, ArtifactSpec> | null")]
    pub artifacts: Option<::std::collections::HashMap<String, ArtifactSpec>>,
    /// Replace the rollback chain entirely when present. To clear it,
    /// send `Some([])`. Omit to leave the existing chain untouched.
    #[serde(default)]
    pub on_failure: Option<Vec<WorkflowStep>>,
    /// Replace the Exec allowlist entirely when present. Send
    /// `Some([])` to disable Exec steps; omit to leave it untouched.
    #[serde(default)]
    pub exec_allowlist: Option<Vec<String>>,
    /// Replace launch-time variables entirely when present. `Some([])`
    /// to clear, omit to keep existing.
    #[serde(default)]
    pub variables: Option<Vec<PromptVariable>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSummary {
    pub id: String,
    pub name: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub trigger_type: String,
    pub step_count: u32,
    pub enabled: bool,
    pub last_run: Option<WorkflowRunSummary>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowRunSummary {
    pub id: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub tokens_used: u64,
}

/// Compact summary of a batch workflow run, with its parent linear run
/// resolved to a human-friendly (workflow name + run sequence number) label.
/// Consumed by the discussion sidebar to render a clickable pastille on each
/// batch group ("↗ run #3 de Recap hebdo") so users can trace a batch back
/// to the workflow that spawned it.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchRunSummary {
    pub run_id: String,
    pub batch_name: Option<String>,
    pub batch_total: u32,
    pub status: RunStatus,
    /// Id + name of the Quick Prompt that this batch fans out. Resolved from
    /// the batch run's virtual `qp:<id>` workflow_id prefix. Used by the
    /// sidebar as the batch folder label (instead of the first child disc's
    /// title, which is just one ticket id among N and misleads users).
    pub quick_prompt_id: Option<String>,
    pub quick_prompt_name: Option<String>,
    pub quick_prompt_icon: Option<String>,
    /// Parent linear workflow run id (None for top-level manual batches).
    pub parent_run_id: Option<String>,
    /// Name of the workflow that spawned this batch, resolved at query time.
    /// None when the parent run was deleted or this is a manual batch.
    pub parent_workflow_id: Option<String>,
    pub parent_workflow_name: Option<String>,
    /// 1-based position of the parent linear run among all runs of that
    /// workflow (ordered by started_at). None for manual batches.
    pub parent_run_sequence: Option<u32>,
}

// ─── Workflow suggestions ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSuggestion {
    pub id: String,
    pub title: String,
    pub description: String,
    pub reason: String,
    pub required_mcps: Vec<String>,
    pub audience: String,
    pub complexity: String,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
}

/// 0.7.0 UX pass — payload for `POST /api/workflows/import`.
/// `content` is the raw JSON of a `WorkflowExportEnvelope` (string
/// rather than nested object so the frontend can `JSON.parse` once
/// from the dropped file and pass it through). `project_id` is the
/// importer's project to attach the workflow to — `None` keeps it
/// unattached (the user picks a project later via the wizard).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportWorkflowRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// 0.6.0 UX pass — optional payload for `POST /api/workflows/:id/trigger`.
/// `variables` carries user-entered values matching `Workflow.variables`
/// (manual launch). The keys become `{{var_name}}` in step prompts via
/// the run's `trigger_context`. Empty/missing body keeps the legacy
/// "trigger with no variables" flow working — back-compat for tracker
/// triggers that don't need variables.
#[derive(Debug, Deserialize, Default, TS)]
#[ts(export)]
pub struct TriggerWorkflowRequest {
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
}

/// Self-contained envelope produced by `GET /api/workflows/:id/export`.
/// Designed to be saved to disk, mailed, attached to a Github issue, etc.
/// `version: 1` is the current shape; future incompatible changes bump
/// the version and add a migration path at import time.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowExportEnvelope {
    /// Discriminator: always `"kronn.workflow"` for this envelope.
    pub kind: String,
    /// Schema version. Bumped on incompatible changes.
    pub version: u32,
    /// ISO-8601 timestamp of the export, for audit and human readability.
    pub exported_at: DateTime<Utc>,
    /// The workflow definition. `id`, `project_id`, `created_at`,
    /// `updated_at`, `enabled` are kept in the wire format (so a
    /// roundtrip is lossless to inspect) but DROPPED at import — the
    /// importer mints fresh values for those fields.
    pub workflow: Workflow,
    /// QPs referenced by `BatchQuickPrompt` steps. Bundled so the
    /// importer doesn't need to fetch them separately. Empty when no
    /// step references a QP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_quick_prompts: Vec<super::QuickPrompt>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct TestStepRequest {
    pub step: WorkflowStep,
    pub project_id: Option<String>,
    /// Mock previous step output (raw text or structured JSON)
    #[serde(default)]
    pub mock_previous_output: Option<String>,
    /// Additional mock variables: {"issue.title": "...", "steps.collect.data": "..."}
    #[serde(default)]
    pub mock_variables: Option<std::collections::HashMap<String, String>>,
    /// Dry run: agent describes what it would do without executing any write actions
    #[serde(default)]
    pub dry_run: bool,
}

#[cfg(test)]
mod step_deserialization_tests {
    use super::*;

    /// 0.8.5 dogfooding regression test — JIRA helper case.
    ///
    /// The wizard's ApiCall step card composed a payload with `step_type:
    /// "ApiCall"` + `api_*` fields but NO `agent` / `prompt_template` /
    /// `mode`. Pre-fix serde rejected with `missing field
    /// "prompt_template"` and axum returned 422 with the body as
    /// text/plain — the frontend swallowed the body and the user saw a
    /// bare "Server error (HTTP 422)" with no clue what was wrong. Now
    /// the LLM-only fields default and an ApiCall step can be a minimal
    /// JSON. The frontend api.ts also surfaces non-JSON error bodies,
    /// so future serde rejects are at least diagnosable.
    #[test]
    fn workflow_step_apicall_deserialises_without_llm_fields() {
        let json = r#"{
            "name": "fetch_issue",
            "step_type": { "type": "ApiCall" },
            "api_plugin_slug": "mcp-atlassian",
            "api_config_id": "cfg-1",
            "api_endpoint_path": "/rest/api/3/issue/EW-7247",
            "api_method": "GET"
        }"#;
        let step: WorkflowStep = serde_json::from_str(json)
            .expect("ApiCall step with no agent/prompt/mode must deserialise");
        assert_eq!(step.name, "fetch_issue");
        assert!(matches!(step.step_type, StepType::ApiCall));
        // Defaults kicked in for the LLM-only fields.
        assert_eq!(step.prompt_template, "");
        assert!(matches!(step.agent, crate::models::AgentType::ClaudeCode));
        assert!(matches!(step.mode, StepMode::Normal));
    }

    /// Agent steps still require an explicit prompt at runtime, but the
    /// JSON shape stays permissive at deserialisation — runtime
    /// validation lives in the workflow runner dispatch, not in serde.
    /// This test pins that a fully-populated Agent step round-trips.
    #[test]
    fn workflow_step_agent_roundtrips_with_explicit_fields() {
        let json = r#"{
            "name": "summarise",
            "step_type": { "type": "Agent" },
            "agent": "Codex",
            "prompt_template": "Résume {{steps.fetch.data}}",
            "mode": { "type": "Normal" }
        }"#;
        let step: WorkflowStep = serde_json::from_str(json).expect("Agent step must deserialise");
        assert_eq!(step.prompt_template, "Résume {{steps.fetch.data}}");
        assert!(matches!(step.agent, crate::models::AgentType::Codex));
    }

    /// 422 friendliness — the test API call endpoint payload that
    /// frontend `workflowsApi.testApiCall()` sends. Without
    /// `prompt_template` / `agent` / `mode` defaults this test would
    /// blow up with the exact error the JIRA helper agent surfaced
    /// during dogfooding ("missing field `prompt_template`").
    #[test]
    fn test_api_call_request_accepts_minimal_step() {
        use crate::api::workflows::TestApiCallRequest;
        let json = r#"{
            "step": {
                "name": "fetch_issue",
                "step_type": { "type": "ApiCall" },
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-1",
                "api_endpoint_path": "/rest/api/3/issue/EW-{{ticket_number}}",
                "api_method": "GET",
                "api_query": { "fields": "summary,description" },
                "api_extract": { "path": "$.fields", "fail_on_empty": false }
            },
            "project_id": "proj-1"
        }"#;
        let req: TestApiCallRequest = serde_json::from_str(json)
            .expect("TestApiCallRequest with minimal ApiCall step must deserialise");
        assert_eq!(req.project_id, "proj-1");
        assert_eq!(req.step.name, "fetch_issue");
    }
}
