//! Durable, human-gated actions proposed from discussion messages.
//!
//! Agents emit one typed `kronn-action` fence. The fence is parsed exactly
//! once while its message is inserted; every UI render reads this table and
//! never reparses agent prose. Target and variable validation happen in the
//! same transaction as the message.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::kronn_action_engine;
use crate::models::{PromptVariable, PromptVariableControl, PromptVariableSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionActionKind {
    QuickPrompt,
    QuickApi,
    QuickExec,
    Workflow,
    /// Storage-only sentinel for a fence Kronn could not parse into a
    /// runnable proposal (invalid JSON). Never launchable: rows carrying it
    /// are always created directly in `preflight_failed`.
    Invalid,
}

impl DiscussionActionKind {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::QuickPrompt => "quick_prompt",
            Self::QuickApi => "quick_api",
            Self::QuickExec => "quick_exec",
            Self::Workflow => "workflow",
            Self::Invalid => "invalid",
        }
    }

    pub(crate) fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "quick_prompt" => Some(Self::QuickPrompt),
            "quick_api" => Some(Self::QuickApi),
            "quick_exec" => Some(Self::QuickExec),
            "workflow" => Some(Self::Workflow),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionActionState {
    Proposed,
    Launching,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    PreflightFailed,
}

impl DiscussionActionState {
    pub(crate) fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "proposed" => Some(Self::Proposed),
            "launching" => Some(Self::Launching),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "preflight_failed" => Some(Self::PreflightFailed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionActionValueProvenance {
    UserInput,
    AgentSuggestion,
    KronnContext,
    ProjectEnv,
    /// Resolved from the Live Page/card/dataset-row the CTA was clicked in
    /// (KT-538). Only accepted for Live-Page-authored proposals — a
    /// discussion `kronn-action` fence still fails closed on this provenance,
    /// exactly like the readonly, never-resolved `dynamic_binding` KT-476
    /// removed. Unlike that dead field, this variant is only ever populated
    /// by `live_page_actions::claim_launch` looking up the real dataset
    /// value server-side; the value a sandboxed click claims is never
    /// trusted directly. See `backend/src/db/live_page_actions.rs`.
    DynamicBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionActionValue {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    pub description: Option<String>,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub control: Option<PromptVariableControl>,
    /// Mirrors `PromptVariable::allow_manual_override` from the target's own
    /// declaration: whether a `project_env`/`kronn_context` value may be
    /// optionally overridden at launch instead of always being read-only.
    #[serde(default)]
    pub allow_manual_override: bool,
    pub provenance: DiscussionActionValueProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub suggested_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub suggested_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionAction {
    pub id: String,
    pub discussion_id: String,
    pub source_message_id: String,
    pub fence_index: i64,
    pub kind: DiscussionActionKind,
    pub target_id: String,
    pub target_name: String,
    pub project_id: Option<String>,
    pub state: DiscussionActionState,
    pub values: Vec<DiscussionActionValue>,
    pub shared_run_id: Option<String>,
    pub result_discussion_id: Option<String>,
    pub deep_link: Option<String>,
    pub diagnostic: Option<String>,
    pub launched_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchDiscussionActionRequest {
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: std::collections::HashMap<String, String>,
}

pub enum ClaimLaunchOutcome {
    Claimed {
        action: DiscussionAction,
        /// The resolved runtime values for this launch, in memory only.
        /// Never derive these from `action.values` — that copy is the one
        /// persisted/returned to every future GET and never carries a
        /// manually supplied plaintext value (see `claim_launch`).
        variables: std::collections::HashMap<String, String>,
    },
    Existing(DiscussionAction),
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ActionFence {
    pub(crate) kind: DiscussionActionKind,
    pub(crate) target_id: String,
    #[serde(default)]
    pub(crate) project_id: Option<String>,
    #[serde(default)]
    pub(crate) values: Vec<ActionFenceValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ActionFenceValue {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) value: Option<String>,
    #[serde(default = "user_input_provenance")]
    pub(crate) provenance: DiscussionActionValueProvenance,
    #[serde(default)]
    pub(crate) source_ref: Option<String>,
    #[serde(default)]
    pub(crate) suggested_by: Option<String>,
}

fn user_input_provenance() -> DiscussionActionValueProvenance {
    DiscussionActionValueProvenance::UserInput
}

/// Extract typed action fences without interpreting surrounding prose.
pub fn extract_action_fences(content: &str) -> Vec<String> {
    let mut fences = Vec::new();
    let mut in_fence = false;
    let mut buffer = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !in_fence
            && trimmed.starts_with("```")
            && trimmed.trim_start_matches('`').trim() == "kronn-action"
        {
            in_fence = true;
            buffer.clear();
            continue;
        }
        if in_fence && trimmed.starts_with("```") {
            fences.push(std::mem::take(&mut buffer));
            in_fence = false;
            continue;
        }
        if in_fence {
            buffer.push_str(line);
            buffer.push('\n');
        }
    }
    fences
}

pub(crate) struct TargetContract {
    pub(crate) name: String,
    pub(crate) project_id: Option<String>,
    pub(crate) variables: Vec<PromptVariable>,
}

pub(crate) fn target_contract(
    conn: &Connection,
    kind: DiscussionActionKind,
    target_id: &str,
) -> Result<Option<TargetContract>> {
    Ok(match kind {
        DiscussionActionKind::QuickPrompt => {
            crate::db::quick_prompts::get_quick_prompt(conn, target_id)?.map(|item| {
                TargetContract {
                    name: item.name,
                    project_id: item.project_id,
                    variables: item.variables,
                }
            })
        }
        DiscussionActionKind::QuickApi => crate::db::quick_apis::get_quick_api(conn, target_id)?
            .map(|item| TargetContract {
                name: item.name,
                project_id: item.project_id,
                variables: item.variables,
            }),
        DiscussionActionKind::QuickExec => crate::db::quick_execs::get_quick_exec(conn, target_id)?
            .map(|item| TargetContract {
                name: item.name,
                project_id: item.project_id,
                variables: item.variables,
            }),
        DiscussionActionKind::Workflow => {
            crate::db::workflows::get_workflow(conn, target_id)?.map(|item| TargetContract {
                name: item.name,
                project_id: item.project_id,
                variables: item.variables,
            })
        }
        // An `invalid` fence never reaches this lookup — it is inserted
        // directly by `insert_unusable_proposal` — but the match must stay
        // exhaustive against a caller that forged this sentinel by hand.
        DiscussionActionKind::Invalid => None,
    })
}

/// `allow_dynamic_binding` gates the `DynamicBinding` provenance: `false` for
/// discussion `kronn-action` fences (fails closed, mirroring the KT-476
/// removal of the dead `dynamic_binding` field), `true` for Live Page action
/// blocks (KT-538), where `live_page_actions::claim_launch` resolves the
/// value server-side from the real dataset/page row instead of ever trusting
/// a client-declared value for it.
pub(crate) fn resolved_values(
    declarations: &[PromptVariable],
    proposed: Vec<ActionFenceValue>,
    allow_dynamic_binding: bool,
) -> Result<Vec<DiscussionActionValue>> {
    let mut proposed = proposed
        .into_iter()
        .map(|value| (value.name.clone(), value))
        .collect::<std::collections::HashMap<_, _>>();
    let mut values = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let source = declaration.source.clone().unwrap_or_default();
        let proposal = proposed.remove(&declaration.name);
        let label = if declaration.label.trim().is_empty() {
            declaration.name.clone()
        } else {
            declaration.label.clone()
        };
        let value = match source {
            PromptVariableSource::ProjectEnv => DiscussionActionValue {
                name: declaration.name.clone(),
                label,
                placeholder: declaration.placeholder.clone(),
                description: declaration.description.clone(),
                required: declaration.required,
                control: declaration.control.clone(),
                allow_manual_override: declaration.allow_manual_override,
                provenance: DiscussionActionValueProvenance::ProjectEnv,
                value: None,
                source_ref: declaration.source_ref.clone(),
                suggested_by: None,
                suggested_value: None,
            },
            PromptVariableSource::KronnContext => DiscussionActionValue {
                name: declaration.name.clone(),
                label,
                placeholder: declaration.placeholder.clone(),
                description: declaration.description.clone(),
                required: declaration.required,
                control: declaration.control.clone(),
                allow_manual_override: declaration.allow_manual_override,
                provenance: DiscussionActionValueProvenance::KronnContext,
                value: None,
                source_ref: declaration.source_ref.clone(),
                suggested_by: None,
                suggested_value: None,
            },
            PromptVariableSource::UserInput => match proposal {
                Some(proposal)
                    if matches!(
                        proposal.provenance,
                        DiscussionActionValueProvenance::AgentSuggestion
                            | DiscussionActionValueProvenance::UserInput
                    ) =>
                {
                    let suggested_value = matches!(
                        proposal.provenance,
                        DiscussionActionValueProvenance::AgentSuggestion
                    )
                    .then(|| proposal.value.clone())
                    .flatten();
                    DiscussionActionValue {
                        name: declaration.name.clone(),
                        label,
                        placeholder: declaration.placeholder.clone(),
                        description: declaration.description.clone(),
                        required: declaration.required,
                        control: declaration.control.clone(),
                        allow_manual_override: declaration.allow_manual_override,
                        provenance: proposal.provenance,
                        value: proposal
                            .value
                            .or_else(|| declaration.default_input_value().map(str::to_owned)),
                        source_ref: proposal.source_ref,
                        suggested_by: proposal.suggested_by,
                        suggested_value,
                    }
                }
                Some(proposal)
                    if proposal.provenance == DiscussionActionValueProvenance::DynamicBinding =>
                {
                    if !allow_dynamic_binding {
                        anyhow::bail!(
                            "dynamic_binding proposals are not accepted for discussion actions (variable `{}`)",
                            declaration.name
                        );
                    }
                    let source_ref = proposal
                        .source_ref
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "dynamic_binding variable `{}` requires a source_ref",
                                declaration.name
                            )
                        })?;
                    DiscussionActionValue {
                        name: declaration.name.clone(),
                        label,
                        placeholder: declaration.placeholder.clone(),
                        description: declaration.description.clone(),
                        required: declaration.required,
                        control: declaration.control.clone(),
                        allow_manual_override: declaration.allow_manual_override,
                        provenance: DiscussionActionValueProvenance::DynamicBinding,
                        value: None,
                        source_ref: Some(source_ref),
                        suggested_by: None,
                        suggested_value: None,
                    }
                }
                Some(_) => anyhow::bail!(
                    "manual variable `{}` cannot claim an environment provenance",
                    declaration.name
                ),
                None => DiscussionActionValue {
                    name: declaration.name.clone(),
                    label,
                    placeholder: declaration.placeholder.clone(),
                    description: declaration.description.clone(),
                    required: declaration.required,
                    control: declaration.control.clone(),
                    allow_manual_override: declaration.allow_manual_override,
                    provenance: DiscussionActionValueProvenance::UserInput,
                    value: declaration.default_input_value().map(str::to_owned),
                    source_ref: None,
                    suggested_by: None,
                    suggested_value: None,
                },
            },
        };
        values.push(value);
    }
    if let Some(unknown) = proposed.keys().next() {
        anyhow::bail!("unknown target variable `{unknown}`");
    }
    Ok(values)
}

/// Persist every valid action proposal in the caller's message transaction.
/// Invalid semantic proposals are retained as `preflight_failed` cards so the
/// human sees an actionable reason instead of a mysteriously missing CTA.
pub fn ingest_message_actions(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
    content: &str,
) -> Result<()> {
    if !content.contains("kronn-action") {
        return Ok(());
    }
    let discussion_project: Option<String> = conn.query_row(
        "SELECT project_id FROM discussions WHERE id = ?1",
        [discussion_id],
        |row| row.get(0),
    )?;
    let now = Utc::now().to_rfc3339();
    for (fence_index, raw) in extract_action_fences(content).into_iter().enumerate() {
        let action_id = format!("action:{message_id}:{fence_index}");
        let fence = match serde_json::from_str::<ActionFence>(&raw) {
            Ok(fence) => fence,
            Err(error) => {
                insert_unusable_proposal(
                    conn,
                    &action_id,
                    discussion_id,
                    message_id,
                    fence_index,
                    None,
                    format!(
                        "Cette proposition d’action n’a pas pu être lue (JSON invalide) : {error}."
                    ),
                    &now,
                )?;
                continue;
            }
        };
        if fence.target_id.trim().is_empty() {
            insert_unusable_proposal(
                conn,
                &action_id,
                discussion_id,
                message_id,
                fence_index,
                Some(fence.kind),
                "Cette proposition d’action ne précise aucune cible (target_id vide).".into(),
                &now,
            )?;
            continue;
        }
        let contract = target_contract(conn, fence.kind, &fence.target_id)?;
        let mut diagnostic = None;
        let (target_name, target_project, values) = match contract {
            Some(contract) => {
                let values = match resolved_values(&contract.variables, fence.values, false) {
                    Ok(values) => values,
                    Err(error) => {
                        diagnostic = Some(error.to_string());
                        Vec::new()
                    }
                };
                (contract.name, contract.project_id, values)
            }
            None => {
                diagnostic = Some("La cible n’existe plus ou n’est pas accessible.".into());
                (fence.target_id.clone(), None, Vec::new())
            }
        };
        let project_id = fence
            .project_id
            .clone()
            .or(target_project.clone())
            .or(discussion_project.clone());
        if target_project.is_some()
            && fence.project_id.is_some()
            && target_project != fence.project_id
        {
            diagnostic = Some("Le projet proposé ne correspond pas au projet de la cible.".into());
        }
        if discussion_project.is_some() && project_id != discussion_project {
            diagnostic =
                Some("Cette action n’est pas autorisée dans le projet de la discussion.".into());
        }
        if let Some(project_id) = project_id.as_deref() {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                [project_id],
                |row| row.get(0),
            )?;
            if !exists {
                diagnostic = Some("Le projet proposé n’existe plus.".into());
            }
        }
        let state = if diagnostic.is_some() {
            "preflight_failed"
        } else {
            "proposed"
        };
        conn.execute(
            "INSERT OR IGNORE INTO discussion_actions (
                 id, discussion_id, source_message_id, fence_index, kind,
                 target_id, target_name, project_id, state, values_json,
                 diagnostic, finished_at, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
            params![
                action_id,
                discussion_id,
                message_id,
                fence_index as i64,
                fence.kind.as_db_str(),
                fence.target_id,
                target_name,
                project_id,
                state,
                serde_json::to_string(&values)?,
                diagnostic,
                diagnostic.as_ref().map(|_| now.clone()),
                now,
            ],
        )?;
    }
    Ok(())
}

/// Insert an actionable `preflight_failed` card for a fence Kronn cannot run
/// at all (unparseable JSON, or a blank `target_id`) instead of silently
/// dropping it. `kind` is `None` only when the JSON itself failed to parse —
/// in that case Kronn cannot know what the agent meant to propose, so the row
/// carries the `invalid` storage sentinel instead of a real kind.
#[allow(clippy::too_many_arguments)]
fn insert_unusable_proposal(
    conn: &Connection,
    action_id: &str,
    discussion_id: &str,
    message_id: &str,
    fence_index: usize,
    kind: Option<DiscussionActionKind>,
    diagnostic: String,
    now: &str,
) -> Result<()> {
    let kind_db = kind.unwrap_or(DiscussionActionKind::Invalid).as_db_str();
    conn.execute(
        "INSERT OR IGNORE INTO discussion_actions (
             id, discussion_id, source_message_id, fence_index, kind,
             target_id, target_name, project_id, state, values_json,
             diagnostic, finished_at, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,'','(proposition invalide)',NULL,'preflight_failed','[]',?6,?7,?7,?7)",
        params![
            action_id,
            discussion_id,
            message_id,
            fence_index as i64,
            kind_db,
            diagnostic,
            now,
        ],
    )?;
    Ok(())
}

fn map_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<DiscussionAction> {
    let kind_raw = row.get::<_, String>(4)?;
    let state_raw = row.get::<_, String>(8)?;
    let values_raw = row.get::<_, String>(9)?;
    Ok(DiscussionAction {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        source_message_id: row.get(2)?,
        fence_index: row.get(3)?,
        kind: DiscussionActionKind::from_db_str(&kind_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                format!("invalid discussion action kind `{kind_raw}`").into(),
            )
        })?,
        target_id: row.get(5)?,
        target_name: row.get(6)?,
        project_id: row.get(7)?,
        state: DiscussionActionState::from_db_str(&state_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                format!("invalid discussion action state `{state_raw}`").into(),
            )
        })?,
        values: serde_json::from_str(&values_raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        shared_run_id: row.get(10)?,
        result_discussion_id: row.get(11)?,
        deep_link: row.get(12)?,
        diagnostic: row.get(13)?,
        launched_at: row.get(14)?,
        finished_at: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

const SELECT_ACTION: &str = "SELECT id, discussion_id, source_message_id,
    fence_index, kind, target_id, target_name, project_id, state, values_json,
    shared_run_id, result_discussion_id, deep_link, diagnostic, launched_at,
    finished_at, created_at, updated_at FROM discussion_actions";

fn refresh_from_shared_run(conn: &Connection, action: &mut DiscussionAction) -> Result<()> {
    let mut core = kronn_action_engine::ActionCore {
        id: action.id.clone(),
        state: action.state,
        values: std::mem::take(&mut action.values),
        shared_run_id: action.shared_run_id.clone(),
        diagnostic: action.diagnostic.clone(),
        launched_at: action.launched_at.clone(),
        finished_at: action.finished_at.clone(),
        updated_at: action.updated_at.clone(),
    };
    kronn_action_engine::refresh_from_shared_run(
        conn,
        kronn_action_engine::ActionTable::Discussion,
        &mut core,
    )?;
    action.state = core.state;
    action.values = core.values;
    action.diagnostic = core.diagnostic;
    action.finished_at = core.finished_at;
    action.updated_at = core.updated_at;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<DiscussionAction>> {
    let mut action = conn
        .query_row(&format!("{SELECT_ACTION} WHERE id = ?1"), [id], map_action)
        .optional()?;
    if let Some(action) = action.as_mut() {
        refresh_from_shared_run(conn, action)?;
    }
    Ok(action)
}

pub fn list_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Vec<DiscussionAction>> {
    let mut statement = conn.prepare(&format!(
        "{SELECT_ACTION} WHERE discussion_id = ?1 ORDER BY created_at, fence_index"
    ))?;
    let mut actions = statement
        .query_map([discussion_id], map_action)?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for action in &mut actions {
        refresh_from_shared_run(conn, action)?;
    }
    Ok(actions)
}

pub fn cancel(conn: &Connection, id: &str) -> Result<Option<DiscussionAction>> {
    kronn_action_engine::cancel(conn, kronn_action_engine::ActionTable::Discussion, id)?;
    get(conn, id)
}

pub fn claim_launch(
    conn: &Connection,
    id: &str,
    supplied: &std::collections::HashMap<String, String>,
) -> Result<Option<ClaimLaunchOutcome>> {
    let transaction = conn.unchecked_transaction()?;
    let Some(mut action) = get(&transaction, id)? else {
        transaction.commit()?;
        return Ok(None);
    };
    if action.state != DiscussionActionState::Proposed {
        transaction.commit()?;
        return Ok(Some(ClaimLaunchOutcome::Existing(action)));
    }
    let mut core = kronn_action_engine::ActionCore {
        id: action.id.clone(),
        state: action.state,
        values: std::mem::take(&mut action.values),
        shared_run_id: action.shared_run_id.clone(),
        diagnostic: action.diagnostic.clone(),
        launched_at: action.launched_at.clone(),
        finished_at: action.finished_at.clone(),
        updated_at: action.updated_at.clone(),
    };
    let claimed_variables = kronn_action_engine::claim_launch(
        &transaction,
        kronn_action_engine::ActionTable::Discussion,
        &mut core,
        supplied,
    )?;
    action.state = core.state;
    action.values = core.values;
    action.launched_at = core.launched_at;
    action.updated_at = core.updated_at;
    transaction.commit()?;
    Ok(Some(match claimed_variables {
        Some(variables) => ClaimLaunchOutcome::Claimed { action, variables },
        None => ClaimLaunchOutcome::Existing(action),
    }))
}

pub use kronn_action_engine::ActionCompletion;

pub fn complete(conn: &Connection, id: &str, completion: ActionCompletion) -> Result<()> {
    kronn_action_engine::complete(
        conn,
        kronn_action_engine::ActionTable::Discussion,
        id,
        completion,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, CollectQuickExecOutputFormat, ModelTier, PromptVariable, PromptVariableSource,
        QuickApi, QuickExec, QuickPrompt,
    };

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('disc-1', 'Actions', ?1, ?1)",
            [Utc::now().to_rfc3339()],
        )
        .unwrap();
        conn
    }

    fn insert_message_row(conn: &Connection, id: &str, content: &str) {
        conn.execute(
            "INSERT INTO messages
                (id, discussion_id, role, channel, content, timestamp, sort_order)
             VALUES (?1, 'disc-1', 'Agent', 'main', ?2, ?3, 0)",
            params![id, content, Utc::now().to_rfc3339()],
        )
        .unwrap();
    }

    fn insert_target(conn: &Connection) {
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            conn,
            &QuickExec {
                id: "qe-1".into(),
                name: "Collect logs".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "service".into(),
                    label: "Service".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: None,
                    source_ref: None,
                    allow_manual_override: false,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
    }

    fn insert_all_target_kinds(conn: &Connection) {
        let now = Utc::now();
        crate::db::quick_prompts::insert_quick_prompt(
            conn,
            &QuickPrompt {
                id: "qp-1".into(),
                pinned: false,
                name: "Frame issue".into(),
                icon: "✨".into(),
                prompt_template: "Frame this issue".into(),
                variables: vec![],
                agent: AgentType::ClaudeCode,
                connection_id: None,
                project_id: None,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                tier: ModelTier::Default,
                agent_settings: None,
                description: String::new(),
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        crate::db::quick_apis::insert_quick_api(
            conn,
            &QuickApi {
                id: "qa-1".into(),
                pinned: false,
                name: "Read ticket".into(),
                description: String::new(),
                icon: "🔌".into(),
                project_id: None,
                api_plugin_slug: "tracker".into(),
                api_config_id: "config".into(),
                api_endpoint_path: "/ticket".into(),
                api_method: Some("GET".into()),
                api_query: None,
                api_path_params: None,
                api_headers: None,
                api_body: None,
                api_extract: None,
                api_pagination: None,
                api_timeout_ms: None,
                api_max_retries: None,
                variables: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflows
                (id, name, trigger_json, steps_json, actions_json, safety_json,
                 variables, enabled, created_at, updated_at)
             VALUES ('wf-1', 'Publish report', '\"Manual\"', '[]', '[]', '{}',
                     '[]', 1, ?1, ?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        insert_target(conn);
    }

    #[test]
    fn fence_is_ingested_once_with_a_validated_target_and_suggestion() {
        let conn = connection();
        insert_target(&conn);
        let content = r#"Je propose de collecter les logs.
```kronn-action
{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","value":"api","provenance":"agent_suggestion","suggested_by":"@codex"}]}
```"#;
        insert_message_row(&conn, "msg-1", content);

        ingest_message_actions(&conn, "disc-1", "msg-1", content).unwrap();
        ingest_message_actions(&conn, "disc-1", "msg-1", content).unwrap();

        let actions = list_for_discussion(&conn, "disc-1").unwrap();
        assert_eq!(actions.len(), 1, "re-ingestion must remain idempotent");
        let action = &actions[0];
        assert_eq!(action.id, "action:msg-1:0");
        assert_eq!(action.kind, DiscussionActionKind::QuickExec);
        assert_eq!(action.target_name, "Collect logs");
        assert_eq!(action.state, DiscussionActionState::Proposed);
        assert_eq!(action.values[0].value.as_deref(), Some("api"));
        assert_eq!(action.values[0].suggested_value.as_deref(), Some("api"));
    }

    #[test]
    fn one_registry_validates_qp_qa_qe_and_workflow_fences() {
        let conn = connection();
        insert_all_target_kinds(&conn);
        let content = r#"```kronn-action
{"kind":"quick_prompt","target_id":"qp-1"}
```
```kronn-action
{"kind":"quick_api","target_id":"qa-1"}
```
```kronn-action
{"kind":"quick_exec","target_id":"qe-1"}
```
```kronn-action
{"kind":"workflow","target_id":"wf-1"}
```"#;
        insert_message_row(&conn, "msg-all", content);
        ingest_message_actions(&conn, "disc-1", "msg-all", content).unwrap();

        let actions = list_for_discussion(&conn, "disc-1").unwrap();
        assert_eq!(
            actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
            vec![
                DiscussionActionKind::QuickPrompt,
                DiscussionActionKind::QuickApi,
                DiscussionActionKind::QuickExec,
                DiscussionActionKind::Workflow,
            ]
        );
        assert!(actions
            .iter()
            .all(|action| action.state == DiscussionActionState::Proposed));
    }

    #[test]
    fn missing_target_becomes_an_actionable_preflight_failure() {
        let conn = connection();
        let content = r#"```kronn-action
{"kind":"workflow","target_id":"missing"}
```"#;
        insert_message_row(&conn, "msg-2", content);
        ingest_message_actions(&conn, "disc-1", "msg-2", content).unwrap();

        let action = get(&conn, "action:msg-2:0").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert!(action.diagnostic.unwrap().contains("n’existe plus"));
    }

    #[test]
    fn launch_claim_is_atomic_and_a_second_click_reuses_the_same_action() {
        let conn = connection();
        insert_target(&conn);
        let content = r#"```kronn-action
{"kind":"quick_exec","target_id":"qe-1"}
```"#;
        insert_message_row(&conn, "msg-3", content);
        ingest_message_actions(&conn, "disc-1", "msg-3", content).unwrap();

        let supplied = std::collections::HashMap::from([("service".into(), "api".into())]);
        let first = claim_launch(&conn, "action:msg-3:0", &supplied)
            .unwrap()
            .unwrap();
        let ClaimLaunchOutcome::Claimed { action, variables } = first else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "api");
        assert!(
            action.values[0].value.is_none(),
            "the manually supplied value must never be persisted in plaintext"
        );
        let second = claim_launch(&conn, "action:msg-3:0", &supplied)
            .unwrap()
            .unwrap();
        assert!(matches!(second, ClaimLaunchOutcome::Existing(_)));
        let reloaded = get(&conn, "action:msg-3:0").unwrap().unwrap();
        assert_eq!(reloaded.state, DiscussionActionState::Launching);
        assert!(
            reloaded.values[0].value.is_none(),
            "a later GET must never replay the manually supplied plaintext value"
        );
    }

    #[test]
    fn allow_manual_override_lets_a_human_override_an_environment_value_without_persisting_it() {
        let conn = connection();
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            &conn,
            &QuickExec {
                id: "qe-env".into(),
                name: "Push status".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "token".into(),
                    label: "Token".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: Some(PromptVariableSource::ProjectEnv),
                    source_ref: Some("<env.TOKEN>".into()),
                    allow_manual_override: true,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        let content = r#"```kronn-action
{"kind":"quick_exec","target_id":"qe-env"}
```"#;
        insert_message_row(&conn, "msg-env", content);
        ingest_message_actions(&conn, "disc-1", "msg-env", content).unwrap();

        let proposed = get(&conn, "action:msg-env:0").unwrap().unwrap();
        assert!(proposed.values[0].allow_manual_override);
        assert_eq!(
            proposed.values[0].provenance,
            DiscussionActionValueProvenance::ProjectEnv
        );

        let supplied =
            std::collections::HashMap::from([("token".into(), "override-secret".into())]);
        let claimed = claim_launch(&conn, "action:msg-env:0", &supplied)
            .unwrap()
            .unwrap();
        let ClaimLaunchOutcome::Claimed { action, variables } = claimed else {
            panic!("an allow_manual_override variable must be claimable");
        };
        assert_eq!(variables["token"], "override-secret");
        assert!(
            action.values[0].value.is_none(),
            "the override must never be persisted in plaintext"
        );
        assert_eq!(
            action.values[0].provenance,
            DiscussionActionValueProvenance::ProjectEnv,
            "provenance stays project_env even when a human overrides it"
        );
    }

    #[test]
    fn an_environment_value_without_override_stays_read_only() {
        let conn = connection();
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            &conn,
            &QuickExec {
                id: "qe-locked".into(),
                name: "Push status".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "token".into(),
                    label: "Token".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: Some(PromptVariableSource::ProjectEnv),
                    source_ref: Some("<env.TOKEN>".into()),
                    allow_manual_override: false,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        let content = r#"```kronn-action
{"kind":"quick_exec","target_id":"qe-locked"}
```"#;
        insert_message_row(&conn, "msg-locked", content);
        ingest_message_actions(&conn, "disc-1", "msg-locked", content).unwrap();

        let supplied =
            std::collections::HashMap::from([("token".into(), "attempted-override".into())]);
        let result = claim_launch(&conn, "action:msg-locked:0", &supplied);
        assert!(
            result.is_err(),
            "a non-overridable environment value must reject a supplied override"
        );
    }

    #[test]
    fn invalid_json_fence_becomes_an_actionable_preflight_failure_instead_of_a_silent_drop() {
        let conn = connection();
        let content = "```kronn-action\n{not valid json\n```";
        insert_message_row(&conn, "msg-bad-json", content);
        ingest_message_actions(&conn, "disc-1", "msg-bad-json", content).unwrap();

        let action = get(&conn, "action:msg-bad-json:0").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert_eq!(action.kind, DiscussionActionKind::Invalid);
        assert!(action.diagnostic.unwrap().contains("JSON invalide"));
    }

    #[test]
    fn empty_target_id_becomes_an_actionable_preflight_failure_instead_of_a_silent_drop() {
        let conn = connection();
        let content = r#"```kronn-action
{"kind":"workflow","target_id":""}
```"#;
        insert_message_row(&conn, "msg-empty-target", content);
        ingest_message_actions(&conn, "disc-1", "msg-empty-target", content).unwrap();

        let action = get(&conn, "action:msg-empty-target:0").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert_eq!(action.kind, DiscussionActionKind::Workflow);
        assert!(action.diagnostic.unwrap().contains("aucune cible"));
    }

    #[test]
    fn dynamic_binding_provenance_is_no_longer_an_accepted_contract() {
        let conn = connection();
        insert_target(&conn);
        let content = r#"```kronn-action
{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","value":"api","provenance":"dynamic_binding","source_ref":"<msg.author>"}]}
```"#;
        insert_message_row(&conn, "msg-dynbind", content);
        ingest_message_actions(&conn, "disc-1", "msg-dynbind", content).unwrap();

        let action = get(&conn, "action:msg-dynbind:0").unwrap().unwrap();
        assert_eq!(
            action.state,
            DiscussionActionState::PreflightFailed,
            "a removed provenance must fail closed as an unreadable proposal, never launch"
        );
        assert!(action.diagnostic.unwrap().contains("dynamic_binding"));
    }

    #[test]
    fn legacy_chain_qp_marker_coexists_with_a_kronn_action_fence() {
        let conn = connection();
        insert_target(&conn);
        let content = "Je termine par KRONN:CHAIN_QP:11111111-1111-1111-1111-111111111111\n\
            ```kronn-action\n{\"kind\":\"quick_exec\",\"target_id\":\"qe-1\"}\n```";
        insert_message_row(&conn, "msg-legacy", content);
        ingest_message_actions(&conn, "disc-1", "msg-legacy", content).unwrap();

        let actions = list_for_discussion(&conn, "disc-1").unwrap();
        assert_eq!(
            actions.len(),
            1,
            "the legacy prose marker must not be parsed as a fence, and must not block the real one"
        );
        assert_eq!(actions[0].kind, DiscussionActionKind::QuickExec);
    }

    #[test]
    fn interrupted_claim_fails_closed_instead_of_replaying_side_effects() {
        let conn = connection();
        insert_target(&conn);
        let content = r#"```kronn-action
{"kind":"quick_exec","target_id":"qe-1"}
```"#;
        insert_message_row(&conn, "msg-stale", content);
        ingest_message_actions(&conn, "disc-1", "msg-stale", content).unwrap();
        let supplied = std::collections::HashMap::from([("service".into(), "api".into())]);
        claim_launch(&conn, "action:msg-stale:0", &supplied).unwrap();
        conn.execute(
            "UPDATE discussion_actions SET launched_at = ?2 WHERE id = ?1",
            params![
                "action:msg-stale:0",
                (Utc::now() - chrono::Duration::minutes(6)).to_rfc3339()
            ],
        )
        .unwrap();

        let action = get(&conn, "action:msg-stale:0").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::Failed);
        assert!(action
            .diagnostic
            .unwrap()
            .contains("not retried automatically"));
    }

    #[test]
    fn parser_ignores_prose_and_extracts_only_the_typed_fence() {
        let content =
            "before\n```json\n{}\n```\n```kronn-action\n{\"kind\":\"workflow\"}\n```\nafter";
        assert_eq!(
            extract_action_fences(content),
            vec!["{\"kind\":\"workflow\"}\n"]
        );
    }
}
