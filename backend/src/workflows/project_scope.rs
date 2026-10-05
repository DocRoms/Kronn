//! Multi-project workflows (KT-851): the project of a run is resolved when it
//! is triggered, then carried by the run (`workflow_runs.project_id`).
//!
//! - Manual: the explicit `project_id` of the request, else the launching
//!   discussion's or page's project when the workflow serves it, else the
//!   workflow's home project.
//! - Cron: one run per project the workflow serves.
//! - Tracker: the served project whose `repo_url` is the tracked repository.

use rusqlite::Connection;

use crate::core::launch_context::LaunchContext;
use crate::models::{TrackerSourceConfig, Workflow, WorkflowProjectScope};

/// The project a manual (or triggered) run of `workflow` runs for.
pub fn resolve_run_project(
    workflow: &Workflow,
    launch: &LaunchContext,
) -> Result<Option<String>, String> {
    let home = workflow.project_id.as_deref();
    let requested = launch
        .requested_project_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let Some(scope) = workflow.project_scope.as_ref() else {
        return match (home, requested) {
            (Some(home), Some(requested)) if home != requested => Err(format!(
                "Workflow « {} » runs only for its own project; declare it multi-project to run it for another one.",
                workflow.name
            )),
            // A workflow's own declared project always wins over a context.
            (Some(home), _) => Ok(Some(home.to_string())),
            (None, Some(requested)) => Ok(Some(requested.to_string())),
            (None, None) => Ok(launch.project_id.clone()),
        };
    };
    if let Some(requested) = requested {
        return if scope.allows(home, requested) {
            Ok(Some(requested.to_string()))
        } else {
            Err(format!(
                "Workflow « {} » does not serve project `{requested}`.",
                workflow.name
            ))
        };
    }
    if let Some(context) = launch.project_id.as_deref() {
        if scope.allows(home, context) {
            return Ok(Some(context.to_string()));
        }
    }
    match home {
        Some(home) => Ok(Some(home.to_string())),
        None => Err(format!(
            "Workflow « {} » serves several projects: choose the project to run it for.",
            workflow.name
        )),
    }
}

/// The projects a scheduled occurrence runs for: one run each.
pub fn scheduled_projects(
    conn: &Connection,
    workflow: &Workflow,
) -> anyhow::Result<Vec<Option<String>>> {
    let Some(scope) = workflow.project_scope.as_ref() else {
        return Ok(vec![workflow.project_id.clone()]);
    };
    let existing: Vec<String> = crate::db::projects::list_projects(conn)?
        .into_iter()
        .map(|project| project.id)
        .collect();
    let mut projects: Vec<Option<String>> = existing
        .into_iter()
        .filter(|id| scope.allows(workflow.project_id.as_deref(), id))
        .map(Some)
        .collect();
    projects.sort();
    Ok(projects)
}

/// The project a tracker issue maps to: for a multi-project workflow, the
/// served project linked to the tracked repository (`repo_url`); otherwise
/// the workflow's own project. `Err` when no served project is linked to it.
pub fn tracker_project(
    conn: &Connection,
    workflow: &Workflow,
    source: &TrackerSourceConfig,
) -> anyhow::Result<Result<Option<String>, String>> {
    let Some(scope) = workflow.project_scope.as_ref() else {
        return Ok(Ok(workflow.project_id.clone()));
    };
    let TrackerSourceConfig::GitHub { owner, repo } = source;
    let tracked = crate::api::discover::normalize_repo_url(&format!(
        "https://github.com/{}/{}",
        owner.trim(),
        repo.trim()
    ));
    let mut linked: Vec<String> = crate::db::projects::list_projects(conn)?
        .into_iter()
        .filter(|project| {
            project
                .repo_url
                .as_deref()
                .is_some_and(|url| crate::api::discover::normalize_repo_url(url.trim()) == tracked)
        })
        .map(|project| project.id)
        .filter(|id| scope.allows(workflow.project_id.as_deref(), id))
        .collect();
    linked.sort();
    // Several served projects on one repository: the home project wins, then
    // the first by id, so the choice is stable across polls.
    if let Some(home) = workflow
        .project_id
        .as_ref()
        .filter(|home| linked.contains(home))
    {
        return Ok(Ok(Some(home.clone())));
    }
    Ok(match linked.into_iter().next() {
        Some(project) => Ok(Some(project)),
        None => Err(format!(
            "no project served by workflow « {} » is linked to github.com/{owner}/{repo}",
            workflow.name
        )),
    })
}

/// Save-time check: a listed scope names existing projects.
pub fn validate_scope(
    conn: &Connection,
    scope: Option<&WorkflowProjectScope>,
) -> anyhow::Result<Result<(), String>> {
    let Some(WorkflowProjectScope::Projects { project_ids }) = scope else {
        return Ok(Ok(()));
    };
    if project_ids.is_empty() {
        return Ok(Err(
            "`project_scope.project_ids` is empty: list at least one project, or use {\"type\":\"All\"}."
                .into(),
        ));
    }
    for id in project_ids {
        if crate::db::projects::get_project(conn, id)?.is_none() {
            return Ok(Err(format!(
                "`project_scope` names an unknown project `{id}`."
            )));
        }
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow(home: Option<&str>, scope: Option<WorkflowProjectScope>) -> Workflow {
        let mut workflow: Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf", "name": "Phase 1", "project_id": home,
            "trigger": {"type": "Manual"}, "steps": [], "actions": [], "safety": {},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        workflow.project_scope = scope;
        workflow
    }

    fn launch(requested: Option<&str>, context: Option<&str>) -> LaunchContext {
        LaunchContext {
            requested_project_id: requested.map(Into::into),
            project_id: context.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn a_single_project_workflow_keeps_its_project_and_refuses_another() {
        let pinned = workflow(Some("a"), None);
        assert_eq!(
            resolve_run_project(&pinned, &launch(None, Some("b"))).unwrap(),
            Some("a".into())
        );
        assert!(resolve_run_project(&pinned, &launch(Some("b"), None))
            .unwrap_err()
            .contains("multi-project"));
        let global = workflow(None, None);
        assert_eq!(
            resolve_run_project(&global, &launch(None, Some("b"))).unwrap(),
            Some("b".into())
        );
    }

    #[test]
    fn a_multi_project_workflow_runs_for_the_requested_served_project() {
        let listed = workflow(
            Some("home"),
            Some(WorkflowProjectScope::Projects {
                project_ids: vec!["front-apollo".into()],
            }),
        );
        assert_eq!(
            resolve_run_project(&listed, &launch(Some("front-apollo"), None)).unwrap(),
            Some("front-apollo".into())
        );
        assert!(resolve_run_project(&listed, &launch(Some("other"), None))
            .unwrap_err()
            .contains("does not serve"));
        // A context the workflow does not serve falls back to its home.
        assert_eq!(
            resolve_run_project(&listed, &launch(None, Some("other"))).unwrap(),
            Some("home".into())
        );
        let all = workflow(None, Some(WorkflowProjectScope::All));
        assert_eq!(
            resolve_run_project(&all, &launch(None, Some("x"))).unwrap(),
            Some("x".into())
        );
        assert!(resolve_run_project(&all, &launch(None, None))
            .unwrap_err()
            .contains("choose the project"));
    }
}
