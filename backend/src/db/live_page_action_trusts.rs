//! Trusted Page actions (KT-1029): a human-only approval to run one action
//! without its card, bound to a fingerprint and invalidated for good on change.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use super::discussion_actions::{
    DiscussionActionKind, DiscussionActionState, DiscussionActionValueProvenance,
};
use super::live_page_actions::LivePageAction;
use crate::core::project_profile::ProfileSnapshots;
use crate::models::{StepType, Workflow, WorkflowStep};

/// Which repository profiles a fingerprint covers (KT-920).
#[derive(Debug, Clone, Copy)]
pub enum ProfileView<'a> {
    /// Snapshots read from git just before, outside the connection: what an
    /// approval, the panel and a run's admission compare.
    Fresh(&'a ProfileSnapshots),
    /// The snapshots the approval was made with: for checks inside a write
    /// transaction, which cannot read git. The admission re-checks fresh ones.
    Approved,
}

/// Minimum spacing between two trusted launches of the same row.
pub const ROW_MIN_INTERVAL_SECS: i64 = 2;
/// Trusted launches allowed per action over `ACTION_WINDOW_SECS`.
pub const ACTION_MAX_PER_WINDOW: i64 = 30;
pub const ACTION_WINDOW_SECS: i64 = 60;

/// Why an action cannot be (or no longer is) trusted. Serialized as a stable
/// code the UI translates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LivePageActionTrustRefusal {
    /// Only workflow actions can be trusted.
    NotWorkflow,
    /// The block is invalid or its target failed preflight.
    NotLaunchable,
    /// The block is no longer in the current Page revision.
    StaleSource,
    TargetMissing,
    /// The workflow is disabled; only a human enables one.
    WorkflowDisabled,
    /// An Agent, BatchQuickPrompt, SubWorkflow or TriggerWorkflow step.
    AgentStep,
    /// Skills, profiles or directives, which only an agent reads.
    AgentContext,
    /// A Quick Exec data source, which a run does not freeze yet.
    UnpinnedDependency,
    /// A field the reader types: a card is the only place to type it.
    UserInput,
    /// A value read from the project environment, which may hold a secret.
    SecretValue,
    /// The target or the block names another project than the Page's.
    CrossProject,
    /// What was approved differs from what the Page now offers.
    Changed,
    /// No approval exists, or it was invalidated.
    NotTrusted,
    RateLimited,
}

impl LivePageActionTrustRefusal {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::NotWorkflow => "not_workflow",
            Self::NotLaunchable => "not_launchable",
            Self::StaleSource => "stale_source",
            Self::TargetMissing => "target_missing",
            Self::WorkflowDisabled => "workflow_disabled",
            Self::AgentStep => "agent_step",
            Self::AgentContext => "agent_context",
            Self::UnpinnedDependency => "unpinned_dependency",
            Self::UserInput => "user_input",
            Self::SecretValue => "secret_value",
            Self::CrossProject => "cross_project",
            Self::Changed => "changed",
            Self::NotTrusted => "not_trusted",
            Self::RateLimited => "rate_limited",
        }
    }

    fn from_db_str(raw: &str) -> Self {
        match raw {
            "not_workflow" => Self::NotWorkflow,
            "not_launchable" => Self::NotLaunchable,
            "stale_source" => Self::StaleSource,
            "target_missing" => Self::TargetMissing,
            "workflow_disabled" => Self::WorkflowDisabled,
            "agent_step" => Self::AgentStep,
            "agent_context" => Self::AgentContext,
            "unpinned_dependency" => Self::UnpinnedDependency,
            "user_input" => Self::UserInput,
            "secret_value" => Self::SecretValue,
            "cross_project" => Self::CrossProject,
            "not_trusted" => Self::NotTrusted,
            "rate_limited" => Self::RateLimited,
            // Unknown codes read as the generic, still-closed reason.
            _ => Self::Changed,
        }
    }
}

impl std::fmt::Display for LivePageActionTrustRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "trusted launch refused: {}", self.as_db_str())
    }
}

impl std::error::Error for LivePageActionTrustRefusal {}

/// A stored approval.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageActionTrust {
    pub action_id: String,
    pub live_page_id: String,
    pub action_ref: String,
    pub project_id: Option<String>,
    pub target_id: String,
    pub fingerprint: String,
    /// New on every approval: launches claimed under an older one never run.
    pub approval_id: String,
    pub approved_at: String,
    pub invalidated_at: Option<String>,
    pub invalidated_reason: Option<LivePageActionTrustRefusal>,
}

/// One current offer of a Page, as the trust panel and the click path read it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageActionTrustState {
    pub action_id: String,
    pub action_ref: String,
    pub target_name: String,
    /// What a human approves now; `None` when the action is not eligible.
    pub fingerprint: Option<String>,
    pub refusal: Option<LivePageActionTrustRefusal>,
    pub trust: Option<LivePageActionTrust>,
    /// True only when a valid approval matches the current fingerprint.
    pub active: bool,
}

/// Step types known to run no agent and no other workflow. Any other type,
/// today's or a future one, is refused by default.
const AGENTLESS_STEP_TYPES: &[&str] = &[
    "ApiCall",
    "Notify",
    "Gate",
    "Exec",
    "BatchApiCall",
    "JsonData",
    "CollectApiData",
    "TransformData",
    "PublishPageData",
];

fn agentless_step_type(name: &str) -> bool {
    AGENTLESS_STEP_TYPES.contains(&name)
}

fn step_type_name(step_type: &StepType) -> Option<String> {
    let value = serde_json::to_value(step_type).ok()?;
    value.get("type")?.as_str().map(str::to_owned)
}

/// An agent, or another workflow whose steps this check would not see: the
/// indirect paths are refused rather than followed.
fn step_needs_agent(step: &WorkflowStep) -> bool {
    !step_type_name(&step.step_type).is_some_and(|name| agentless_step_type(&name))
        || step.quick_prompt_id.is_some()
        || step.batch_quick_prompt_id.is_some()
        || step.sub_workflow_id.is_some()
        || !step.batch_chain_prompt_ids.is_empty()
}

pub(crate) const DEP_WORKFLOW: &str = "workflow";
pub(crate) const DEP_QUICK_API: &str = "quick_api";

/// The stored resources the workflow executes, as `(kind, id)` after the same
/// reference resolution as a run, or why a trusted run could not freeze them.
pub(crate) fn execution_dependencies(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
) -> Result<std::result::Result<Vec<(&'static str, String)>, LivePageActionTrustRefusal>> {
    use LivePageActionTrustRefusal as Refusal;
    let mut resolved = workflow.clone();
    crate::core::resource_refs::resolve_run_structured_references(conn, &mut resolved, project_id)?;
    let mut apis = std::collections::BTreeSet::new();
    for step in resolved.steps.iter().chain(resolved.on_failure.iter()) {
        if step_needs_agent(step) {
            return Ok(Err(Refusal::AgentStep));
        }
        if !step.skill_ids.is_empty()
            || !step.profile_ids.is_empty()
            || !step.directive_ids.is_empty()
        {
            return Ok(Err(Refusal::AgentContext));
        }
        if let Some(id) = step
            .quick_api_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            apis.insert(id.to_string());
        }
        for source in step
            .collect_api_data
            .iter()
            .flat_map(|config| &config.sources)
        {
            if !source.quick_exec_id.trim().is_empty() {
                return Ok(Err(Refusal::UnpinnedDependency));
            }
            if !source.quick_api_id.trim().is_empty() {
                apis.insert(source.quick_api_id.clone());
            }
        }
    }
    let mut dependencies = vec![(DEP_WORKFLOW, workflow.id.clone())];
    for id in apis {
        let Some(api) = crate::db::quick_apis::get_quick_api(conn, &id)? else {
            return Ok(Err(Refusal::TargetMissing));
        };
        if !api.profile_ids.is_empty() || !api.directive_ids.is_empty() {
            return Ok(Err(Refusal::AgentContext));
        }
        dependencies.push((DEP_QUICK_API, id));
    }
    Ok(Ok(dependencies))
}

/// Objects sorted at every level, so the hash never depends on map order.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

fn page_project(conn: &Connection, live_page_id: &str) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT project_id FROM live_pages WHERE id = ?1",
        [live_page_id],
        |row| row.get(0),
    )?)
}

/// The fingerprint of an eligible action, or the reason it is not eligible.
pub fn evaluate(
    conn: &Connection,
    action: &LivePageAction,
) -> Result<std::result::Result<String, LivePageActionTrustRefusal>> {
    evaluate_in(conn, action, ProfileView::Approved)
}

/// [`evaluate`] covering the repository profiles `profiles` says.
pub fn evaluate_in(
    conn: &Connection,
    action: &LivePageAction,
    profiles: ProfileView<'_>,
) -> Result<std::result::Result<String, LivePageActionTrustRefusal>> {
    Ok(evaluate_with(conn, action, None, profiles)?.map(|(fingerprint, _)| fingerprint))
}

/// The profile snapshots `action_id` was approved with; none before any.
fn approved_profiles(conn: &Connection, action_id: &str) -> Result<ProfileSnapshots> {
    let stored: Option<Option<String>> = conn
        .query_row(
            "SELECT profile_snapshots_json FROM live_page_action_trusts WHERE action_id = ?1",
            [action_id],
            |row| row.get(0),
        )
        .optional()?;
    match stored.flatten() {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(ProfileSnapshots::new()),
    }
}

/// A fingerprint and the stored resources it depends on.
type Evaluated = (String, Vec<(&'static str, String)>);

/// As [`evaluate`], against `snapshot` instead of the stored workflow when given:
/// the definition a run is about to pin. Also returns what it depends on.
fn evaluate_with(
    conn: &Connection,
    action: &LivePageAction,
    snapshot: Option<&Workflow>,
    profiles: ProfileView<'_>,
) -> Result<std::result::Result<Evaluated, LivePageActionTrustRefusal>> {
    use LivePageActionTrustRefusal as Refusal;
    if action.kind != DiscussionActionKind::Workflow {
        return Ok(Err(Refusal::NotWorkflow));
    }
    if action.state != DiscussionActionState::Proposed {
        return Ok(Err(Refusal::NotLaunchable));
    }
    if action.stale_source {
        return Ok(Err(Refusal::StaleSource));
    }
    let stored;
    let workflow = match snapshot {
        Some(snapshot) => snapshot,
        None => {
            stored = crate::db::workflows::get_workflow(conn, &action.target_id)?;
            match stored.as_ref() {
                Some(workflow) => workflow,
                None => return Ok(Err(Refusal::TargetMissing)),
            }
        }
    };
    if workflow.id != action.target_id {
        return Ok(Err(Refusal::Changed));
    }
    let page_project = page_project(conn, &action.live_page_id)?;
    let foreign_target = workflow
        .project_id
        .as_ref()
        .is_some_and(|project| Some(project) != page_project.as_ref());
    if foreign_target || action.project_id != page_project {
        return Ok(Err(Refusal::CrossProject));
    }
    let dependencies = match execution_dependencies(conn, workflow, action.project_id.as_deref())? {
        Ok(dependencies) => dependencies,
        Err(refusal) => return Ok(Err(refusal)),
    };
    for value in &action.values {
        match value.provenance {
            DiscussionActionValueProvenance::UserInput => return Ok(Err(Refusal::UserInput)),
            DiscussionActionValueProvenance::ProjectEnv => return Ok(Err(Refusal::SecretValue)),
            DiscussionActionValueProvenance::DynamicBinding
            | DiscussionActionValueProvenance::AgentSuggestion
            | DiscussionActionValueProvenance::KronnContext => {}
        }
        // Overridable at launch means a reader could change it in the card.
        if value.allow_manual_override {
            return Ok(Err(Refusal::UserInput));
        }
    }
    if !workflow.enabled {
        return Ok(Err(Refusal::WorkflowDisabled));
    }
    let profile_snapshots = match profiles {
        ProfileView::Fresh(fresh) => fresh.clone(),
        ProfileView::Approved => approved_profiles(conn, &action.id)?,
    };
    let approved = serde_json::json!({
        "live_page_id": action.live_page_id,
        "action_ref": action.action_ref,
        "page_project_id": page_project,
        "block": {
            "kind": action.kind,
            "target_id": action.target_id,
            "project_id": action.project_id,
            "values": action.values,
        },
        // `enabled` is outside it and checked live: only a human turns it back on.
        // The profile values an Exec step may read are part of the revision.
        "workflow_revision": crate::workflows::run_pins::revision_fingerprint_with_profiles(
            conn,
            workflow,
            action.project_id.as_deref(),
            &profile_snapshots,
        )?,
    });
    let digest = Sha256::digest(serde_json::to_vec(&canonical(approved))?);
    let fingerprint = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(Ok((fingerprint, dependencies)))
}

fn map_trust(row: &rusqlite::Row<'_>) -> rusqlite::Result<LivePageActionTrust> {
    let reason: Option<String> = row.get(9)?;
    Ok(LivePageActionTrust {
        action_id: row.get(0)?,
        live_page_id: row.get(1)?,
        action_ref: row.get(2)?,
        project_id: row.get(3)?,
        target_id: row.get(4)?,
        fingerprint: row.get(5)?,
        approval_id: row.get(6)?,
        approved_at: row.get(7)?,
        invalidated_at: row.get(8)?,
        invalidated_reason: reason
            .as_deref()
            .map(LivePageActionTrustRefusal::from_db_str),
    })
}

const SELECT_TRUST: &str = "SELECT action_id, live_page_id, action_ref, project_id, target_id,
    fingerprint, approval_id, approved_at, invalidated_at, invalidated_reason
    FROM live_page_action_trusts";

pub fn get(conn: &Connection, action_id: &str) -> Result<Option<LivePageActionTrust>> {
    Ok(conn
        .query_row(
            &format!("{SELECT_TRUST} WHERE action_id = ?1"),
            [action_id],
            map_trust,
        )
        .optional()?)
}

fn invalidate(
    conn: &Connection,
    action_id: &str,
    reason: LivePageActionTrustRefusal,
) -> Result<()> {
    conn.execute(
        "UPDATE live_page_action_trusts SET invalidated_at = ?2, invalidated_reason = ?3
         WHERE action_id = ?1 AND invalidated_at IS NULL",
        params![action_id, Utc::now().to_rfc3339(), reason.as_db_str()],
    )?;
    Ok(())
}

/// Compare a stored approval with the action as it stands, invalidating it on
/// the first difference. Returns whether it is still in force.
fn revalidate(
    conn: &Connection,
    action: &LivePageAction,
    trust: &mut LivePageActionTrust,
    current: std::result::Result<&String, LivePageActionTrustRefusal>,
) -> Result<bool> {
    if trust.invalidated_at.is_some() {
        return Ok(false);
    }
    let reason = match current {
        Ok(fingerprint) if *fingerprint == trust.fingerprint => return Ok(true),
        Ok(_) => LivePageActionTrustRefusal::Changed,
        Err(refusal) => refusal,
    };
    invalidate(conn, &action.id, reason)?;
    trust.invalidated_at = Some(Utc::now().to_rfc3339());
    trust.invalidated_reason = Some(reason);
    Ok(false)
}

/// Record a human's approval of the action exactly as they reviewed it:
/// `expected_fingerprint` is the one their screen showed.
pub fn approve(
    conn: &Connection,
    action_id: &str,
    expected_fingerprint: &str,
) -> Result<LivePageActionTrust> {
    approve_with_profiles(
        conn,
        action_id,
        expected_fingerprint,
        &ProfileSnapshots::new(),
    )
}

/// [`approve`] against the repository profiles read just before; they are
/// stored with the approval and are what the run it admits will pin.
pub fn approve_with_profiles(
    conn: &Connection,
    action_id: &str,
    expected_fingerprint: &str,
    profiles: &ProfileSnapshots,
) -> Result<LivePageActionTrust> {
    // Immediate: two approvals of one row serialize instead of deadlocking.
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let Some(action) = crate::db::live_page_actions::declaration(&transaction, action_id)? else {
        anyhow::bail!("Action not found");
    };
    let (fingerprint, dependencies) =
        evaluate_with(&transaction, &action, None, ProfileView::Fresh(profiles))??;
    if fingerprint != expected_fingerprint {
        return Err(LivePageActionTrustRefusal::Changed.into());
    }
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "INSERT INTO live_page_action_trusts (
             action_id, live_page_id, action_ref, project_id, target_id,
             fingerprint, approval_id, approved_at, invalidated_at, invalidated_reason,
             profile_snapshots_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,?9)
         ON CONFLICT(action_id) DO UPDATE SET
             project_id = excluded.project_id,
             target_id = excluded.target_id,
             fingerprint = excluded.fingerprint,
             approval_id = excluded.approval_id,
             approved_at = excluded.approved_at,
             invalidated_at = NULL,
             invalidated_reason = NULL,
             profile_snapshots_json = excluded.profile_snapshots_json",
        params![
            action.id,
            action.live_page_id,
            action.action_ref,
            action.project_id,
            action.target_id,
            fingerprint,
            uuid::Uuid::new_v4().to_string(),
            now,
            serde_json::to_string(profiles)?,
        ],
    )?;
    // What the write triggers watch: any later write to one invalidates this.
    transaction.execute(
        "DELETE FROM live_page_action_trust_deps WHERE action_id = ?1",
        [&action.id],
    )?;
    for (kind, id) in dependencies {
        transaction.execute(
            "INSERT OR IGNORE INTO live_page_action_trust_deps (action_id, dep_kind, dep_id)
             VALUES (?1, ?2, ?3)",
            params![action.id, kind, id],
        )?;
    }
    let trust = get(&transaction, &action.id)?
        .ok_or_else(|| anyhow::anyhow!("approval was not recorded"))?;
    transaction.commit()?;
    Ok(trust)
}

/// Withdraw an approval, valid or invalidated. Effective on the next click.
pub fn revoke(conn: &Connection, action_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM live_page_action_trusts WHERE action_id = ?1",
        [action_id],
    )? > 0)
}

/// Every current offer of the Page with its eligibility and approval, after
/// invalidating each approval that no longer matches.
pub fn list_for_page(
    conn: &Connection,
    live_page_id: &str,
) -> Result<Vec<LivePageActionTrustState>> {
    list_for_page_in(conn, live_page_id, ProfileView::Approved)
}

/// [`list_for_page`] against the repository profiles `profiles` says: the
/// panel passes fresh ones, so a changed profile invalidates an approval.
pub fn list_for_page_in(
    conn: &Connection,
    live_page_id: &str,
    profiles: ProfileView<'_>,
) -> Result<Vec<LivePageActionTrustState>> {
    let actions = crate::db::live_page_actions::list_for_live_page(conn, live_page_id)?;
    let mut states = Vec::with_capacity(actions.len());
    for action in actions {
        let current = evaluate_in(conn, &action, profiles)?;
        let mut trust = get(conn, &action.id)?;
        let active = match trust.as_mut() {
            Some(trust) => revalidate(conn, &action, trust, current.as_ref().map_err(|r| *r))?,
            None => false,
        };
        states.push(LivePageActionTrustState {
            action_id: action.id,
            action_ref: action.action_ref,
            target_name: action.target_name,
            fingerprint: current.as_ref().ok().cloned(),
            refusal: current.err(),
            trust,
            active,
        });
    }
    Ok(states)
}

/// Invalidate the approvals of a Page whose actions no longer match, including
/// blocks that left the Page. Runs in the caller's revision transaction.
pub fn revalidate_page(conn: &Connection, live_page_id: &str) -> Result<()> {
    let ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT action_id FROM live_page_action_trusts
             WHERE live_page_id = ?1 AND invalidated_at IS NULL",
        )?;
        let rows = statement.query_map([live_page_id], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for action_id in ids {
        let Some(action) = crate::db::live_page_actions::declaration(conn, &action_id)? else {
            continue;
        };
        let current = evaluate(conn, &action)?;
        if let Some(mut trust) = get(conn, &action_id)? {
            revalidate(conn, &action, &mut trust, current.as_ref().map_err(|r| *r))?;
        }
    }
    Ok(())
}

/// The gate of a launch without a card: a matching approval and both rate limits.
/// An invalidation it records is kept even though the launch is refused.
pub fn admit_launch(
    conn: &Connection,
    action: &LivePageAction,
    binding_key: &str,
) -> Result<std::result::Result<LivePageActionTrust, LivePageActionTrustRefusal>> {
    let Some(mut trust) = get(conn, &action.id)? else {
        return Ok(Err(LivePageActionTrustRefusal::NotTrusted));
    };
    let current = evaluate(conn, action)?;
    if !revalidate(conn, action, &mut trust, current.as_ref().map_err(|r| *r))? {
        return Ok(Err(trust
            .invalidated_reason
            .unwrap_or(LivePageActionTrustRefusal::NotTrusted)));
    }
    let now = Utc::now();
    let row_cutoff = (now - Duration::seconds(ROW_MIN_INTERVAL_SECS)).to_rfc3339();
    let recent_row: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_page_action_launches
         WHERE action_id = ?1 AND binding_key = ?2 AND trusted = 1
           AND julianday(created_at) > julianday(?3))",
        params![action.id, binding_key, row_cutoff],
        |row| row.get(0),
    )?;
    let window_cutoff = (now - Duration::seconds(ACTION_WINDOW_SECS)).to_rfc3339();
    let in_window: i64 = conn.query_row(
        "SELECT COUNT(*) FROM live_page_action_launches
         WHERE action_id = ?1 AND trusted = 1 AND julianday(created_at) > julianday(?2)",
        params![action.id, window_cutoff],
        |row| row.get(0),
    )?;
    if recent_row || in_window >= ACTION_MAX_PER_WINDOW {
        return Ok(Err(LivePageActionTrustRefusal::RateLimited));
    }
    Ok(Ok(trust))
}

/// The admission of a trusted launch's run, inside the transaction that then
/// inserts and pins it: `snapshot` is the definition that will execute and
/// `profiles` the repository profiles it will pin. It runs
/// only under the very approval its launch was claimed with, still matching.
pub fn admit_run(
    conn: &Connection,
    launch_id: &str,
    snapshot: &Workflow,
    profiles: &ProfileSnapshots,
) -> Result<std::result::Result<(), LivePageActionTrustRefusal>> {
    use LivePageActionTrustRefusal as Refusal;
    type Claimed = (
        String,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    );
    let claimed: Option<Claimed> = conn
        .query_row(
            "SELECT action_id, kind, target_id, project_id, values_json,
                    trust_approval_id, trust_fingerprint
             FROM live_page_action_launches WHERE id = ?1 AND trusted = 1",
            [launch_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((action_id, kind, target_id, project_id, values_json, approval_id, fingerprint)) =
        claimed
    else {
        return Ok(Err(Refusal::NotTrusted));
    };
    let Some(mut trust) = get(conn, &action_id)? else {
        return Ok(Err(Refusal::NotTrusted));
    };
    if trust.invalidated_at.is_some() {
        return Ok(Err(trust.invalidated_reason.unwrap_or(Refusal::NotTrusted)));
    }
    if approval_id.as_deref() != Some(trust.approval_id.as_str())
        || fingerprint.as_deref() != Some(trust.fingerprint.as_str())
    {
        return Ok(Err(Refusal::NotTrusted));
    }
    let Some(action) = crate::db::live_page_actions::declaration(conn, &action_id)? else {
        return Ok(Err(Refusal::NotTrusted));
    };
    // The claim runs what it copied: it must still be what the Page offers,
    // compared as a claim stores it (runtime values scrubbed).
    let mut offered = action.values.clone();
    super::kronn_action_engine::scrub_runtime_values(&mut offered);
    let claimed_values: Vec<super::discussion_actions::DiscussionActionValue> =
        serde_json::from_str(&values_json)?;
    let same_offer = action.kind.as_db_str() == kind
        && action.target_id == target_id
        && action.project_id == project_id
        && offered == claimed_values;
    if !same_offer {
        return Ok(Err(Refusal::Changed));
    }
    // `profiles` are the snapshots the run is about to pin, read just before.
    let current = evaluate_with(conn, &action, Some(snapshot), ProfileView::Fresh(profiles))?;
    if revalidate(
        conn,
        &action,
        &mut trust,
        current
            .as_ref()
            .map(|(fingerprint, _)| fingerprint)
            .map_err(|r| *r),
    )? {
        Ok(Ok(()))
    } else {
        Ok(Err(trust.invalidated_reason.unwrap_or(Refusal::NotTrusted)))
    }
}

/// The Page's approvals for an agent's read: which actions run without a card.
pub fn summary_for_page(conn: &Connection, live_page_id: &str) -> Result<serde_json::Value> {
    let states = list_for_page(conn, live_page_id)?;
    Ok(serde_json::json!(states
        .into_iter()
        .filter_map(|state| state.trust.map(|trust| serde_json::json!({
            "action_ref": state.action_ref,
            "active": state.active,
            "invalidated_reason": trust.invalidated_reason,
        })))
        .collect::<Vec<_>>()))
}

#[cfg(test)]
#[path = "live_page_action_trusts_tests.rs"]
pub(crate) mod tests;
