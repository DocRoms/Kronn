//! Artifact portability: a bounded, versioned graph of content and automation
//! definitions. Export and preview are read-only; import never starts a run.
mod import;
pub use import::{import, preview};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{anyhow, bail, Context, Result};
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};

use crate::db::discussion_actions::{ActionFence, DiscussionActionKind};
use crate::{models::*, AppState};

pub(crate) const BUNDLE_KIND: &str = "kronn.artifact";
pub(crate) const BUNDLE_VERSION: u32 = 1;
pub(crate) const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BUNDLE_ITEMS: usize = 512;

use ArtifactResourceKind as ResourceKind;

fn action_dependencies(html: &str) -> Result<Vec<(ResourceKind, String)>> {
    let mut seen = BTreeSet::new();
    let mut dependencies = Vec::new();
    for (reference, raw) in crate::db::live_page_actions::extract_page_action_blocks(html) {
        if !seen.insert(reference.clone()) {
            continue;
        }
        let action: ActionFence = serde_json::from_str(&raw)
            .with_context(|| format!("Invalid Artifact action {reference}"))?;
        let kind = match action.kind {
            DiscussionActionKind::Workflow => ResourceKind::Workflow,
            DiscussionActionKind::QuickPrompt => ResourceKind::QuickPrompt,
            DiscussionActionKind::QuickApi => ResourceKind::QuickApi,
            DiscussionActionKind::QuickExec => ResourceKind::QuickExec,
            DiscussionActionKind::Invalid => bail!("Invalid Artifact action {reference}"),
        };
        if action.target_id.trim().is_empty() {
            bail!("Missing target for Artifact action {reference}");
        }
        dependencies.push((kind, action.target_id));
    }
    Ok(dependencies)
}

pub(crate) fn export_page(conn: &Connection, id: &str) -> Result<ArtifactBundlePage> {
    // Refuse oversized observations before hydrating the complete dataset graph.
    let estimated: Option<i64> = conn.query_row(
        "SELECT length(CAST(r.html AS BLOB)) +
            COALESCE((SELECT SUM(COALESCE(length(CAST(d.current_json AS BLOB)), 0) + COALESCE(length(CAST(d.schema_json AS BLOB)), 0) + length(CAST(d.name AS BLOB)) + 80)
                FROM live_page_datasets d WHERE d.page_id = p.id), 0) +
            COALESCE((SELECT SUM(length(CAST(pt.payload_json AS BLOB)) + length(CAST(COALESCE(pt.dedupe_key, '') AS BLOB)) + 80)
                FROM live_page_dataset_points pt JOIN live_page_datasets d ON d.id = pt.dataset_id
                WHERE d.page_id = p.id), 0)
         FROM live_pages p JOIN live_page_revisions r ON r.id = p.current_revision_id
         WHERE p.id = ?1 OR p.slug = ?1 ORDER BY (p.id = ?1) DESC LIMIT 1", [id], |row| row.get(0)).optional()?;
    let estimated = estimated.ok_or_else(|| anyhow!("Artifact not found: {id}"))?;
    if estimated > MAX_BUNDLE_BYTES as i64 {
        bail!("Artifact exceeds the 16 MiB bundle limit");
    }
    let detail = crate::db::live_pages::get_live_page(conn, id)?
        .ok_or_else(|| anyhow!("Artifact not found: {id}"))?;
    let mut datasets = Vec::with_capacity(detail.datasets.len());
    for view in detail.datasets {
        let dataset = view.dataset;
        let points = if dataset.kind == LivePageDatasetKind::TimeSeries {
            let mut statement = conn.prepare(
                "SELECT observed_at, payload_json, dedupe_key FROM live_page_dataset_points
                 WHERE dataset_id = ?1 ORDER BY observed_at, rowid",
            )?;
            let rows = statement
                .query_map([&dataset.id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(observed_at, payload, dedupe_key)| {
                    Ok(ArtifactBundlePoint {
                        observed_at: chrono::DateTime::parse_from_rfc3339(&observed_at)?
                            .with_timezone(&Utc),
                        payload: serde_json::from_str(&payload)?,
                        dedupe_key,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        datasets.push(ArtifactBundleDataset {
            name: dataset.name,
            kind: dataset.kind,
            has_current: dataset.current.is_some(),
            current: dataset.current.unwrap_or(serde_json::Value::Null),
            schema: dataset.schema,
            max_points: dataset.max_points,
            max_age_days: dataset.max_age_days,
            updated_at: dataset.updated_at,
            points,
        });
    }
    Ok(ArtifactBundlePage {
        id: detail.page.id,
        title: detail.page.title,
        slug: detail.page.slug,
        html: detail.revision.html,
        created_by_agent: detail.revision.created_by_agent,
        datasets,
    })
}

fn export_bundle(conn: &Connection, id: &str) -> Result<ArtifactBundle> {
    let artifact = export_page(conn, id)?;
    let root_id = artifact.id.clone();
    let root_slug = artifact.slug.clone();
    let mut pages = BTreeMap::new();
    let mut workflows = BTreeMap::new();
    let mut quick_prompts = BTreeMap::new();
    let mut quick_apis = BTreeMap::new();
    let mut quick_execs = BTreeMap::new();
    let mut queue: VecDeque<_> = action_dependencies(&artifact.html)?.into();
    let mut seen = BTreeSet::from([
        (ResourceKind::Artifact, root_id.clone()),
        (ResourceKind::Artifact, root_slug.clone()),
    ]);
    // Include all saved publishers of the exported root, including on_failure.
    // Referenced secondary Artifacts do not pull in unrelated incoming workflows.
    for workflow in crate::db::workflows::list_workflows(conn)? {
        if workflow
            .steps
            .iter()
            .chain(&workflow.on_failure)
            .any(|step| {
                step.page_publish.as_ref().is_some_and(|publish| {
                    publish.page_id == root_id || publish.page_id == root_slug
                })
            })
        {
            queue.push_back((ResourceKind::Workflow, workflow.id));
        }
    }
    let mut bytes = serde_json::to_vec(&artifact)?.len();
    while let Some((kind, id)) = queue.pop_front() {
        if !seen.insert((kind, id.clone())) {
            continue;
        }
        let size = match kind {
            ResourceKind::Artifact => {
                let page = export_page(conn, &id)?;
                if pages.contains_key(&page.id) || page.id == root_id {
                    continue;
                }
                seen.insert((ResourceKind::Artifact, page.id.clone()));
                seen.insert((ResourceKind::Artifact, page.slug.clone()));
                queue.extend(action_dependencies(&page.html)?);
                let size = serde_json::to_vec(&page)?.len();
                pages.insert(page.id.clone(), page);
                size
            }
            ResourceKind::Workflow => {
                let workflow = crate::db::workflows::get_workflow(conn, &id)?
                    .ok_or_else(|| anyhow!("Missing workflow dependency: {id}"))?;
                queue.extend(
                    super::workflows::workflow_sub_workflow_child_ids(&workflow)
                        .into_iter()
                        .map(|id| (ResourceKind::Workflow, id)),
                );
                let deps = super::workflows::workflow_dependency_ids([&workflow]);
                for (kind, ids) in [
                    (ResourceKind::Artifact, deps.pages),
                    (ResourceKind::QuickPrompt, deps.quick_prompts),
                    (ResourceKind::QuickApi, deps.quick_apis),
                    (ResourceKind::QuickExec, deps.quick_execs),
                ] {
                    queue.extend(ids.into_iter().map(|id| (kind, id)));
                }
                let size = serde_json::to_vec(&workflow)?.len();
                workflows.insert(id, workflow);
                size
            }
            ResourceKind::QuickPrompt => {
                let value = crate::db::quick_prompts::get_quick_prompt(conn, &id)?
                    .ok_or_else(|| anyhow!("Missing Quick Prompt dependency: {id}"))?;
                let size = serde_json::to_vec(&value)?.len();
                quick_prompts.insert(id, value);
                size
            }
            ResourceKind::QuickApi => {
                let value = crate::db::quick_apis::get_quick_api(conn, &id)?
                    .ok_or_else(|| anyhow!("Missing Quick API dependency: {id}"))?;
                let size = serde_json::to_vec(&value)?.len();
                quick_apis.insert(id, value);
                size
            }
            ResourceKind::QuickExec => {
                let value = crate::db::quick_execs::get_quick_exec(conn, &id)?
                    .ok_or_else(|| anyhow!("Missing Quick Exec dependency: {id}"))?;
                let size = serde_json::to_vec(&value)?.len();
                quick_execs.insert(id, value);
                size
            }
        };
        if 1 + pages.len()
            + workflows.len()
            + quick_prompts.len()
            + quick_apis.len()
            + quick_execs.len()
            > MAX_BUNDLE_ITEMS
        {
            bail!("Artifact bundle exceeds 512 dependencies");
        }
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| anyhow!("Artifact bundle size overflow"))?;
        if bytes > MAX_BUNDLE_BYTES {
            bail!("Artifact bundle exceeds 16 MiB");
        }
    }
    let mut redacted_fields = Vec::new();
    let mut workflows: Vec<_> = workflows.into_values().collect();
    let mut quick_apis: Vec<_> = quick_apis.into_values().collect();
    let mut quick_execs: Vec<_> = quick_execs.into_values().collect();
    for workflow in &mut workflows {
        crate::core::export_secrets::redact_workflow(workflow, &mut redacted_fields);
    }
    for api in &mut quick_apis {
        crate::core::export_secrets::redact_quick_api(api, &mut redacted_fields);
    }
    for exec in &mut quick_execs {
        crate::core::export_secrets::redact_quick_exec(exec, &mut redacted_fields);
    }
    let bundle = ArtifactBundle {
        kind: BUNDLE_KIND.into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        artifact,
        referenced_artifacts: pages.into_values().collect(),
        referenced_workflows: workflows,
        referenced_quick_prompts: quick_prompts.into_values().collect(),
        referenced_quick_apis: quick_apis,
        referenced_quick_execs: quick_execs,
        redacted_fields,
    };
    if serde_json::to_vec(&bundle)?.len() > MAX_BUNDLE_BYTES {
        bail!("Artifact bundle exceeds 16 MiB");
    }
    Ok(bundle)
}

pub async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ArtifactBundle>> {
    match state
        .db
        .with_read_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            export_bundle(&tx, &id)
        })
        .await
    {
        Ok(bundle) => Json(ApiResponse::ok(bundle)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> crate::AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    fn prompt(id: &str, name: &str) -> crate::models::QuickPrompt {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "icon": "x", "prompt_template": "Review it",
            "variables": [], "agent": "ClaudeCode", "project_id": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    /// KT-1037: an Artifact import that brings a Quick Prompt shadowing a
    /// shared `ref:` disables the enabled workflow already using it.
    #[tokio::test]
    async fn an_artifact_import_that_shadows_a_shared_reference_disables_its_user() {
        // Source instance: a page whose action runs a workflow using "Review".
        let source = state();
        let content = source
            .db
            .with_conn(|conn| {
                crate::db::quick_prompts::insert_quick_prompt(conn, &prompt("qp-src", "Review"))?;
                conn.execute(
                    "INSERT INTO workflows (id, name, trigger_json, steps_json, enabled, created_at, updated_at)
                     VALUES ('wf-src', 'uses review', '{\"type\":\"Manual\"}',
                             '[{\"name\":\"s\",\"step_type\":{\"type\":\"Agent\"},\"quick_prompt_id\":\"qp-src\"}]', 1,
                             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                let now = chrono::Utc::now();
                let page = crate::models::LivePage {
                    id: "page-src".into(),
                    project_id: None,
                    title: "Board".into(),
                    slug: "board".into(),
                    current_revision_id: "rev-src".into(),
                    data_revision: 0,
                    created_at: now,
                    updated_at: now,
                    last_published_at: None,
                    pinned: false,
                    archived: false,
                };
                let revision = crate::models::LivePageRevision {
                    id: "rev-src".into(),
                    page_id: "page-src".into(),
                    revision: 1,
                    html: r#"<script type="application/kronn-action" data-action-id="go">{"kind":"workflow","target_id":"wf-src"}</script>"#.into(),
                    created_by_agent: None,
                    created_at: now,
                };
                let tx = conn.unchecked_transaction()?;
                crate::db::live_pages::create_live_page_in_transaction(&tx, &page, &revision, &[], None)?;
                tx.commit()?;
                Ok(serde_json::to_string(&export_bundle(conn, "page-src")?)?)
            })
            .await
            .unwrap();

        // Destination: an enabled unscoped workflow using the shared "Review".
        let target = state();
        target
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO projects (id, name, path, created_at, updated_at)
                     VALUES ('proj-p', 'P', '/tmp/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                crate::db::quick_prompts::insert_quick_prompt(conn, &prompt("qp-global", "Review"))?;
                conn.execute(
                    "INSERT INTO workflows (id, name, trigger_json, steps_json, enabled, created_at, updated_at)
                     VALUES ('wf-user', 'shared user', '{\"type\":\"Manual\"}',
                             '[{\"name\":\"s\",\"quick_prompt_id\":\"ref:prompt:review\"}]', 1,
                             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let request = |digest: Option<String>| ArtifactImportRequest {
            content: content.clone(),
            project_id: Some("proj-p".into()),
            choices: vec![],
            approved_quick_exec_ids: vec![],
            preview_digest: digest,
        };
        let axum::Json(preview) = import::preview(
            axum::extract::State(target.clone()),
            axum::Json(request(None)),
        )
        .await;
        let preview = preview.data.expect("preview");
        let axum::Json(imported) = import::import(
            axum::extract::State(target.clone()),
            axum::Json(request(Some(preview.digest.clone()))),
        )
        .await;
        assert!(imported.success, "{:?}", imported.error);
        let enabled = target
            .db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT enabled FROM workflows WHERE id = 'wf-user'",
                    [],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .await
            .unwrap();
        assert!(!enabled, "the shadowed reference's user is disabled");
        let records = target
            .db
            .with_read_conn(crate::db::workflows::list_auto_disabled)
            .await
            .unwrap();
        let record = records
            .iter()
            .find(|item| item.id == "wf-user")
            .expect("listed");
        assert!(
            record.summary.contains("shadows a shared reference"),
            "{}",
            record.summary
        );
    }

    #[test]
    fn action_dependencies_follow_runtime_first_occurrence_and_unicode() {
        let html = r#"<h1>Équipe 🦀</h1>
            <script type="application/kronn-action" data-action-id="open">{"kind":"quick_exec","target_id":"qe-1"}</script>
            <script type="application/kronn-action" data-action-id="open">{"kind":"workflow","target_id":"ignored"}</script>
            <script type='application/kronn-action' data-action-id='refresh'>{"kind":"workflow","target_id":"wf-1"}</script>
            <script>const target_id = 'not-an-action';</script>"#;
        assert_eq!(
            action_dependencies(html).unwrap(),
            vec![
                (ResourceKind::QuickExec, "qe-1".into()),
                (ResourceKind::Workflow, "wf-1".into())
            ]
        );
        assert!(action_dependencies("<p>Static artifact</p>")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn action_dependencies_reject_invalid_or_missing_targets() {
        for body in [
            "{",
            r#"{"kind":"workflow","target_id":" "}"#,
            r#"{"kind":"unsupported","target_id":"wf"}"#,
        ] {
            let html = format!(
                r#"<script type="application/kronn-action" data-action-id="a">{body}</script>"#
            );
            assert!(action_dependencies(&html).is_err(), "{html}");
        }
    }
}
