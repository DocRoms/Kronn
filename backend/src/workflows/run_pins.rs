//! Revision pinning (KT-1096): a run executes the workflow and the
//! dependencies it had when it started, whatever is edited meanwhile.
//!
//! The first execution of a run freezes its workflow plus every Quick Prompt,
//! Quick API, sub-workflow, skill, directive and profile it can load
//! (`workflow_run_pins`). Gate and quota resumes, restarts, rollback steps,
//! sub-workflow children and BatchQuickPrompt fan-out read those rows. A run
//! started before pinning existed has no rows: it is pinned at its next
//! execution, unless an agent changed its workflow since it started.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::core::project_profile::{project_key, ProfileSnapshot, ProfileSnapshots};
use crate::db::workflow_run_pins as rows;
use crate::models::{
    AgentProfile, Directive, QuickApi, QuickPrompt, Skill, StepType, Workflow, WorkflowRun,
};

pub const QUICK_PROMPT: &str = "quick_prompt";
pub const QUICK_API: &str = "quick_api";
pub const WORKFLOW: &str = "workflow";
pub const SKILL: &str = "skill";
pub const DIRECTIVE: &str = "directive";
pub const PROFILE: &str = "profile";
pub const RESOLUTION: &str = "resolution";
/// The repository profile a run read (KT-920), keyed by its project id.
pub const PROJECT_PROFILE: &str = "project_profile";

#[derive(Serialize, Deserialize)]
struct RunHeader {
    pinned_at: DateTime<Utc>,
    /// `None` for a BatchQuickPrompt run, which has no workflow of its own.
    #[serde(default)]
    workflow: Option<Workflow>,
    /// [`revision_fingerprint`] at pin time; `None` when inherited.
    #[serde(default)]
    fingerprint: Option<String>,
    /// Zone of the run's `{{time.now}}` (KT-1103); absent (pinned before
    /// zones existed) means UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timezone: Option<String>,
}

/// Pins `workflow` and its dependencies the first time `run` executes and
/// returns it; afterwards returns the pinned definition. `Ok(Err(reason))`
/// refuses a run whose workflow an agent changed after it was launched.
pub fn pin_or_load(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> Result<std::result::Result<Workflow, String>> {
    pin_or_load_with_profiles(conn, workflow, run, &ProfileSnapshots::new())
}

/// [`pin_or_load`] that also pins the repository profiles `profiles` holds
/// (resolved beforehand, outside any connection) and covers them in the
/// pinned fingerprint. A child records its own on the root run, first writer
/// wins, so every run of a tree reads one snapshot per project.
pub fn pin_or_load_with_profiles(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
    profiles: &ProfileSnapshots,
) -> Result<std::result::Result<Workflow, String>> {
    if let Some(header) = header(conn, &run.id)? {
        return Ok(Ok(header.workflow.unwrap_or_else(|| workflow.clone())));
    }
    // A run with no row is refused by the runner's claim; nothing to pin.
    let exists = conn
        .query_row(
            "SELECT 1 FROM workflow_runs WHERE id = ?1",
            params![run.id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Ok(workflow.clone()));
    }
    let tx = conn.unchecked_transaction()?;
    let pinned = pin_fresh(&tx, workflow, run, profiles)?;
    if pinned.is_ok() {
        tx.commit()?;
    }
    Ok(pinned)
}

/// Pins `run` inside the caller's transaction, right after the caller inserted
/// it: an admission check and the pin then see the same definitions (KT-1029).
pub fn pin_within(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> Result<std::result::Result<Workflow, String>> {
    pin_fresh(conn, workflow, run, &ProfileSnapshots::new())
}

/// [`pin_within`] that also pins the repository profiles the caller resolved
/// (and, for an admission, verified) before its transaction.
pub fn pin_within_with_profiles(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
    profiles: &ProfileSnapshots,
) -> Result<std::result::Result<Workflow, String>> {
    pin_fresh(conn, workflow, run, profiles)
}

fn pin_fresh(
    tx: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
    profiles: &ProfileSnapshots,
) -> Result<std::result::Result<Workflow, String>> {
    let inherited = match run.parent_run_id.as_deref() {
        Some(parent) if rows::has_pin(tx, parent)? => {
            rows::copy_dependencies(tx, parent, &run.id)?;
            true
        }
        _ => false,
    };
    let mut fingerprint = None;
    if !inherited {
        if let Some(reason) = changed_since(tx, &workflow.id, run.started_at)? {
            return Ok(Err(reason));
        }
        let project_id = run.project_id.as_deref().or(workflow.project_id.as_deref());
        let deps = Deps::gather(tx, workflow, project_id)?;
        deps.store(tx, &run.id)?;
        let covered = deps.profiles_of(profiles);
        for (key, snapshot) in &covered {
            rows::insert(
                tx,
                &run.id,
                PROJECT_PROFILE,
                key,
                &serde_json::to_string(snapshot)?,
            )?;
        }
        fingerprint = Some(fingerprint_of(workflow, &deps, &covered)?);
    } else {
        let owner = pin_owner(tx, run.parent_run_id.as_deref().unwrap_or_default())?;
        for (key, snapshot) in profiles {
            rows::insert(
                tx,
                &owner,
                PROJECT_PROFILE,
                key,
                &serde_json::to_string(snapshot)?,
            )?;
        }
    }
    let timezone = match run.parent_run_id.as_deref().filter(|_| inherited) {
        Some(parent) => inherited_timezone(tx, parent)?,
        // A run that already executed steps before it had a pin started
        // under UTC templates: it keeps them.
        None if !run.step_results.is_empty() => chrono_tz::UTC,
        None => crate::core::timezone::current(),
    };
    let header = RunHeader {
        pinned_at: Utc::now(),
        workflow: Some(workflow.clone()),
        fingerprint,
        timezone: Some(timezone.name().to_string()),
    };
    rows::insert(
        tx,
        &run.id,
        rows::RUN_KIND,
        "",
        &serde_json::to_string(&header)?,
    )?;
    Ok(Ok(workflow.clone()))
}

/// The definition `run` pinned, if it has one.
pub fn pinned_workflow(conn: &Connection, run_id: &str) -> Result<Option<Workflow>> {
    Ok(header(conn, run_id)?.and_then(|header| header.workflow))
}

/// Applies the structured `ref:` resolutions `run_id` recorded for this
/// workflow; `false` when it recorded none (resolve live).
pub fn apply_pinned_structured(
    conn: &Connection,
    run_id: &str,
    workflow: &mut Workflow,
) -> Result<bool> {
    let Some(resolution) = resolution(conn, run_id, &workflow.id)? else {
        return Ok(false);
    };
    crate::core::resource_refs::apply_structured_reference_map(workflow, &resolution.structured);
    Ok(true)
}

/// The `{{ref:…}}` template values `run_id` recorded for this workflow.
pub fn pinned_template_references(
    conn: &Connection,
    run_id: &str,
    workflow_id: &str,
) -> Result<Option<std::collections::HashMap<String, String>>> {
    Ok(resolution(conn, run_id, workflow_id)?
        .map(|resolution| resolution.template.into_iter().collect()))
}

fn resolution(conn: &Connection, run_id: &str, workflow_id: &str) -> Result<Option<Resolution>> {
    match rows::get(conn, run_id, RESOLUTION, workflow_id)? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

/// The root of `run_id`'s pinned run tree: the farthest pinned ancestor.
/// Profiles are shared there, so two children never read different ones.
fn pin_owner(conn: &Connection, run_id: &str) -> Result<String> {
    let mut owner = run_id.to_string();
    for _ in 0..64 {
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_run_id FROM workflow_runs WHERE id = ?1",
                params![owner],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        match parent {
            Some(parent) if rows::has_pin(conn, &parent)? => owner = parent,
            _ => break,
        }
    }
    Ok(owner)
}

/// The repository profile pinned for `project_id` in `run_id`'s tree: the
/// run's own row, else its root's.
pub fn pinned_project_profile(
    conn: &Connection,
    run_id: &str,
    project_id: Option<&str>,
) -> Result<Option<ProfileSnapshot>> {
    let key = project_key(project_id);
    for holder in [run_id.to_string(), pin_owner(conn, run_id)?] {
        if let Some(json) = rows::get(conn, &holder, PROJECT_PROFILE, &key)? {
            return Ok(Some(serde_json::from_str(&json)?));
        }
    }
    Ok(None)
}

/// Records a profile read after the pin on the tree's root, first writer
/// wins, and returns what the tree holds. An unpinned run keeps `read`.
pub fn record_project_profile(
    conn: &Connection,
    run_id: &str,
    project_id: Option<&str>,
    read: ProfileSnapshot,
) -> Result<ProfileSnapshot> {
    if !rows::has_pin(conn, run_id)? {
        return Ok(read);
    }
    let owner = pin_owner(conn, run_id)?;
    rows::insert(
        conn,
        &owner,
        PROJECT_PROFILE,
        &project_key(project_id),
        &serde_json::to_string(&read)?,
    )?;
    Ok(pinned_project_profile(conn, run_id, project_id)?.unwrap_or(read))
}

/// Every project `workflow` (and its sub-workflows) executes in when run for
/// `project_id`, as profile keys.
pub fn profile_projects(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
) -> Result<BTreeSet<String>> {
    let mut deps = Deps::default();
    collect(conn, workflow, project_id, &mut deps)?;
    Ok(deps.projects)
}

/// The projects whose repository profile the run of `workflow` (and of its
/// sub-workflows) for `project_id` will read, minus those `parent_run_id`'s
/// tree already pinned: what to resolve before [`pin_or_load_with_profiles`].
pub fn profile_projects_to_resolve(
    conn: &Connection,
    workflow: &Workflow,
    run: &WorkflowRun,
) -> Result<BTreeSet<String>> {
    if rows::has_pin(conn, &run.id)? {
        return Ok(BTreeSet::new());
    }
    let project_id = run.project_id.as_deref().or(workflow.project_id.as_deref());
    let mut wanted = profile_projects(conn, workflow, project_id)?;
    if let Some(parent) = run.parent_run_id.as_deref().filter(|p| !p.is_empty()) {
        if rows::has_pin(conn, parent)? {
            let mut pinned = BTreeSet::new();
            for key in &wanted {
                let id = (!key.is_empty()).then_some(key.as_str());
                if pinned_project_profile(conn, parent, id)?.is_some() {
                    pinned.insert(key.clone());
                }
            }
            wanted.retain(|key| !pinned.contains(key));
        }
    }
    Ok(wanted)
}

/// The [`revision_fingerprint`] a top-level run pinned when it started.
pub fn pinned_fingerprint(conn: &Connection, run_id: &str) -> Result<Option<String>> {
    Ok(header(conn, run_id)?.and_then(|header| header.fingerprint))
}

/// The zone `run_id`'s templates render in: the one it pinned, UTC for a pin
/// older than zones, `None` while it has no pin yet.
pub fn pinned_timezone(conn: &Connection, run_id: &str) -> Result<Option<chrono_tz::Tz>> {
    Ok(header(conn, run_id)?.map(|header| zone_of(&header)))
}

fn zone_of(header: &RunHeader) -> chrono_tz::Tz {
    header
        .timezone
        .as_deref()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// A child renders in its parent's pinned zone.
fn inherited_timezone(conn: &Connection, parent_run_id: &str) -> Result<chrono_tz::Tz> {
    Ok(header(conn, parent_run_id)?
        .map(|header| zone_of(&header))
        .unwrap_or(chrono_tz::UTC))
}

/// A BatchQuickPrompt run reads its chain prompts from its parent's pin.
pub fn inherit_for_batch(conn: &Connection, parent_run_id: &str, batch_run_id: &str) -> Result<()> {
    if !rows::has_pin(conn, parent_run_id)? {
        return Ok(());
    }
    rows::copy_dependencies(conn, parent_run_id, batch_run_id)?;
    let header = RunHeader {
        pinned_at: Utc::now(),
        workflow: None,
        fingerprint: None,
        timezone: Some(inherited_timezone(conn, parent_run_id)?.name().to_string()),
    };
    rows::insert(
        conn,
        batch_run_id,
        rows::RUN_KIND,
        "",
        &serde_json::to_string(&header)?,
    )
}

pub fn purge(conn: &Connection, run_id: &str) -> Result<()> {
    rows::purge(conn, run_id)?;
    Ok(())
}

/// The Quick Prompt a run loads: its pinned revision when the run has a pin,
/// else the live one. `Ok(Err(_))` explains why a pinned run cannot load it.
pub fn quick_prompt_for(
    conn: &Connection,
    run_id: Option<&str>,
    id: &str,
) -> Result<std::result::Result<Option<QuickPrompt>, String>> {
    pinned_or_live(
        conn,
        run_id,
        QUICK_PROMPT,
        "Quick Prompt",
        id,
        crate::db::quick_prompts::get_quick_prompt,
    )
}

/// A chain prompt of a batch discussion: pinned when its batch run belongs to
/// a pinned workflow run.
pub fn chain_prompt_for(
    conn: &Connection,
    discussion_id: &str,
    id: &str,
) -> Result<std::result::Result<Option<QuickPrompt>, String>> {
    let batch_run: Option<String> = conn
        .query_row(
            "SELECT workflow_run_id FROM discussions WHERE id = ?1",
            params![discussion_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    quick_prompt_for(conn, batch_run.as_deref(), id)
}

/// Same rule as [`quick_prompt_for`] for a Quick API.
pub fn quick_api_for(
    conn: &Connection,
    run_id: Option<&str>,
    id: &str,
) -> Result<std::result::Result<Option<QuickApi>, String>> {
    pinned_or_live(conn, run_id, QUICK_API, "Quick API", id, |conn, id| {
        crate::db::quick_apis::get_quick_api(conn, id)
    })
}

/// The sub-workflow a parent run starts. Its `enabled` says whether the
/// parent may run it: enabled when pinned, and not switched off by a human
/// since (an agent's edit disables the live row, never the pinned revision).
pub fn sub_workflow_for(
    conn: &Connection,
    parent_run_id: &str,
    id: &str,
) -> Result<std::result::Result<Option<Workflow>, String>> {
    let loaded = pinned_or_live(
        conn,
        Some(parent_run_id),
        WORKFLOW,
        "Sub-workflow",
        id,
        crate::db::workflows::get_workflow,
    )?;
    let Ok(Some(mut workflow)) = loaded else {
        return Ok(loaded);
    };
    if workflow.enabled && disabled_by_human(conn, id)? {
        workflow.enabled = false;
    }
    Ok(Ok(Some(workflow)))
}

/// Seeds the in-memory skill/directive/profile snapshots of `run_id` with its
/// pinned revisions, so a resumed run renders what it started with.
pub fn seed_resource_snapshots(conn: &Connection, run_id: &str) -> Result<()> {
    seed_resource_snapshots_as(conn, run_id, run_id).map(|_| ())
}

/// Seeds the snapshot `key` with `run_id`'s pinned skills, directives and
/// profiles; `false` when the run has no pin.
pub fn seed_resource_snapshots_as(conn: &Connection, run_id: &str, key: &str) -> Result<bool> {
    if !rows::has_pin(conn, run_id)? {
        return Ok(false);
    }
    for (id, json) in rows::list_kind(conn, run_id, SKILL)? {
        crate::core::skills::pin_skill_snapshot(key, &id, serde_json::from_str(&json)?);
    }
    for (id, json) in rows::list_kind(conn, run_id, DIRECTIVE)? {
        crate::core::directives::pin_directive_snapshot(key, &id, serde_json::from_str(&json)?);
    }
    for (id, json) in rows::list_kind(conn, run_id, PROFILE)? {
        crate::core::profiles::pin_profile_snapshot(key, &id, serde_json::from_str(&json)?);
    }
    Ok(true)
}

/// The one revision identity of a workflow and everything it executes: its
/// steps plus the transitive Quick Prompts, Quick APIs, sub-workflows, skills,
/// directives and profiles. Any change to one of them changes the
/// fingerprint; enabling, pinning, timestamps and a workflow's run retention
/// do not. An approval tied to
/// this value (a run's pin, a Live Page action) must not outlive it.
///
/// It does not cover the repository profiles (`kronn/project.toml`) the run
/// reads, which can change what an Exec step runs. A caller that needs an
/// approval-grade identity resolves the profiles first, outside any
/// connection ([`profile_projects_to_resolve`], then
/// `core::project_profile::snapshot` per project), and calls
/// [`revision_fingerprint_with_profiles`]; the run pin does the same.
pub fn revision_fingerprint(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
) -> Result<String> {
    revision_fingerprint_with_profiles(conn, workflow, project_id, &ProfileSnapshots::new())
}

/// [`revision_fingerprint`] that also covers the repository profiles of the
/// projects the run tree reads, from snapshots resolved beforehand. Pass the
/// same snapshots the run pins so the approval and the run agree.
pub fn revision_fingerprint_with_profiles(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
    profiles: &ProfileSnapshots,
) -> Result<String> {
    let deps = Deps::gather(conn, workflow, project_id)?;
    let covered = deps.profiles_of(profiles);
    fingerprint_of(workflow, &deps, &covered)
}

/// Hashes exactly the closure `deps` materialised, never a second read.
fn fingerprint_of(workflow: &Workflow, deps: &Deps, profiles: &ProfileSnapshots) -> Result<String> {
    let mut identity = serde_json::json!({
        "workflow": workflow_revision_content(workflow)?,
        "quick_prompts": deps.prompts.values().map(revision_content).collect::<Result<Vec<_>>>()?,
        "quick_apis": deps.apis.values().map(revision_content).collect::<Result<Vec<_>>>()?,
        "workflows": deps.workflows.values().map(workflow_revision_content).collect::<Result<Vec<_>>>()?,
        "resolutions": deps.resolutions,
        "skills": deps.skills.values().collect::<Vec<_>>(),
        "directives": deps.directives.values().collect::<Vec<_>>(),
        "profiles": deps.profiles.values().collect::<Vec<_>>(),
    });
    // Absent when no profile was resolved, so older fingerprints stay equal.
    if !profiles.is_empty() {
        identity["project_profiles"] = profiles
            .iter()
            .map(|(key, snapshot)| (key.clone(), snapshot.identity()))
            .collect::<serde_json::Map<_, _>>()
            .into();
    }
    let canonical = serde_json::to_vec(&canonical(identity))?;
    Ok(crate::core::repository_resources::sha256(&canonical))
}

/// What a revision is made of: the resource without its switches and dates.
fn revision_content<T: Serialize>(resource: &T) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(resource)?;
    if let Some(object) = value.as_object_mut() {
        for key in ["enabled", "pinned", "created_at", "updated_at"] {
            object.remove(key);
        }
    }
    Ok(value)
}

/// A workflow's revision also leaves out its run retention (KT-1100): it only
/// decides when finished runs are purged, never what a run executes.
fn workflow_revision_content(workflow: &Workflow) -> Result<serde_json::Value> {
    let mut value = revision_content(workflow)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("retention");
    }
    Ok(value)
}

/// Sorted keys at every level, so map order never changes the hash.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> = map
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

fn header(conn: &Connection, run_id: &str) -> Result<Option<RunHeader>> {
    match rows::get(conn, run_id, rows::RUN_KIND, "")? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

fn pinned_or_live<T: DeserializeOwned>(
    conn: &Connection,
    run_id: Option<&str>,
    kind: &str,
    label: &str,
    id: &str,
    live: impl Fn(&Connection, &str) -> Result<Option<T>>,
) -> Result<std::result::Result<Option<T>, String>> {
    let Some(run_id) = run_id.filter(|run_id| rows::has_pin(conn, run_id).unwrap_or(false)) else {
        return Ok(Ok(live(conn, id)?));
    };
    let Some(json) = rows::get(conn, run_id, kind, id)? else {
        return Ok(Err(format!(
            "{label} `{id}` is not part of the revision this run started with: launch a new run to use it."
        )));
    };
    // The pinned revision runs only while the resource still exists.
    if live(conn, id)?.is_none() {
        return Ok(Err(format!(
            "{label} `{id}` was deleted while this run was in progress: the run cannot load it."
        )));
    }
    Ok(Ok(Some(serde_json::from_str(&json)?)))
}

/// An agent's change (or an import) disabled the workflow after `since`.
fn changed_since(
    conn: &Connection,
    workflow_id: &str,
    since: DateTime<Utc>,
) -> Result<Option<String>> {
    let row: Option<(i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT enabled, disabled_at, disabled_summary FROM workflows WHERE id = ?1",
            params![workflow_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((0, Some(at), summary)) = row else {
        return Ok(None);
    };
    let Ok(at) = DateTime::parse_from_rfc3339(&at) else {
        return Ok(None);
    };
    if at.with_timezone(&Utc) <= since {
        return Ok(None);
    }
    let summary = summary
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "changed".into());
    Ok(Some(format!(
        "This run's workflow was disabled after the run was launched ({summary}): its approved revision was not recorded, so the run does not continue. Review the change, enable the workflow again and launch a new run."
    )))
}

fn disabled_by_human(conn: &Connection, id: &str) -> Result<bool> {
    let row: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT enabled, disabled_at FROM workflows WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(matches!(row, Some((0, None))))
}

/// How a run resolved one workflow's references when it was pinned: its
/// structured `ref:` fields and its `{{ref:…}}` template values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Resolution {
    structured: BTreeMap<String, String>,
    template: BTreeMap<String, String>,
}

/// Everything a pin holds, materialised once: what is stored and what is
/// hashed are the same objects.
#[derive(Default)]
struct Deps {
    prompts: BTreeMap<String, QuickPrompt>,
    apis: BTreeMap<String, QuickApi>,
    workflows: BTreeMap<String, Workflow>,
    resolutions: BTreeMap<String, Resolution>,
    skill_ids: BTreeSet<String>,
    /// Repository skill ids by the project of the step naming them (`""` for
    /// none): a sub-workflow of another project loads that project's skills.
    repository_skill_ids: BTreeMap<String, BTreeSet<String>>,
    directive_ids: BTreeSet<String>,
    profile_ids: BTreeSet<String>,
    skills: BTreeMap<String, Skill>,
    directives: BTreeMap<String, Directive>,
    profiles: BTreeMap<String, AgentProfile>,
    /// Projects the run tree executes in (`""` for none), for their profiles.
    projects: BTreeSet<String>,
}

impl Deps {
    fn gather(conn: &Connection, workflow: &Workflow, project_id: Option<&str>) -> Result<Self> {
        let mut deps = Deps::default();
        collect(conn, workflow, project_id, &mut deps)?;
        let skill_ids: Vec<String> = deps.skill_ids.iter().cloned().collect();
        for skill in crate::core::skills::get_skills_by_ids(&skill_ids) {
            deps.skills.insert(skill.id.clone(), skill);
        }
        // Repository skills enter the pin as read from the default branch now,
        // each in the project of the step naming it; one that cannot be read is
        // left out and stops its step by name.
        for (context, ids) in &deps.repository_skill_ids {
            let ids: Vec<String> = ids.iter().cloned().collect();
            let repository = crate::api::projects::used_skills::resolve_repository_skills_from(
                conn,
                (!context.is_empty()).then_some(context.as_str()),
                &ids,
                crate::api::projects::used_skills::RepositorySkillSource::DefaultBranch,
            )?;
            for skill in repository.resolved {
                deps.skills.insert(skill.id.clone(), skill);
            }
        }
        let directive_ids: Vec<String> = deps.directive_ids.iter().cloned().collect();
        for directive in crate::core::directives::get_directives_by_ids(&directive_ids) {
            deps.directives.insert(directive.id.clone(), directive);
        }
        for id in &deps.profile_ids {
            if let Some(profile) = crate::core::profiles::get_profile(id) {
                deps.profiles.insert(id.clone(), profile);
            }
        }
        Ok(deps)
    }

    /// The snapshots of `profiles` for projects this closure executes in.
    fn profiles_of(&self, profiles: &ProfileSnapshots) -> ProfileSnapshots {
        profiles
            .iter()
            .filter(|(key, _)| self.projects.contains(*key))
            .map(|(key, snapshot)| (key.clone(), snapshot.clone()))
            .collect()
    }

    fn bindings(
        &mut self,
        project_id: Option<&str>,
        skills: &[String],
        profiles: &[String],
        directives: &[String],
    ) {
        self.skill_ids.extend(skills.iter().cloned());
        let repository = skills.iter().filter(|id| {
            crate::api::projects::used_skills::parse_repository_skill_id(id).is_some()
        });
        self.repository_skill_ids
            .entry(project_id.unwrap_or_default().to_string())
            .or_default()
            .extend(repository.cloned());
        self.profile_ids.extend(profiles.iter().cloned());
        self.directive_ids.extend(directives.iter().cloned());
    }

    fn store(&self, conn: &Connection, run_id: &str) -> Result<()> {
        fn put<T: Serialize>(
            conn: &Connection,
            run_id: &str,
            kind: &str,
            items: &BTreeMap<String, T>,
        ) -> Result<()> {
            for (id, item) in items {
                rows::insert(conn, run_id, kind, id, &serde_json::to_string(item)?)?;
            }
            Ok(())
        }
        put(conn, run_id, QUICK_PROMPT, &self.prompts)?;
        put(conn, run_id, QUICK_API, &self.apis)?;
        put(conn, run_id, WORKFLOW, &self.workflows)?;
        put(conn, run_id, RESOLUTION, &self.resolutions)?;
        put(conn, run_id, SKILL, &self.skills)?;
        put(conn, run_id, DIRECTIVE, &self.directives)?;
        put(conn, run_id, PROFILE, &self.profiles)
    }
}

/// Every dependency `workflow` can load, sub-workflows included, with its
/// references resolved the way the run resolves them, and recorded.
fn collect(
    conn: &Connection,
    workflow: &Workflow,
    project_id: Option<&str>,
    deps: &mut Deps,
) -> Result<()> {
    deps.projects.insert(project_key(project_id));
    let structured =
        crate::core::resource_refs::structured_reference_map(conn, workflow, project_id)?;
    let mut resolved = workflow.clone();
    crate::core::resource_refs::apply_structured_reference_map(&mut resolved, &structured);
    let template =
        crate::core::resource_refs::resolve_template_references(conn, &resolved, project_id)?
            .into_iter()
            .collect();
    deps.resolutions
        .entry(workflow.id.clone())
        .or_insert(Resolution {
            structured,
            template,
        });
    for step in resolved.steps.iter().chain(resolved.on_failure.iter()) {
        deps.bindings(
            project_id,
            &step.skill_ids,
            &step.profile_ids,
            &step.directive_ids,
        );
        let prompt_ids = step
            .quick_prompt_id
            .iter()
            .chain(step.batch_quick_prompt_id.iter())
            .chain(step.batch_chain_prompt_ids.iter());
        for id in prompt_ids {
            if deps.prompts.contains_key(id) {
                continue;
            }
            if let Some(prompt) = crate::db::quick_prompts::get_quick_prompt(conn, id)? {
                deps.bindings(
                    project_id,
                    &prompt.skill_ids,
                    &prompt.profile_ids,
                    &prompt.directive_ids,
                );
                deps.prompts.insert(id.clone(), prompt);
            }
        }
        let collected = step
            .collect_api_data
            .iter()
            .flat_map(|config| config.sources.iter())
            .map(|source| &source.quick_api_id);
        for id in step.quick_api_id.iter().chain(collected) {
            if id.trim().is_empty() || deps.apis.contains_key(id) {
                continue;
            }
            if let Some(api) = crate::db::quick_apis::get_quick_api(conn, id)? {
                deps.bindings(project_id, &[], &api.profile_ids, &api.directive_ids);
                deps.apis.insert(id.clone(), api);
            }
        }
        if step.step_type != StepType::SubWorkflow {
            continue;
        }
        let Some(target) = step.sub_workflow_id.as_deref().map(str::trim) else {
            continue;
        };
        if target.is_empty() || deps.workflows.contains_key(target) {
            continue;
        }
        if let Some(child) = crate::db::workflows::get_workflow(conn, target)? {
            let child_project = super::sub_workflow_step::child_project_id(&child, project_id);
            deps.workflows.insert(target.to_string(), child.clone());
            collect(conn, &child, child_project.as_deref(), deps)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::{
        AgentType, ModelTier, RunStatus, WorkflowSafety, WorkflowStep, WorkflowTrigger,
    };

    fn workflow(id: &str, steps: Vec<WorkflowStep>) -> Workflow {
        Workflow {
            retention: None,
            project_scope: None,
            pinned: false,
            id: id.into(),
            name: id.into(),
            project_id: None,
            trigger: WorkflowTrigger::Manual,
            steps,
            actions: vec![],
            safety: WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            concurrency_key: None,
            guards: None,
            artifacts: Default::default(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn run(id: &str, workflow_id: &str) -> WorkflowRun {
        serde_json::from_value(serde_json::json!({
            "id": id, "workflow_id": workflow_id, "status": "Pending",
            "step_results": [], "tokens_used": 0, "started_at": Utc::now() - chrono::Duration::seconds(5)
        }))
        .unwrap()
    }

    fn prompt(id: &str, template: &str) -> QuickPrompt {
        QuickPrompt {
            id: id.into(),
            name: id.into(),
            icon: "P".into(),
            prompt_template: template.into(),
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
            pinned: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn api(id: &str, path: &str) -> QuickApi {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "icon": "A", "project_id": null,
            "api_plugin_slug": "plugin", "api_config_id": "config", "api_endpoint_path": path,
            "variables": [], "created_at": Utc::now(), "updated_at": Utc::now()
        }))
        .unwrap()
    }

    /// An Agent step on `qp-1`, an ApiCall step on `qa-1`, a sub-workflow
    /// `child` whose own step uses `qp-child`; run `run-1` launched, not pinned.
    async fn seeded() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            crate::db::quick_prompts::insert_quick_prompt(
                conn,
                &prompt("qp-1", "APPROVED prompt"),
            )?;
            crate::db::quick_prompts::insert_quick_prompt(
                conn,
                &prompt("qp-child", "APPROVED child"),
            )?;
            crate::db::quick_apis::insert_quick_api(conn, &api("qa-1", "/approved"))?;
            let child = workflow(
                "child",
                vec![WorkflowStep {
                    name: "child-agent".into(),
                    quick_prompt_id: Some("qp-child".into()),
                    ..WorkflowStep::default()
                }],
            );
            crate::db::workflows::insert_workflow(conn, &child)?;
            let parent = workflow(
                "parent",
                vec![
                    WorkflowStep {
                        name: "agent".into(),
                        quick_prompt_id: Some("qp-1".into()),
                        ..WorkflowStep::default()
                    },
                    WorkflowStep {
                        name: "call".into(),
                        step_type: StepType::ApiCall,
                        quick_api_id: Some("qa-1".into()),
                        ..WorkflowStep::default()
                    },
                    WorkflowStep {
                        name: "sub".into(),
                        step_type: StepType::SubWorkflow,
                        sub_workflow_id: Some("child".into()),
                        ..WorkflowStep::default()
                    },
                ],
            );
            crate::db::workflows::insert_workflow(conn, &parent)?;
            crate::db::workflows::insert_run(conn, &run("run-1", "parent"))?;
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    async fn pin(db: &Database, run_id: &str) -> std::result::Result<Workflow, String> {
        let run_id = run_id.to_string();
        db.with_conn(move |conn| {
            let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
            let run = crate::db::workflows::get_run(conn, &run_id)?.unwrap();
            pin_or_load(conn, &parent, &run)
        })
        .await
        .unwrap()
    }

    async fn agent_edits(db: &Database) {
        db.with_conn(|conn| {
            let mut edited = prompt("qp-1", "INJECTED prompt");
            edited.updated_at = Utc::now();
            crate::db::quick_prompts::update_quick_prompt_invalidating(
                conn,
                &edited,
                Some("codex"),
            )?;
            let edited = prompt("qp-child", "INJECTED child");
            crate::db::quick_prompts::update_quick_prompt_invalidating(
                conn,
                &edited,
                Some("codex"),
            )?;
            crate::db::quick_apis::update_quick_api_invalidating(
                conn,
                &api("qa-1", "/injected"),
                Some("codex"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    fn agent_step(qp: &str) -> WorkflowStep {
        WorkflowStep {
            name: "agent".into(),
            quick_prompt_id: Some(qp.into()),
            ..WorkflowStep::default()
        }
    }

    #[tokio::test]
    async fn an_agent_edit_during_the_run_never_reaches_its_steps() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        agent_edits(&db).await;

        let mut step = agent_step("qp-1");
        crate::workflows::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut step,
            &db,
            Some("run-1"),
        )
        .await
        .unwrap();
        assert_eq!(step.prompt_template, "APPROVED prompt");

        let mut call = WorkflowStep {
            step_type: StepType::ApiCall,
            quick_api_id: Some("qa-1".into()),
            ..WorkflowStep::default()
        };
        crate::workflows::quick_api_hydrate::hydrate_step_from_quick_api(
            &mut call,
            &db,
            Some("run-1"),
        )
        .await
        .unwrap();
        assert_eq!(call.api_endpoint_path.as_deref(), Some("/approved"));

        // Outside the run, the live revision.
        let mut live = agent_step("qp-1");
        crate::workflows::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut live, &db, None,
        )
        .await
        .unwrap();
        assert_eq!(live.prompt_template, "INJECTED prompt");
    }

    #[tokio::test]
    async fn a_child_run_inherits_its_parents_pinned_dependencies() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        db.with_conn(|conn| {
            let mut child = crate::db::workflows::get_workflow(conn, "child")?.unwrap();
            child.steps[0].prompt_template = "INJECTED inline".into();
            crate::db::workflows::update_workflow_as_agent(conn, &child)?;
            Ok(())
        })
        .await
        .unwrap();
        agent_edits(&db).await;

        let pinned_child = db
            .with_conn(|conn| sub_workflow_for(conn, "run-1", "child"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(pinned_child.steps[0].prompt_template.is_empty());
        assert!(
            pinned_child.enabled,
            "an agent's disable does not stop the pinned child"
        );

        let mut child_run = run("child-run", "child");
        child_run.parent_run_id = Some("run-1".into());
        let child_def = pinned_child.clone();
        let stored = child_run.clone();
        db.with_conn(move |conn| {
            crate::db::workflows::insert_run(conn, &stored)?;
            pin_or_load(conn, &child_def, &child_run)
        })
        .await
        .unwrap()
        .expect("a child pins from its parent");

        let mut step = agent_step("qp-child");
        crate::workflows::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut step,
            &db,
            Some("child-run"),
        )
        .await
        .unwrap();
        assert_eq!(step.prompt_template, "APPROVED child");
    }

    #[tokio::test]
    async fn a_dependency_deleted_mid_run_fails_with_a_clear_message() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        db.with_conn(|conn| {
            crate::db::quick_prompts::delete_quick_prompt(conn, "qp-1")?;
            crate::db::workflows::delete_workflow(conn, "child")
        })
        .await
        .unwrap();

        let mut step = agent_step("qp-1");
        let error = crate::workflows::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut step,
            &db,
            Some("run-1"),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains("was deleted while this run was in progress"),
            "{error}"
        );
        let child = db
            .with_conn(|conn| sub_workflow_for(conn, "run-1", "child"))
            .await
            .unwrap();
        assert!(
            child.as_ref().unwrap_err().contains("was deleted"),
            "{child:?}"
        );
    }

    #[tokio::test]
    async fn a_dependency_absent_from_the_pin_is_refused() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        db.with_conn(|conn| {
            crate::db::quick_prompts::insert_quick_prompt(conn, &prompt("qp-new", "new"))
        })
        .await
        .unwrap();
        let mut step = agent_step("qp-new");
        let error = crate::workflows::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut step,
            &db,
            Some("run-1"),
        )
        .await
        .unwrap_err();
        assert!(error.contains("not part of the revision"), "{error}");
    }

    #[tokio::test]
    async fn the_pin_survives_a_restart_and_a_second_pin_changes_nothing() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        db.with_conn(|conn| {
            let mut parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
            parent.steps[0].prompt_template = "INJECTED inline".into();
            crate::db::workflows::update_workflow(conn, &parent)?;
            Ok(())
        })
        .await
        .unwrap();
        // A resume passes the latest definition: the pinned one comes back.
        let resumed = pin(&db, "run-1").await.expect("still pinned");
        assert!(resumed.steps[0].prompt_template.is_empty());
    }

    #[tokio::test]
    async fn a_run_whose_workflow_an_agent_changed_after_launch_is_not_pinned() {
        let db = seeded().await;
        agent_edits(&db).await;
        let error = pin(&db, "run-1").await.unwrap_err();
        assert!(
            error.contains("disabled after the run was launched"),
            "{error}"
        );
        let pinned = db
            .with_conn(|conn| rows::has_pin(conn, "run-1"))
            .await
            .unwrap();
        assert!(!pinned, "a refused pin leaves nothing behind");
    }

    #[tokio::test]
    async fn a_batch_run_reads_its_parents_pinned_prompts_and_purge_drops_them() {
        let db = seeded().await;
        pin(&db, "run-1").await.expect("pinned");
        db.with_conn(|conn| {
            let mut batch = run("batch-1", "parent");
            batch.run_type = "batch".into();
            batch.parent_run_id = Some("run-1".into());
            batch.status = RunStatus::Running;
            crate::db::workflows::insert_run(conn, &batch)?;
            inherit_for_batch(conn, "run-1", "batch-1")
        })
        .await
        .unwrap();
        agent_edits(&db).await;
        let prompt = db
            .with_conn(|conn| quick_prompt_for(conn, Some("batch-1"), "qp-1"))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(prompt.prompt_template, "APPROVED prompt");

        db.with_conn(|conn| purge(conn, "run-1")).await.unwrap();
        let parent_pinned = db
            .with_conn(|conn| rows::has_pin(conn, "run-1"))
            .await
            .unwrap();
        assert!(!parent_pinned);
        let batch_pinned = db
            .with_conn(|conn| rows::has_pin(conn, "batch-1"))
            .await
            .unwrap();
        assert!(batch_pinned, "a fire-and-forget batch keeps its own copy");
    }

    /// What is stored and hashed is the closure gathered once: a skill edited
    /// between the gathering and the store changes neither.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_pin_stores_and_hashes_the_closure_it_gathered() {
        let _data_dir = crate::core::config::TestDataDir::new();
        let skill = crate::core::skills::save_custom_skill(
            "Barrier Skill",
            "desc",
            "S",
            &crate::models::SkillCategory::Domain,
            "GATHERED-BODY",
            None,
            None,
            None,
        )
        .unwrap();
        let db = seeded().await;
        let pinned_skill = skill.clone();
        let (before, after, stored) = db
            .with_conn(move |conn| {
                let mut parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                parent.steps[0].skill_ids = vec![pinned_skill.clone()];
                let deps = Deps::gather(conn, &parent, None)?;
                let before = fingerprint_of(&parent, &deps, &ProfileSnapshots::new())?;
                // The barrier: the catalog changes after the gathering.
                crate::core::skills::update_custom_skill(
                    &pinned_skill,
                    "Barrier Skill",
                    "desc",
                    "S",
                    &crate::models::SkillCategory::Domain,
                    "LATER-BODY",
                    None,
                    None,
                    None,
                )
                .map_err(anyhow::Error::msg)?;
                deps.store(conn, "run-1")?;
                let after = fingerprint_of(&parent, &deps, &ProfileSnapshots::new())?;
                let stored = rows::get(conn, "run-1", SKILL, &pinned_skill)?.unwrap();
                Ok((before, after, stored))
            })
            .await
            .unwrap();
        let _ = crate::core::skills::delete_custom_skill(&skill);
        assert_eq!(before, after);
        assert!(stored.contains("GATHERED-BODY"), "{stored}");
        assert!(!stored.contains("LATER-BODY"), "{stored}");
    }

    fn test_project(id: &str, path: &str) -> crate::models::Project {
        let now = Utc::now();
        crate::models::Project {
            id: id.into(),
            name: id.into(),
            path: path.into(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: Default::default(),
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            workspace: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// A git repository whose default branch commits `body` as the
    /// `block-migration` skill.
    fn repository_with_skill(body: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = crate::core::cmd::git_cmd()
                .args(args)
                .current_dir(root.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        let dir = root.path().join(".agents/skills/block-migration");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: block-migration\n---\n{body}\n"),
        )
        .unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "--", "."]);
        git(&["commit", "-q", "-m", "skill"]);
        root
    }

    /// A sub-workflow of another project loads that project's repository
    /// skills; its parent's project still cannot load them directly.
    #[tokio::test]
    async fn a_sub_workflow_of_another_project_pins_that_project_s_repository_skill() {
        let repo_b = repository_with_skill("B-BODY");
        let db = seeded().await;
        let path_b = repo_b.path().display().to_string();
        let id = "repository:pB:block-migration";
        db.with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &test_project("pA", "/nonexistent/a"))?;
            crate::db::projects::insert_project(conn, &test_project("pB", &path_b))?;
            crate::db::project_skill_references::upsert(
                conn,
                "pB",
                "block-migration",
                ".agents/skills/block-migration/SKILL.md",
                "Block migration",
                "2026-10-09T00:00:00Z",
            )?;
            let mut child = crate::db::workflows::get_workflow(conn, "child")?.unwrap();
            child.project_id = Some("pB".into());
            child.steps[0].skill_ids = vec![id.into()];
            crate::db::workflows::update_workflow(conn, &child)?;
            let mut parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
            parent.project_id = Some("pA".into());
            parent.steps[0].skill_ids = vec![id.into()];
            crate::db::workflows::update_workflow(conn, &parent)?;
            let run = crate::db::workflows::get_run(conn, "run-1")?.unwrap();
            pin_or_load(conn, &parent, &run)
        })
        .await
        .unwrap()
        .unwrap();

        // As after a restart: a fresh snapshot seeded from the stored pin.
        let key = "kt1128-resume-key";
        let seeded = db
            .with_read_conn(move |conn| seed_resource_snapshots_as(conn, "run-1", key))
            .await
            .unwrap();
        assert!(seeded);
        let ids = vec![id.to_string()];
        let (_, loaded) = super::super::steps::step_skills(&ids, Some("pB"), Some(key)).unwrap();
        assert!(loaded[0].content.contains("B-BODY"));
        let refused = super::super::steps::step_skills(&ids, Some("pA"), Some(key)).unwrap_err();
        assert!(refused.to_string().contains("another project"), "{refused}");
        crate::core::skills::release_skills_snapshot(key);
    }

    /// A repository skill enters the pin as committed on the default branch:
    /// neither the checkout's branch nor a later commit changes what runs.
    #[tokio::test]
    async fn a_repository_skill_is_pinned_from_the_default_branch() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = crate::core::cmd::git_cmd()
                .args(args)
                .current_dir(root.path())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        let write = |body: &str| {
            let dir = root.path().join(".agents/skills/block-migration");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: block-migration\n---\n{body}\n"),
            )
            .unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        write("COMMITTED-BODY");
        git(&["add", "--", "."]);
        git(&["commit", "-q", "-m", "skill"]);
        git(&["checkout", "-q", "-b", "feature"]);
        write("CHECKOUT-BODY");

        let db = seeded().await;
        let path = root.path().display().to_string();
        let stored = db
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &test_project("p1", &path))?;
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "block-migration",
                    ".agents/skills/block-migration/SKILL.md",
                    "Block migration",
                    "2026-10-09T00:00:00Z",
                )?;
                let mut parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                parent.steps[0].skill_ids = vec!["repository:p1:block-migration".into()];
                let deps = Deps::gather(conn, &parent, Some("p1"))?;
                deps.store(conn, "run-1")?;
                rows::get(conn, "run-1", SKILL, "repository:p1:block-migration")
            })
            .await
            .unwrap()
            .expect("the repository skill is pinned");
        assert!(stored.contains("COMMITTED-BODY"), "{stored}");
        assert!(!stored.contains("CHECKOUT-BODY"), "{stored}");
    }

    fn lint_profile(command: &str, commit: &str) -> ProfileSnapshots {
        let snapshot = ProfileSnapshot {
            git_ref: Some("refs/heads/main".into()),
            commit: Some(commit.into()),
            state: crate::core::project_profile::SnapshotState::Loaded,
            error: None,
            values: BTreeMap::from([(
                "project.validation.targets.lint.command".to_string(),
                command.to_string(),
            )]),
        };
        BTreeMap::from([(String::new(), snapshot)])
    }

    async fn fingerprint_with(db: &Database, profiles: ProfileSnapshots) -> String {
        db.with_conn(move |conn| {
            let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
            revision_fingerprint_with_profiles(conn, &parent, None, &profiles)
        })
        .await
        .unwrap()
    }

    /// KT-1100 x KT-1029: retention, on the workflow or a sub-workflow, is
    /// not part of the revision; a step still is.
    #[tokio::test]
    async fn the_fingerprint_leaves_out_retention_but_not_steps() {
        let db = seeded().await;
        let (plain, retained, child_retained, stepped) = db
            .with_conn(|conn| {
                let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                let plain = revision_fingerprint(conn, &parent, None)?;
                let mut retained = parent.clone();
                retained.retention = Some(crate::models::WorkflowRetention {
                    success_days: Some(7),
                    ..Default::default()
                });
                let retained = revision_fingerprint(conn, &retained, None)?;
                let mut child = crate::db::workflows::get_workflow(conn, "child")?.unwrap();
                child.retention = Some(crate::models::WorkflowRetention {
                    failure_days: Some(0),
                    ..Default::default()
                });
                crate::db::workflows::update_workflow(conn, &child)?;
                let child_retained = revision_fingerprint(conn, &parent, None)?;
                let mut stepped = parent.clone();
                stepped.steps[0].name = "renamed".into();
                let stepped = revision_fingerprint(conn, &stepped, None)?;
                Ok((plain, retained, child_retained, stepped))
            })
            .await
            .unwrap();
        assert_eq!(retained, plain, "the workflow's retention");
        assert_eq!(child_retained, plain, "a sub-workflow's retention");
        assert_ne!(stepped, plain, "a step");
    }

    #[tokio::test]
    async fn the_fingerprint_covers_the_repository_profile_the_run_reads() {
        let db = seeded().await;
        let lint = fingerprint_with(&db, lint_profile("make lint", "c1")).await;
        assert_eq!(
            fingerprint_with(&db, lint_profile("make lint", "c2")).await,
            lint,
            "an unchanged profile keeps the fingerprint, whatever its commit"
        );
        assert_ne!(
            fingerprint_with(&db, lint_profile("make lint && curl evil", "c3")).await,
            lint,
            "a changed command changes it"
        );
        let plain = db
            .with_conn(|conn| {
                let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                revision_fingerprint(conn, &parent, None)
            })
            .await
            .unwrap();
        assert_eq!(fingerprint_with(&db, ProfileSnapshots::new()).await, plain);
        assert_ne!(lint, plain);

        // The run pin hashes and stores the same snapshot.
        let pinned = db
            .with_conn(|conn| {
                let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                let run = crate::db::workflows::get_run(conn, "run-1")?.unwrap();
                pin_or_load_with_profiles(conn, &parent, &run, &lint_profile("make lint", "c1"))?
                    .map_err(anyhow::Error::msg)?;
                Ok((
                    pinned_fingerprint(conn, "run-1")?,
                    pinned_project_profile(conn, "run-1", None)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(pinned.0.as_deref(), Some(lint.as_str()));
        assert_eq!(
            pinned.1.unwrap().values["project.validation.targets.lint.command"],
            "make lint"
        );
    }

    #[tokio::test]
    async fn the_fingerprint_follows_every_executed_dependency_and_nothing_else() {
        async fn fingerprint(db: &Database) -> Result<String> {
            db.with_conn(|conn| {
                let parent = crate::db::workflows::get_workflow(conn, "parent")?.unwrap();
                revision_fingerprint(conn, &parent, None)
            })
            .await
        }
        let db = seeded().await;
        let before = fingerprint(&db).await.unwrap();
        db.with_conn(|conn| {
            crate::core::resource_refs::disable_workflows(conn, &["parent".to_string()])?;
            crate::db::quick_prompts::update_quick_prompt_pinned(conn, "qp-1", true).map(|_| ())
        })
        .await
        .unwrap();
        assert_eq!(
            fingerprint(&db).await.unwrap(),
            before,
            "switches are not content"
        );

        // A transitive dependency: the sub-workflow's own Quick Prompt.
        db.with_conn(|conn| {
            crate::db::quick_prompts::update_quick_prompt(conn, &prompt("qp-child", "other"))
        })
        .await
        .unwrap();
        let after = fingerprint(&db).await.unwrap();
        assert_ne!(after, before);

        let pinned = pin(&db, "run-1").await;
        assert!(pinned.is_ok());
        let recorded = db
            .with_conn(|conn| pinned_fingerprint(conn, "run-1"))
            .await
            .unwrap();
        assert_eq!(recorded.as_deref(), Some(after.as_str()));
    }
}
