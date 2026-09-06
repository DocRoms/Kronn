// Quick Prompts (reusable prompt templates) + Quick APIs (reusable API
// call templates). Both ride on the same `{{variable}}` rendering engine
// the workflow steps use, so they live alongside the workflow types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use ts_rs::TS;

use super::{AgentSettings, AgentType, ExtractSpec, ModelTier, PaginationSpec};

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts (reusable prompt templates with variables)
// ═══════════════════════════════════════════════════════════════════════════════

/// Where a declared template variable obtains its value at execution time.
///
/// The declaration is deliberately a reference only. In particular a
/// `ProjectEnv` declaration stores `<env.NAME>`, never the secret value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PromptVariableSource {
    #[default]
    UserInput,
    KronnContext,
    ProjectEnv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptVariableOption {
    /// Stable execution value. Renaming the label never changes run payloads.
    pub value: String,
    pub label: String,
    /// Disabled options remain in version history but cannot be selected by a
    /// new run.
    #[serde(default = "default_variable_option_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptVariableControl {
    Text,
    Textarea,
    Select {
        options: Vec<PromptVariableOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        default_value: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptVariable {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    /// Optional human description of what this variable means. Shown in
    /// the batch-workflow UI so the user mapping tracker fields to QP
    /// variables knows what each one is for.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the variable must be filled before the QP can run.
    /// Defaults to `true` for backward compatibility — existing QP
    /// variables are treated as required.
    #[serde(default = "default_variable_required")]
    pub required: bool,
    /// 2026-06-10 — optional regex the provided value must match (anchored
    /// full-match). Lets a workflow declare a shape (`^[A-Z]+-\d+$` for a
    /// Jira key) so a typo like `7152` instead of `EW-7152` is rejected at
    /// launch with a clear message, BEFORE it reaches the API as a literal
    /// path param and 404s. `None` = no shape constraint (legacy). Invalid
    /// regex is treated as "no constraint" (never blocks a launch on a
    /// malformed pattern; logged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Resolution strategy. Omitted legacy definitions remain manual inputs.
    #[serde(default)]
    pub source: Option<PromptVariableSource>,
    /// Declarative source reference (`<env.NAME>` for `ProjectEnv`).
    /// This field must never carry a resolved value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Project environment variables are read-only unless the template author
    /// explicitly allows an audited launch-time override.
    #[serde(default)]
    pub allow_manual_override: bool,
    /// Presentation and bounded-value contract. Missing legacy values are
    /// regular single-line text inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub control: Option<PromptVariableControl>,
}

impl PromptVariable {
    /// Only a `UserInput` variable is ever collected from the launcher.
    /// `allow_manual_override` makes an override *possible* for a
    /// project-resolved variable, never mandatory: Kronn still resolves the
    /// value when the launcher leaves the optional override blank, so an
    /// override-enabled `ProjectEnv`/`KronnContext` declaration must not be
    /// treated as a required input.
    pub fn requires_user_input(&self) -> bool {
        self.source.clone().unwrap_or_default() == PromptVariableSource::UserInput
    }

    pub fn validate_source(&self) -> Result<(), String> {
        match self.source.clone().unwrap_or_default() {
            PromptVariableSource::UserInput => Ok(()),
            PromptVariableSource::KronnContext => self
                .source_ref
                .as_deref()
                .filter(|reference| is_reference(reference, "context"))
                .map(|_| ())
                .ok_or_else(|| {
                    format!(
                        "Context variable `{}` must reference <context.NAME>",
                        self.name
                    )
                }),
            PromptVariableSource::ProjectEnv => {
                let reference = self.source_ref.as_deref().unwrap_or_default();
                if is_reference(reference, "env") {
                    Ok(())
                } else {
                    Err(format!(
                        "Project environment variable `{}` must reference <env.NAME>",
                        self.name
                    ))
                }
            }
        }
    }

    pub fn default_input_value(&self) -> Option<&str> {
        match self.control.as_ref() {
            Some(PromptVariableControl::Select { default_value, .. }) => default_value.as_deref(),
            _ => None,
        }
    }

    pub fn accepts_value(&self, value: &str) -> bool {
        match self.control.as_ref() {
            Some(PromptVariableControl::Select { options, .. }) => options
                .iter()
                .any(|option| option.enabled && option.value == value),
            _ => true,
        }
    }

    fn validate_control(&self) -> Result<(), String> {
        let Some(PromptVariableControl::Select {
            options,
            default_value,
        }) = self.control.as_ref()
        else {
            return Ok(());
        };
        if options.is_empty() {
            return Err(format!(
                "Select variable `{}` must declare at least one option",
                self.name
            ));
        }
        let mut values = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for option in options {
            if option.value.trim().is_empty()
                || option.label.trim().is_empty()
                || !values.insert(option.value.as_str())
                || !labels.insert(option.label.as_str())
            {
                return Err(format!(
                    "Select variable `{}` requires unique non-empty option values and labels",
                    self.name
                ));
            }
        }
        if !options.iter().any(|option| option.enabled) {
            return Err(format!(
                "Select variable `{}` must keep at least one active option",
                self.name
            ));
        }
        if default_value.as_ref().is_some_and(|default| {
            !options
                .iter()
                .any(|option| option.enabled && option.value == *default)
        }) {
            return Err(format!(
                "Select variable `{}` default must reference an active option",
                self.name
            ));
        }
        Ok(())
    }
}

/// A source declaration has no room for a literal.  Keep the accepted syntax
/// deliberately narrow so persisted definitions can be inspected without
/// guessing whether a string is a reference or a secret value.
fn is_reference(reference: &str, namespace: &str) -> bool {
    let Some(name) = reference
        .strip_prefix(&format!("<{namespace}."))
        .and_then(|value| value.strip_suffix('>'))
    else {
        return false;
    };
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reject invalid declarations at every authoring boundary.  Runtime
/// resolution can then rely on the declaration shape without ever accepting
/// a persisted literal as an environment reference.
pub fn validate_prompt_variables(variables: &[PromptVariable]) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for variable in variables {
        if variable.name.trim().is_empty() || !names.insert(variable.name.trim()) {
            return Err("Variable names must be non-empty and unique".into());
        }
        variable.validate_source()?;
        variable.validate_control()?;
    }
    Ok(())
}

const TEMPLATE_ENV_VARIABLE_PREFIX: &str = "__kronn_template_env__";

/// Add internal declarations for environment references written directly in a
/// Quick Prompt template. `{{env.NAME}}` is the author-facing syntax; the
/// older `<env.NAME>` form stays executable for existing templates.
///
/// The generated declarations are runtime-only: they are never persisted in
/// the Quick Prompt and therefore cannot turn an environment value into an
/// editor-visible field.
pub fn declarations_with_template_environment_variables(
    template: &str,
    declarations: &[PromptVariable],
) -> Vec<PromptVariable> {
    let mut result = declarations.to_vec();
    for (variable_name, environment_name) in template_environment_bindings(template, declarations) {
        result.push(PromptVariable {
            name: variable_name,
            label: format!("Environment {environment_name}"),
            placeholder: String::new(),
            description: None,
            required: true,
            pattern: None,
            source: Some(PromptVariableSource::ProjectEnv),
            source_ref: Some(format!("<env.{environment_name}>")),
            allow_manual_override: false,
            control: None,
        });
    }
    result
}

/// Render values prepared for a Quick Prompt without persisting them in the
/// prompt body. Internal template-environment names intentionally map to both
/// supported authoring forms.
pub fn render_quick_prompt_template(
    template: &str,
    values: &std::collections::HashMap<String, String>,
    declarations: &[PromptVariable],
) -> String {
    let environment_bindings: HashMap<_, _> = template_environment_bindings(template, declarations)
        .into_iter()
        .map(|(variable_name, environment_name)| (environment_name, variable_name))
        .collect();
    let mut rendered = String::with_capacity(template.len());
    let mut index = 0;
    while index < template.len() {
        let remaining = &template[index..];
        let environment_reference = if remaining.starts_with("{{env.") {
            remaining[6..].find("}}").map(|end| (6, end + 6, end + 8))
        } else if remaining.starts_with("<env.") {
            remaining[5..].find('>').map(|end| (5, end + 5, end + 6))
        } else {
            None
        };
        if let Some((name_start, name_end, placeholder_end)) = environment_reference {
            let environment_name = &remaining[name_start..name_end];
            if is_environment_name(environment_name) {
                if let Some(variable_name) = environment_bindings.get(environment_name) {
                    if let Some(value) = values.get(variable_name) {
                        rendered.push_str(value);
                        index += placeholder_end;
                        continue;
                    }
                }
            }
        }
        if let Some(name) = remaining
            .strip_prefix("{{")
            .and_then(|rest| rest.find("}}").map(|end| &rest[..end]))
        {
            if let Some(value) = values.get(name) {
                rendered.push_str(value);
                index += name.len() + 4;
                continue;
            }
        }
        let character = remaining.chars().next().expect("index is in bounds");
        rendered.push(character);
        index += character.len_utf8();
    }
    rendered
}

/// Pair each environment name in a template with a runtime-only declaration
/// name. The collision-free name is derived from the persisted declarations,
/// so rendering can distinguish it from a user variable with the same prefix.
fn template_environment_bindings(
    template: &str,
    declarations: &[PromptVariable],
) -> Vec<(String, String)> {
    let mut names: HashSet<String> = declarations
        .iter()
        .map(|variable| variable.name.clone())
        .collect();
    let mut references = template_environment_references(template);
    references.sort();
    references.dedup();

    references
        .into_iter()
        .map(|environment_name| {
            let mut variable_name = format!("{TEMPLATE_ENV_VARIABLE_PREFIX}{environment_name}");
            let mut suffix = 2;
            while names.contains(&variable_name) {
                variable_name =
                    format!("{TEMPLATE_ENV_VARIABLE_PREFIX}{environment_name}#{suffix}");
                suffix += 1;
            }
            names.insert(variable_name.clone());
            (variable_name, environment_name)
        })
        .collect()
}

fn template_environment_references(template: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut index = 0;
    while index < template.len() {
        let (prefix, suffix) = if template[index..].starts_with("{{env.") {
            ("{{env.", "}}")
        } else if template[index..].starts_with("<env.") {
            ("<env.", ">")
        } else {
            index += template[index..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            continue;
        };
        let start = index + prefix.len();
        let Some(end_offset) = template[start..].find(suffix) else {
            index = start;
            continue;
        };
        let end = start + end_offset;
        let name = &template[start..end];
        if is_environment_name(name) {
            references.push(name.to_string());
        }
        index = end + suffix.len();
    }
    references
}

fn is_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn default_variable_required() -> bool {
    true
}

fn default_variable_option_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPrompt {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub prompt_template: String,
    pub variables: Vec<PromptVariable>,
    pub agent: AgentType,
    /// Named external API connection used when `agent` is `Custom` (or when
    /// selecting a specific named HTTP provider instance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// 0.8.5 — optional profile binding (persona injection at launch).
    /// Mirrors `WorkflowStep.profile_ids` + `Discussion.profile_ids`. Empty
    /// vec = no profile bound (legacy behaviour).
    #[serde(default)]
    pub profile_ids: Vec<String>,
    /// 0.8.5 — optional directive binding (rules-of-conduct at launch).
    /// Mirrors `WorkflowStep.directive_ids` + `Discussion.directive_ids`.
    /// Empty vec = no directive bound (legacy behaviour).
    #[serde(default)]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    /// 0.8.10 — optional explicit model (+ effort / max_tokens), mirroring
    /// `WorkflowStep.agent_settings`. `agent_settings.model` wins over `tier`
    /// (see `runner::effective_model_flag`). Copied onto a workflow step by
    /// `quick_prompt_hydrate`, and onto the launched discussion by the QP
    /// launch path. `None` = resolve the model from `tier` as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_settings: Option<AgentSettings>,
    /// Optional human description of what this Quick Prompt does. Shown
    /// in the batch-workflow picker so the user knows which QP fits their
    /// use case. Empty string = legacy QP created before 2026-04-10.
    #[serde(default)]
    pub description: String,
    /// User-pinned / favorite Quick Prompt in the Automation library.
    #[serde(default)]
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickPromptRequest {
    pub name: String,
    pub icon: Option<String>,
    pub prompt_template: String,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    pub agent: Option<AgentType>,
    #[serde(default)]
    pub connection_id: Option<String>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    #[serde(default)]
    pub agent_settings: Option<AgentSettings>,
    #[serde(default)]
    pub description: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick APIs (0.6.0) — reusable API call templates with {{variables}}.
// Same pattern as QuickPrompt but the engine is HTTP, not LLM. Field names
// follow `WorkflowStep` ApiCall fields verbatim so the frontend can reuse
// `ApiCallStepCard` (and therefore `ApiCallAiHelper`) as the editor.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApi {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Optional human description — shown in the BatchApiCall picker.
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,

    // API request shape — same field names as WorkflowStep ApiCall fields.
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_query: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    /// Same shape as `WorkflowStep.api_body`: a JSON `Value` rather than a
    /// raw string. The runtime engine walks the tree and interpolates
    /// string leaves only — no string-level templating that would let a
    /// `{{var}}` containing `","` punch through into JSON injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_body: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_extract: Option<ExtractSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_pagination: Option<PaginationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_max_retries: Option<u8>,

    /// Variables prompted at run-time (single-call) or whose names become
    /// the keys mapped from each batch item (batch-call).
    pub variables: Vec<PromptVariable>,

    /// 0.8.5 — optional profile binding. Picked up by any downstream
    /// agent surface that consumes this Quick API (e.g. the "Compare
    /// agents" QA helper). Empty vec = unbound. Pure API calls ignore
    /// this; it only matters when the QA result feeds into an LLM step.
    #[serde(default)]
    pub profile_ids: Vec<String>,
    /// 0.8.5 — optional directive binding. Same rationale as
    /// `profile_ids` above.
    #[serde(default)]
    pub directive_ids: Vec<String>,

    /// User-pinned / favorite Quick API in the Automation library.
    #[serde(default)]
    pub pinned: bool,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickApiRequest {
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    pub api_method: Option<String>,
    pub api_query: Option<std::collections::HashMap<String, String>>,
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    pub api_body: Option<serde_json::Value>,
    pub api_extract: Option<ExtractSpec>,
    pub api_pagination: Option<PaginationSpec>,
    pub api_timeout_ms: Option<u64>,
    pub api_max_retries: Option<u8>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Execs (saved shell-free CLI data collectors)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExec {
    pub id: String,
    pub name: String,
    pub icon: String,
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub timeout_secs: u32,
    pub output_format: super::CollectQuickExecOutputFormat,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    /// User-pinned / favorite Quick Exec in the Automation library.
    #[serde(default)]
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickExecRequest {
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub output_format: super::CollectQuickExecOutputFormat,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
}

/// Partial favorite toggle shared by Quick Prompts, Quick APIs and Quick
/// Execs. All three resource routes accept the exact same PATCH payload.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateQuickFavoriteRequest {
    pub pinned: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RunQuickExecRequest {
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
    /// Deterministic source-discussion context for a launch proposed inline
    /// from a discussion (KT-476). Server-owned: never accepted from the
    /// wire, so an HTTP caller cannot spoof another project's environment or
    /// worktree.
    #[serde(skip)]
    #[ts(skip)]
    pub launch: Option<crate::core::launch_context::LaunchContext>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RunQuickExecResponse {
    pub run_id: String,
    pub success: bool,
    pub duration_ms: u64,
    #[ts(type = "any")]
    pub data: Option<serde_json::Value>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickExecRequest {
    pub content: String,
    pub project_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExecExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub quick_exec: QuickExec,
}

/// Canonicalize the JSON body accepted by Quick API authoring surfaces.
///
/// Older UI/agent helpers encoded an object once before sending it, turning
/// `{ "foo": "bar" }` into a top-level JSON string. `reqwest::json` then
/// correctly encoded that string a second time, so APIs received a JSON
/// string instead of the object they declared. Keep legitimate scalar string
/// bodies untouched, but recover object/array text at the Quick API boundary
/// so already-saved QAs start working without a migration.
pub fn normalize_quick_api_body(body: Option<serde_json::Value>) -> Option<serde_json::Value> {
    body.map(|value| match value {
        serde_json::Value::String(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(parsed @ serde_json::Value::Object(_))
            | Ok(parsed @ serde_json::Value::Array(_)) => parsed,
            _ => serde_json::Value::String(raw),
        },
        other => other,
    })
}

// 0.8.5 — Quick Prompt version snapshot. Written by `db::quick_prompts`
// on every INSERT (v1) and UPDATE (v2, v3, …). Carries every editable
// field at the time of the change so the history drawer can render the
// timeline + the metrics aggregator can group launches by `(qp_id,
// version_index)` via the matching columns on `discussions`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptVersion {
    pub id: String,
    pub quick_prompt_id: String,
    pub version_index: u32,
    pub name: String,
    pub icon: String,
    pub prompt_template: String,
    pub variables: Vec<PromptVariable>,
    pub agent: AgentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    pub project_id: Option<String>,
    pub skill_ids: Vec<String>,
    pub profile_ids: Vec<String>,
    pub directive_ids: Vec<String>,
    pub tier: ModelTier,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

// 0.8.5 — Aggregated launch metrics for a single QP version. Returned
// by `GET /api/quick-prompts/:id/metrics` (one row per `version_index`
// that has at least one launch with `originating_qp_version` set).
// Only the FIRST agent reply of each discussion is counted — that's
// the message that reflects the QP's pertinence; follow-up turns are
// driven by the user's reactions, not the QP itself.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptVersionMetrics {
    pub version_index: u32,
    /// Number of launched discussions whose first-agent-reply lands
    /// in this version's window. Pertinence Δs are only emitted when
    /// `launches >= 3` (the noise floor).
    pub launches: u32,
    pub avg_tokens: u64,
    /// Mean wall-clock duration of the first agent reply, milliseconds.
    /// `None` when no launch in this version has a captured
    /// `duration_ms` (legacy rows or imported transcripts).
    pub avg_duration_ms: Option<u64>,
    /// Mean USD cost of the first agent reply. `None` when no launch
    /// has cost data (e.g. local Ollama runs).
    pub avg_cost_usd: Option<f64>,
}

// Skills / Profiles / Directives extracted to `agents.rs` (TD-models-monolith).

// ─── Quick Prompts / APIs API requests ────────────────────────────────────

/// 0.7.0 UX pass — payload for `POST /api/quick-prompts/import`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickPromptRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// Self-contained envelope produced by `GET /api/quick-prompts/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// Like the workflow envelope: `id`, `project_id`, `created_at`,
    /// `updated_at` are present on the wire but reset at import.
    pub quick_prompt: QuickPrompt,
}

/// 0.6.0 — payload for `POST /api/quick-apis/import`. Mirrors the QP shape.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickApiRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// Self-contained envelope produced by `GET /api/quick-apis/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApiExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// `id`, `project_id`, `created_at`, `updated_at` are present on the
    /// wire but reset at import — fresh values are minted by the importer.
    pub quick_api: QuickApi,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/run`. Lets the user
/// launch a saved QuickApi standalone (Run drawer in the Quick APIs page),
/// passing values for the declared `variables`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RunQuickApiRequest {
    /// Map of variable name → user-entered value. Keys must match
    /// `QuickApi.variables[*].name`. Missing keys for required variables
    /// get the call rejected before any HTTP fires.
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
    /// Server-owned attribution for a Quick API invoked by an HTTP workflow
    /// Agent. Skipped at the HTTP/TypeScript boundary so callers cannot forge
    /// a workflow run identity.
    #[serde(skip)]
    #[ts(skip)]
    pub workflow_run_id: Option<String>,
    #[serde(skip)]
    #[ts(skip)]
    pub agent: Option<String>,
    /// Deterministic source-discussion context for a launch proposed inline
    /// from a discussion (KT-476). Server-owned: never accepted from the
    /// wire, so an HTTP caller cannot spoof another project's environment.
    #[serde(skip)]
    #[ts(skip)]
    pub launch: Option<crate::core::launch_context::LaunchContext>,
}

/// Response from `POST /api/quick-apis/:id/run`. Mirrors the
/// `/test-api-call` shape so the frontend can reuse the same UI.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RunQuickApiResponse {
    pub run_id: String,
    pub success: bool,
    pub duration_ms: u64,
    /// Parsed envelope (data/status/summary) on success, `None` on failure.
    pub envelope: Option<serde_json::Value>,
    /// Error message on failure, `None` on success.
    pub error: Option<String>,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/batch`. Fan-out the same
/// QA over a list of items (sub-domains, ticket keys, languages, etc.)
/// without needing a workflow. Mirror of the `BatchApiCall` step type
/// but standalone — uses the same parallel HTTP executor under the hood.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiRequest {
    /// Items to fan-out over. Accepts:
    ///   - JSON array of strings (each fills the QA's first variable):
    ///     `["www.example.com", "de.example.com", "fr.example.com"]`
    ///   - JSON array of objects (each key maps to a variable name):
    ///     `[{"host":"www.example.com","limit":"5"}, ...]`
    pub items: serde_json::Value,
    /// Max parallel HTTP calls (default 5, hard-capped at 20).
    #[serde(default)]
    pub concurrent_limit: Option<u32>,
}

/// Response from `POST /api/quick-apis/:id/batch`. The full aggregated
/// envelope produced by the BatchApiCall executor — the frontend renders
/// `envelope.data.items[]` as a per-item result table.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiResponse {
    pub run_id: String,
    /// Overall status: `OK` (all succeeded), `PARTIAL` (some failed), `ERROR` (all failed).
    pub status: String,
    pub duration_ms: u64,
    pub envelope: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(source: PromptVariableSource, source_ref: Option<&str>) -> PromptVariable {
        PromptVariable {
            name: "token".into(),
            label: "Token".into(),
            placeholder: String::new(),
            description: None,
            required: true,
            pattern: None,
            source: Some(source),
            source_ref: source_ref.map(str::to_owned),
            allow_manual_override: false,
            control: None,
        }
    }

    #[test]
    fn project_environment_variables_keep_only_a_declarative_reference() {
        let declaration = variable(PromptVariableSource::ProjectEnv, Some("<env.API_TOKEN>"));
        assert!(declaration.validate_source().is_ok());
        assert!(!declaration.requires_user_input());
        assert!(
            variable(PromptVariableSource::ProjectEnv, Some("secret-value"))
                .validate_source()
                .is_err()
        );
    }

    #[test]
    fn allowing_an_override_never_makes_a_resolved_variable_a_required_input() {
        let mut declaration = variable(PromptVariableSource::ProjectEnv, Some("<env.API_TOKEN>"));
        declaration.allow_manual_override = true;
        // An override is possible but optional: Kronn still resolves the value
        // from the project, so the launcher must not be forced to supply one.
        assert!(!declaration.requires_user_input());
        let mut context = variable(PromptVariableSource::KronnContext, Some("<context.locale>"));
        context.allow_manual_override = true;
        assert!(!context.requires_user_input());
    }

    #[test]
    fn context_variables_require_a_source_reference() {
        assert!(variable(PromptVariableSource::KronnContext, None)
            .validate_source()
            .is_err());
    }

    #[test]
    fn declarations_reject_literals_and_duplicate_names() {
        let mut first = variable(PromptVariableSource::ProjectEnv, Some("<env.API_TOKEN>"));
        let second = first.clone();
        assert!(validate_prompt_variables(&[first.clone(), second]).is_err());
        first.source_ref = Some("<env.123bad>".into());
        assert!(validate_prompt_variables(&[first]).is_err());
    }

    #[test]
    fn template_environment_references_use_the_recommended_syntax_and_keep_legacy_support() {
        let declarations = declarations_with_template_environment_variables(
            "é {{env.API_TOKEN}} then <env.LEGACY_TOKEN> and {{env.API_TOKEN}}",
            &[],
        );
        assert_eq!(declarations.len(), 2);
        assert!(declarations.iter().all(|variable| {
            variable.source == Some(PromptVariableSource::ProjectEnv)
                && variable
                    .source_ref
                    .as_deref()
                    .is_some_and(|reference| reference.starts_with("<env."))
        }));

        let values = std::collections::HashMap::from([
            (
                "__kronn_template_env__API_TOKEN".into(),
                "recommended".into(),
            ),
            ("__kronn_template_env__LEGACY_TOKEN".into(), "legacy".into()),
        ]);
        assert_eq!(
            render_quick_prompt_template("{{env.API_TOKEN}} / <env.LEGACY_TOKEN>", &values, &[]),
            "recommended / legacy"
        );
    }

    #[test]
    fn template_environment_references_ignore_invalid_names_and_do_not_overwrite_regular_variables()
    {
        let declarations = declarations_with_template_environment_variables(
            "{{env.123BAD}} {{env.OK_NAME}} {{name}}",
            &[variable(PromptVariableSource::UserInput, None)],
        );
        assert_eq!(declarations.len(), 2);
        let values = std::collections::HashMap::from([
            ("name".into(), "manual".into()),
            ("__kronn_template_env__OK_NAME".into(), "environment".into()),
        ]);
        assert_eq!(
            render_quick_prompt_template(
                "{{name}} {{env.OK_NAME}} {{env.123BAD}}",
                &values,
                &[variable(PromptVariableSource::UserInput, None)]
            ),
            "manual environment {{env.123BAD}}"
        );
    }

    #[test]
    fn template_environment_names_ending_in_digits_render_without_truncation() {
        let declarations =
            declarations_with_template_environment_variables("{{env.SERVICE_2}}", &[]);
        let generated_name = &declarations[0].name;
        let values = std::collections::HashMap::from([(generated_name.clone(), "resolved".into())]);
        assert_eq!(
            render_quick_prompt_template("{{env.SERVICE_2}}", &values, &[]),
            "resolved"
        );
    }

    #[test]
    fn template_environment_rendering_preserves_prefix_named_user_variables() {
        let user_variable = PromptVariable {
            name: "__kronn_template_env__API_TOKEN".into(),
            label: "ordinary".into(),
            placeholder: String::new(),
            description: None,
            required: true,
            pattern: None,
            source: Some(PromptVariableSource::UserInput),
            source_ref: None,
            allow_manual_override: false,
            control: None,
        };
        let declarations = declarations_with_template_environment_variables(
            "{{__kronn_template_env__API_TOKEN}} {{env.API_TOKEN}} <env.API_TOKEN>",
            std::slice::from_ref(&user_variable),
        );
        let environment_name = declarations[1].name.clone();
        assert_eq!(environment_name, "__kronn_template_env__API_TOKEN#2");
        let values = std::collections::HashMap::from([
            (user_variable.name.clone(), "ordinary".into()),
            (environment_name, "environment".into()),
        ]);
        assert_eq!(
            render_quick_prompt_template(
                "{{__kronn_template_env__API_TOKEN}} {{env.API_TOKEN}} <env.API_TOKEN>",
                &values,
                &[user_variable],
            ),
            "ordinary environment environment"
        );
    }

    #[test]
    fn template_environment_reference_without_a_project_value_fails_preflight() {
        let declarations =
            declarations_with_template_environment_variables("{{env.API_TOKEN}}", &[]);
        let failures = crate::core::execution_variables::resolve(
            &declarations,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            None,
            "project_mcp_configs",
        )
        .unwrap_err();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].cause, "missing_source");
    }

    #[test]
    fn legacy_variables_default_to_text_and_selects_validate_their_contract() {
        let legacy: PromptVariable = serde_json::from_value(serde_json::json!({
            "name": "topic",
            "label": "Topic",
            "placeholder": "",
            "required": true,
            "source": "user_input",
            "allow_manual_override": false
        }))
        .unwrap();
        assert!(legacy.control.is_none());
        assert!(legacy.accepts_value("anything"));

        let mut select = variable(PromptVariableSource::UserInput, None);
        select.control = Some(PromptVariableControl::Select {
            options: vec![
                PromptVariableOption {
                    value: "fr".into(),
                    label: "Français".into(),
                    enabled: true,
                },
                PromptVariableOption {
                    value: "en".into(),
                    label: "English".into(),
                    enabled: false,
                },
            ],
            default_value: Some("fr".into()),
        });
        assert!(validate_prompt_variables(&[select.clone()]).is_ok());
        assert!(select.accepts_value("fr"));
        assert!(!select.accepts_value("en"));
        assert_eq!(select.default_input_value(), Some("fr"));

        if let Some(PromptVariableControl::Select { default_value, .. }) = select.control.as_mut() {
            *default_value = Some("en".into());
        }
        assert!(validate_prompt_variables(&[select]).is_err());
    }
}
