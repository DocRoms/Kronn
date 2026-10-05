use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use uuid::Uuid;

#[derive(Clone)]
struct Resource {
    kind: ResourceKind,
    id: String,
    name: String,
    value: Value,
}

impl Resource {
    fn new(kind: ResourceKind, value: impl serde::Serialize) -> Result<Self> {
        let value = serde_json::to_value(value)?;
        let id = value["id"]
            .as_str()
            .context("Missing resource identity")?
            .to_owned();
        let name = value
            .get("name")
            .or_else(|| value.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        Ok(Self {
            kind,
            id,
            name,
            value,
        })
    }

    /// The declared, immutable identity used by `resource_identities`
    /// (ADR-005 slice 3): an Artifact's real `slug`, or the bundle's own `id`
    /// for the other kinds — none of which expose a separate slug field yet.
    fn slug(&self) -> &str {
        if self.kind == ResourceKind::Artifact {
            self.value["slug"].as_str().unwrap()
        } else {
            &self.id
        }
    }
}

struct PlannedResource {
    source: Resource,
    target_id: String,
    entry: ArtifactImportEntry,
    candidate: Option<Value>,
    explicit: Option<ArtifactImportAction>,
}

struct ImportPlan {
    resources: Vec<PlannedResource>,
    preview: ArtifactImportPreview,
    /// Distinct origins the bundle's Pages embed from, sorted.
    embed_origins: Vec<String>,
}

/// The origins every Page of the bundle embeds from: what its HTML writes,
/// plus what it declares (content a script adds at runtime). Declarations that
/// are not valid origins are informative only and simply dropped.
fn bundle_embed_origins(resources: &[Resource]) -> Vec<String> {
    let mut origins = BTreeSet::new();
    for page in resources
        .iter()
        .filter(|r| r.kind == ResourceKind::Artifact)
    {
        let html = page.value["html"].as_str().unwrap_or_default();
        origins.extend(crate::core::embed_origins::declared_origins(html));
        let declared = page.value["embed_origins"].as_array().into_iter().flatten();
        for origin in declared.filter_map(Value::as_str) {
            if let Ok(origin) = crate::core::embed_origins::normalize_origin(origin) {
                origins.insert(origin);
            }
        }
    }
    origins
        .into_iter()
        .take(crate::core::embed_origins::MAX_EMBED_ORIGINS)
        .collect()
}

/// The allowed-sites list once the user's choices for this import are added.
/// Only origins the bundle declares can be added this way.
fn origins_to_allow(plan: &ImportPlan, request: &ArtifactImportRequest) -> Result<Vec<String>> {
    let mut chosen = BTreeSet::new();
    for origin in &request.allow_embed_origins {
        let origin =
            crate::core::embed_origins::normalize_origin(origin).map_err(|e| anyhow!(e))?;
        if !plan.embed_origins.contains(&origin) {
            bail!("{origin} is not embedded by this Artifact");
        }
        chosen.insert(origin);
    }
    Ok(chosen.into_iter().collect())
}

type Remap = BTreeMap<ResourceKind, HashMap<String, String>>;

fn kind_key(kind: ResourceKind) -> String {
    serde_json::to_value(kind)
        .expect("enum serializes")
        .as_str()
        .unwrap()
        .to_owned()
}

fn canonical(mut value: Value, kind: ResourceKind) -> Value {
    if let Some(object) = value.as_object_mut() {
        for key in ["id", "created_at", "updated_at", "pinned", "enabled"] {
            object.remove(key);
        }
        if kind == ResourceKind::Workflow {
            for group in ["steps", "on_failure"] {
                if let Some(steps) = object.get_mut(group).and_then(Value::as_array_mut) {
                    for step in steps {
                        if let Some(step) = step.as_object_mut() {
                            step.remove("id");
                        }
                    }
                }
            }
        }
    }
    value
}

fn load_existing(conn: &Connection, kind: ResourceKind, id: &str) -> Result<Option<Value>> {
    Ok(match kind {
        ResourceKind::Artifact => {
            // An imported identity is an ID, never a coincidentally equal slug.
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_pages WHERE id = ?1)",
                [id],
                |row| row.get(0),
            )?;
            if exists {
                Some(serde_json::to_value(export_page(conn, id)?)?)
            } else {
                None
            }
        }
        ResourceKind::Workflow => crate::db::workflows::get_workflow(conn, id)?
            .map(serde_json::to_value)
            .transpose()?,
        ResourceKind::QuickPrompt => crate::db::quick_prompts::get_quick_prompt(conn, id)?
            .map(serde_json::to_value)
            .transpose()?,
        ResourceKind::QuickApi => crate::db::quick_apis::get_quick_api(conn, id)?
            .map(serde_json::to_value)
            .transpose()?,
        ResourceKind::QuickExec => crate::db::quick_execs::get_quick_exec(conn, id)?
            .map(serde_json::to_value)
            .transpose()?,
    })
}

fn validate_page(page: &ArtifactBundlePage) -> Result<()> {
    if page.title.trim().is_empty() || page.title.chars().count() > 200 {
        bail!("Artifact title must contain 1–200 characters");
    }
    if page.html.trim().is_empty() || page.html.len() > 1024 * 1024 {
        bail!("Artifact HTML must contain 1 byte–1 MiB");
    }
    if page.slug.is_empty()
        || page.slug.len() > 120
        || page.slug.starts_with('-')
        || page.slug.ends_with('-')
        || page.slug.contains("--")
        || !page
            .slug
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        bail!("Invalid Artifact slug");
    }
    let mut names = BTreeSet::new();
    for dataset in &page.datasets {
        if dataset.name.is_empty()
            || dataset.name.len() > 80
            || !dataset
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            || !names.insert(&dataset.name)
        {
            bail!("Invalid or duplicate dataset name: {}", dataset.name);
        }
        if dataset.max_points == 0 || dataset.max_age_days == Some(0) {
            bail!("Invalid dataset retention: {}", dataset.name);
        }
        if dataset.kind == LivePageDatasetKind::TimeSeries && dataset.has_current {
            bail!("Time-series datasets store points, not a snapshot");
        }
        if !dataset.has_current && !dataset.current.is_null() {
            bail!("A dataset without a snapshot must use a null current value");
        }
        if dataset.kind != LivePageDatasetKind::TimeSeries && !dataset.points.is_empty() {
            bail!("Only time-series datasets can contain points");
        }
        let mut keys = BTreeSet::new();
        for point in &dataset.points {
            if let Some(key) = &point.dedupe_key {
                if !keys.insert(key) {
                    bail!("Duplicate dataset point key");
                }
            }
        }
    }
    action_dependencies(&page.html)?;
    Ok(())
}

fn parse_resources(request: &ArtifactImportRequest) -> Result<Vec<Resource>> {
    if request.content.len() > MAX_BUNDLE_BYTES {
        bail!("Artifact bundle exceeds 16 MiB");
    }
    let bundle: ArtifactBundle =
        serde_json::from_str(&request.content).context("Invalid Artifact JSON")?;
    if bundle.kind != BUNDLE_KIND || bundle.version != BUNDLE_VERSION {
        bail!("Unsupported Artifact bundle kind or version");
    }
    let mut resources = Vec::new();
    for page in std::iter::once(bundle.artifact).chain(bundle.referenced_artifacts) {
        validate_page(&page)?;
        resources.push(Resource::new(ResourceKind::Artifact, page)?);
    }
    for workflow in bundle.referenced_workflows {
        super::super::workflows::validate_workflow_for_import(&workflow)
            .map_err(anyhow::Error::msg)?;
        resources.push(Resource::new(ResourceKind::Workflow, workflow)?);
    }
    for prompt in bundle.referenced_quick_prompts {
        validate_prompt_variables(&prompt.variables).map_err(anyhow::Error::msg)?;
        resources.push(Resource::new(ResourceKind::QuickPrompt, prompt)?);
    }
    for api in bundle.referenced_quick_apis {
        validate_prompt_variables(&api.variables).map_err(anyhow::Error::msg)?;
        resources.push(Resource::new(ResourceKind::QuickApi, api)?);
    }
    for exec in bundle.referenced_quick_execs {
        validate_prompt_variables(&exec.variables).map_err(anyhow::Error::msg)?;
        if let Some(error) = crate::core::inline_code::quick_exec_validation_error(
            &exec.name,
            &exec.command,
            &exec.args,
        ) {
            bail!(error);
        }
        resources.push(Resource::new(ResourceKind::QuickExec, exec)?);
    }
    if resources.len() > MAX_BUNDLE_ITEMS {
        bail!("Artifact bundle exceeds 512 dependencies");
    }
    let mut ids = BTreeSet::new();
    let mut page_aliases = BTreeMap::new();
    for resource in &resources {
        if resource.id.is_empty()
            || resource.id.len() > 128
            || !ids.insert((resource.kind, &resource.id))
        {
            bail!("Invalid or duplicate bundle resource identity");
        }
        if resource.kind == ResourceKind::Artifact {
            for alias in [
                resource.id.as_str(),
                resource.value["slug"].as_str().unwrap(),
            ] {
                if page_aliases
                    .insert(alias.to_owned(), resource.id.clone())
                    .is_some_and(|owner| owner != resource.id)
                {
                    bail!("Ambiguous Artifact id or slug: {alias}");
                }
            }
        }
    }
    Ok(resources)
}

fn remap_for(resources: &[PlannedResource]) -> Remap {
    let mut map = BTreeMap::new();
    for kind in [
        ResourceKind::Artifact,
        ResourceKind::Workflow,
        ResourceKind::QuickPrompt,
        ResourceKind::QuickApi,
        ResourceKind::QuickExec,
    ] {
        map.insert(kind, HashMap::new());
    }
    for item in resources {
        let ids = map.get_mut(&item.source.kind).unwrap();
        ids.insert(item.source.id.clone(), item.target_id.clone());
        if item.source.kind == ResourceKind::Artifact {
            ids.insert(
                item.source.value["slug"].as_str().unwrap().to_owned(),
                item.target_id.clone(),
            );
        }
    }
    map
}

fn remap_html(html: &str, map: &Remap, project_id: Option<&str>) -> Result<String> {
    let mut output = html.to_owned();
    // Replace only parsed JSON bodies, in reverse byte order. JS/CSS/text that
    // happens to contain the same identifier is intentionally preserved.
    let mut seen = BTreeSet::new();
    let ranges: Vec<_> = crate::db::live_page_actions::page_action_block_ranges(html)
        .into_iter()
        .filter(|(reference, _)| seen.insert(reference.clone()))
        .collect();
    for (_, range) in ranges.into_iter().rev() {
        let mut body: Value = serde_json::from_str(&html[range.clone()])?;
        let action: ActionFence = serde_json::from_value(body.clone())?;
        let kind = match action.kind {
            DiscussionActionKind::Workflow => ResourceKind::Workflow,
            DiscussionActionKind::QuickPrompt => ResourceKind::QuickPrompt,
            DiscussionActionKind::QuickApi => ResourceKind::QuickApi,
            DiscussionActionKind::QuickExec => ResourceKind::QuickExec,
            DiscussionActionKind::Invalid => bail!("Invalid Artifact action"),
        };
        let target = map[&kind]
            .get(&action.target_id)
            .ok_or_else(|| anyhow!("Missing action dependency: {}", action.target_id))?;
        let mut changed = false;
        if &action.target_id != target {
            body["target_id"] = json!(target);
            changed = true;
        }
        if body["project_id"].is_string() && body["project_id"] != json!(project_id) {
            body["project_id"] = json!(project_id);
            changed = true;
        }
        if !changed {
            continue;
        }
        // Literal '<' must stay escaped inside an HTML script element.
        let replacement = serde_json::to_string(&body)?.replace('<', "\\u003c");
        output.replace_range(range, &replacement);
    }
    Ok(output)
}

fn remap_value(resource: &Resource, map: &Remap, project_id: Option<&str>) -> Result<Value> {
    let mut value = resource.value.clone();
    if resource.kind != ResourceKind::Artifact {
        value["project_id"] = json!(project_id);
    }
    match resource.kind {
        ResourceKind::Artifact => {
            value["html"] = json!(remap_html(
                resource.value["html"].as_str().unwrap(),
                map,
                project_id
            )?);
        }
        ResourceKind::Workflow => {
            let mut workflow: Workflow = serde_json::from_value(value)?;
            for step in workflow
                .steps
                .iter_mut()
                .chain(workflow.on_failure.iter_mut())
            {
                super::super::workflows::remap_workflow_step_dependencies(
                    step,
                    &map[&ResourceKind::QuickPrompt],
                    &map[&ResourceKind::QuickApi],
                    &map[&ResourceKind::QuickExec],
                    &map[&ResourceKind::Artifact],
                    true,
                )
                .map_err(anyhow::Error::msg)?;
                if let Some(id) = step.sub_workflow_id.as_mut() {
                    *id = map[&ResourceKind::Workflow]
                        .get(id)
                        .ok_or_else(|| anyhow!("Missing sub-workflow dependency: {id}"))?
                        .clone();
                }
            }
            value = serde_json::to_value(workflow)?;
        }
        _ => {}
    }
    Ok(value)
}

fn execution_requirements(
    conn: &Connection,
    resources: &[PlannedResource],
) -> Result<Vec<ArtifactImportWarning>> {
    let mut references = BTreeSet::new();
    let mut collect = |value: &Value| {
        for field in [
            "skill_ids",
            "profile_ids",
            "directive_ids",
            "mcp_config_ids",
        ] {
            if let Some(ids) = value[field].as_array() {
                for id in ids.iter().filter_map(Value::as_str) {
                    references.insert((field.to_owned(), id.to_owned()));
                }
            }
        }
        for id in [
            value["connection_id"].as_str(),
            value["agent_settings"]["connection_id"].as_str(),
        ]
        .into_iter()
        .flatten()
        {
            if !id.is_empty() {
                references.insert(("connection_id".into(), id.to_owned()));
            }
        }
        if let Some(id) = value["api_config_id"].as_str().filter(|id| !id.is_empty()) {
            references.insert(("mcp_config_ids".into(), id.to_owned()));
        }
    };
    for item in resources {
        let value = if item.entry.disposition == ArtifactImportDisposition::Reuse {
            item.candidate.as_ref().unwrap_or(&item.source.value)
        } else {
            &item.source.value
        };
        if item.source.kind == ResourceKind::Workflow {
            for group in ["steps", "on_failure"] {
                for step in value[group].as_array().into_iter().flatten() {
                    collect(step);
                }
            }
        } else if item.source.kind != ResourceKind::Artifact {
            collect(value);
        }
    }
    let mut warnings = Vec::new();
    for (kind, id) in references {
        let (available, warning_kind) = match kind.as_str() {
            "skill_ids" => (crate::core::skills::get_skill(&id).is_some(), "skill"),
            "profile_ids" => (crate::core::profiles::get_profile(&id).is_some(), "profile"),
            "directive_ids" => (
                crate::core::directives::get_directive(&id).is_some(),
                "directive",
            ),
            "connection_id" => (
                crate::db::external_api_connections::get(conn, &id)?.is_some(),
                "model_connection",
            ),
            "mcp_config_ids" => (
                crate::db::mcps::get_config(conn, &id)?.is_some(),
                "plugin_connection",
            ),
            _ => unreachable!("known reference field"),
        };
        if !available {
            warnings.push(ArtifactImportWarning {
                kind: warning_kind.into(),
                id,
            });
        }
    }
    Ok(warnings)
}

fn prepare_plan(conn: &Connection, request: &ArtifactImportRequest) -> Result<ImportPlan> {
    let resources = parse_resources(request)?;
    let embed_origins = bundle_embed_origins(&resources);
    if let Some(project) = &request.project_id {
        if crate::db::projects::get_project(conn, project)?.is_none() {
            bail!("Import project not found");
        }
    }
    let known: BTreeSet<_> = resources.iter().map(|r| (r.kind, r.id.clone())).collect();
    let mut approved_execs = BTreeSet::new();
    for id in &request.approved_quick_exec_ids {
        if !known.contains(&(ResourceKind::QuickExec, id.clone()))
            || !approved_execs.insert(id.clone())
        {
            bail!("Unknown or duplicate Quick Exec approval");
        }
    }
    let mut choices = BTreeMap::new();
    for choice in &request.choices {
        let key = (choice.kind, choice.source_id.clone());
        if !known.contains(&key) || choices.insert(key, choice).is_some() {
            bail!("Unknown or duplicate import choice");
        }
    }
    let root_key = (resources[0].kind, resources[0].id.clone());
    if choices
        .get(&root_key)
        .is_some_and(|choice| choice.action == ArtifactImportAction::Reuse)
    {
        bail!("The imported root Artifact must be a new resource");
    }
    let project_key =
        crate::db::resource_identities::project_key(conn, request.project_id.as_deref())?;
    let mut planned = Vec::new();
    let mut observations = Vec::new();
    for source in resources {
        let key = (source.kind, source.id.clone());
        let choice = choices.get(&key);
        let is_root = key == root_key;
        if is_root {
            // The root is always new (a fresh copy per import, never reused —
            // see `artifact_roundtrip_preserves_null_points_and_reuses_previous_import_identities`).
            // Observe the identity this slug currently maps to, so committing
            // a DIFFERENT mapping between preview and commit invalidates the
            // stale preview, without hydrating every previous copy.
            let prior_identity = crate::db::resource_identities::lookup_scoped(
                conn,
                &project_key,
                &kind_key(source.kind),
                source.slug(),
            )?;
            observations
                .push(json!({"kind":source.kind,"id":source.id,"prior_identity":prior_identity}));
            planned.push(PlannedResource {
                target_id: Uuid::new_v4().to_string(),
                entry: ArtifactImportEntry {
                    kind: source.kind,
                    source_id: source.id.clone(),
                    name: source.name.clone(),
                    disposition: ArtifactImportDisposition::Create,
                    existing_id: None,
                    reason: "root".into(),
                    quick_exec: None,
                    quick_api: None,
                },
                source,
                candidate: None,
                explicit: choice.map(|c| c.action),
            });
            continue;
        }
        let mut candidate_ids = BTreeSet::from([source.id.clone()]);
        candidate_ids.extend(crate::db::resource_identities::lookup_scoped(
            conn,
            &project_key,
            &kind_key(source.kind),
            source.slug(),
        )?);
        if let Some(id) = choice.and_then(|choice| choice.target_id.as_ref()) {
            candidate_ids.insert(id.clone());
        }
        let mut candidates = Vec::new();
        for id in candidate_ids {
            if let Some(value) = load_existing(conn, source.kind, &id)? {
                candidates.push((id, value));
            }
        }
        observations.push(json!({"kind":source.kind,"id":source.id,"candidates":candidates}));
        let mut expected = source.value.clone();
        if source.kind != ResourceKind::Artifact {
            expected["project_id"] = json!(request.project_id);
        }
        let canonical_expected = canonical(expected, source.kind);
        let candidate =
            if let Some(choice) = choice.filter(|c| c.action == ArtifactImportAction::Reuse) {
                let id = choice
                    .target_id
                    .as_ref()
                    .context("A reuse choice requires a target identity")?;
                Some(
                    candidates
                        .iter()
                        .find(|(candidate, _)| candidate == id)
                        .ok_or_else(|| anyhow!("Reuse target no longer exists: {id}"))?
                        .clone(),
                )
            } else {
                candidates
                    .iter()
                    .find(|(_, value)| canonical(value.clone(), source.kind) == canonical_expected)
                    .or_else(|| candidates.iter().find(|(id, _)| id == &source.id))
                    .or_else(|| candidates.first())
                    .cloned()
            };
        let (disposition, reason) =
            if is_root || choice.is_some_and(|c| c.action == ArtifactImportAction::Create) {
                (
                    ArtifactImportDisposition::Create,
                    if is_root { "root" } else { "chosen" },
                )
            } else if choice.is_some_and(|c| c.action == ArtifactImportAction::Reuse) {
                (ArtifactImportDisposition::Reuse, "chosen")
            } else if let Some((_, value)) = &candidate {
                if canonical(value.clone(), source.kind) == canonical_expected {
                    (ArtifactImportDisposition::Reuse, "identical")
                } else {
                    (ArtifactImportDisposition::Conflict, "changed")
                }
            } else {
                (ArtifactImportDisposition::Create, "missing")
            };
        let existing_id = candidate.as_ref().map(|(id, _)| id.clone());
        let target_id = if disposition == ArtifactImportDisposition::Create {
            Uuid::new_v4().to_string()
        } else {
            existing_id.clone().unwrap()
        };
        let entry = ArtifactImportEntry {
            kind: source.kind,
            source_id: source.id.clone(),
            name: source.name.clone(),
            disposition,
            existing_id,
            reason: reason.into(),
            quick_exec: None,
            quick_api: None,
        };
        planned.push(PlannedResource {
            source,
            target_id,
            entry,
            candidate: candidate.map(|(_, value)| value),
            explicit: choice.map(|c| c.action),
        });
    }
    let mut issues = Vec::new();
    // A reused publisher must not keep pointing at the old Artifact. Propagate
    // required copies through the graph until no retained definition changes.
    for _ in 0..=planned.len() {
        let map = remap_for(&planned);
        let mut changed = false;
        for item in &mut planned {
            let remapped = remap_value(&item.source, &map, request.project_id.as_deref())?;
            if item.entry.disposition == ArtifactImportDisposition::Create {
                continue;
            }
            if item.explicit.is_none()
                && item.candidate.as_ref().is_some_and(|candidate| {
                    canonical(candidate.clone(), item.source.kind)
                        == canonical(remapped.clone(), item.source.kind)
                })
            {
                item.entry.disposition = ArtifactImportDisposition::Reuse;
                item.entry.reason = "identical".into();
                continue;
            }
            let mut source_scoped = item.source.value.clone();
            if item.source.kind != ResourceKind::Artifact {
                source_scoped["project_id"] = json!(request.project_id);
            }
            if canonical(remapped, item.source.kind) != canonical(source_scoped, item.source.kind) {
                if item.explicit == Some(ArtifactImportAction::Reuse) {
                    issues.push(format!(
                        "{} must be copied because its dependency destination changes",
                        item.source.name
                    ));
                } else {
                    item.entry.disposition = ArtifactImportDisposition::Create;
                    item.entry.reason = "retargeted".into();
                    item.target_id = Uuid::new_v4().to_string();
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    issues.sort();
    issues.dedup();
    for item in &mut planned {
        if item.entry.disposition != ArtifactImportDisposition::Create {
            continue;
        }
        match item.source.kind {
            ResourceKind::QuickExec => {
                let exec: QuickExec = serde_json::from_value(item.source.value.clone())?;
                item.entry.quick_exec = Some(ArtifactImportExecReview {
                    command: exec.command,
                    args: exec.args,
                    approved: approved_execs.contains(&item.source.id),
                });
            }
            ResourceKind::QuickApi => {
                let api: QuickApi = serde_json::from_value(item.source.value.clone())?;
                item.entry.quick_api = Some(ArtifactImportApiReview {
                    method: api.api_method,
                    endpoint: api.api_endpoint_path,
                    plugin: api.api_plugin_slug,
                });
            }
            _ => {}
        }
    }
    let warnings = execution_requirements(conn, &planned)?;
    let digest = Sha256::digest(serde_json::to_vec(&json!({
        "content":request.content,"project_id":request.project_id,"choices":request.choices,"approved_quick_exec_ids":approved_execs,"observations":observations,"warnings":warnings
    }))?).iter().fold(String::with_capacity(64), |mut result, byte| {
        write!(&mut result, "{byte:02x}").expect("writing to String"); result
    });
    let preview = ArtifactImportPreview {
        title: planned[0].source.name.clone(),
        entries: planned.iter().map(|r| r.entry.clone()).collect(),
        can_import: issues.is_empty()
            && planned.iter().all(|r| {
                r.entry.disposition != ArtifactImportDisposition::Conflict
                    && r.entry.quick_exec.as_ref().is_none_or(|exec| exec.approved)
            }),
        issues,
        warnings,
        digest,
        // Filled by the caller, which knows this Kronn's allowed sites.
        embed_origins: Vec::new(),
    };
    Ok(ImportPlan {
        resources: planned,
        preview,
        embed_origins,
    })
}

pub async fn preview(
    State(state): State<AppState>,
    Json(request): Json<ArtifactImportRequest>,
) -> Json<ApiResponse<ArtifactImportPreview>> {
    let allowed = state.config.read().await.embed_allowed_origins.clone();
    match state
        .db
        .with_read_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            prepare_plan(&tx, &request).map(|plan| {
                let mut preview = plan.preview;
                preview.embed_origins = plan
                    .embed_origins
                    .into_iter()
                    .map(|origin| ArtifactImportEmbedOrigin {
                        already_allowed: allowed.contains(&origin),
                        origin,
                    })
                    .collect();
                preview
            })
        })
        .await
    {
        Ok(preview) => Json(ApiResponse::ok(preview)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

fn record_origin(conn: &Connection, project_key: &str, resource: &PlannedResource) -> Result<()> {
    crate::db::resource_identities::upsert(
        conn,
        project_key,
        &kind_key(resource.source.kind),
        resource.source.slug(),
        &resource.target_id,
    )
}

/// Drop workflow-step skill/profile/directive references that don't resolve
/// on this instance instead of silently carrying a dangling id into the new
/// local copy — `execution_requirements` already warns about the same gap,
/// this is what makes the created step act on it (ADR-005 slice 3).
fn resolve_step_requirements(step: &mut WorkflowStep) {
    step.skill_ids
        .retain(|id| crate::core::skills::get_skill(id).is_some());
    step.profile_ids
        .retain(|id| crate::core::profiles::get_profile(id).is_some());
    step.directive_ids
        .retain(|id| crate::core::directives::get_directive(id).is_some());
}

fn create_imported_page(
    tx: &rusqlite::Transaction<'_>,
    item: &PlannedResource,
    map: &Remap,
    project_id: Option<&str>,
) -> Result<LivePage> {
    let exported: ArtifactBundlePage =
        serde_json::from_value(remap_value(&item.source, map, project_id)?)?;
    let now = Utc::now();
    let mut slug = exported.slug;
    while tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_pages WHERE slug = ?1)",
        [&slug],
        |row| row.get::<_, bool>(0),
    )? {
        let stem = item.source.value["slug"].as_str().unwrap();
        let stem = &stem[..stem.len().min(87)]; // validated ASCII slug
        slug = format!("{}-{}", stem.trim_end_matches('-'), Uuid::new_v4().simple());
    }
    let revision_id = Uuid::new_v4().to_string();
    let page = LivePage {
        id: item.target_id.clone(),
        project_id: project_id.map(str::to_owned),
        title: exported.title,
        slug,
        current_revision_id: revision_id.clone(),
        data_revision: 0,
        created_at: now,
        updated_at: now,
        last_published_at: None,
        pinned: false,
        archived: false,
    };
    let revision = LivePageRevision {
        id: revision_id,
        page_id: page.id.clone(),
        revision: 1,
        html: exported.html,
        created_by_agent: exported.created_by_agent,
        created_at: now,
    };
    let datasets: Vec<_> = exported
        .datasets
        .iter()
        .map(|dataset| CreateLivePageDataset {
            name: dataset.name.clone(),
            kind: dataset.kind,
            initial: dataset.has_current.then(|| dataset.current.clone()),
            schema: dataset.schema.clone(),
            max_points: Some(dataset.max_points),
            max_age_days: dataset.max_age_days,
        })
        .collect();
    crate::db::live_pages::create_live_page_in_transaction(tx, &page, &revision, &datasets, None)?;
    // What the Artifact declares it embeds (content its scripts build) travels
    // on: the next export merges it with what the HTML spells out. Never a
    // permission: those stay in this Kronn's config.
    tx.execute(
        "UPDATE live_pages SET declared_embed_origins = ?1 WHERE id = ?2",
        rusqlite::params![
            serde_json::to_string(&crate::core::embed_origins::normalize_declared(
                &exported.embed_origins
            ))?,
            page.id
        ],
    )?;
    for dataset in exported.datasets {
        tx.execute(
            "UPDATE live_page_datasets SET updated_at = ?1 WHERE page_id = ?2 AND name = ?3",
            rusqlite::params![dataset.updated_at.to_rfc3339(), page.id, dataset.name],
        )?;
        for point in dataset.points {
            tx.execute("INSERT INTO live_page_dataset_points
                (id, dataset_id, observed_at, payload_json, dedupe_key, created_at)
                SELECT ?1, id, ?2, ?3, ?4, ?5 FROM live_page_datasets WHERE page_id = ?6 AND name = ?7",
                rusqlite::params![Uuid::new_v4().to_string(), point.observed_at.to_rfc3339(),
                    serde_json::to_string(&point.payload)?, point.dedupe_key, now.to_rfc3339(), page.id, dataset.name])?;
        }
    }
    Ok(page)
}

fn commit_plan(
    tx: &rusqlite::Transaction<'_>,
    plan: ImportPlan,
    request: &ArtifactImportRequest,
) -> Result<ArtifactImportResult> {
    let map = remap_for(&plan.resources);
    let now = Utc::now();
    let project_key =
        crate::db::resource_identities::project_key(tx, request.project_id.as_deref())?;
    // Create action targets first, so Page action ingestion can resolve all of
    // them. Workflows contain JSON references and remain disabled throughout.
    for item in plan.resources.iter().filter(|item| {
        item.source.kind != ResourceKind::Artifact
            && item.entry.disposition == ArtifactImportDisposition::Create
    }) {
        let mut value = remap_value(&item.source, &map, request.project_id.as_deref())?;
        value["id"] = json!(item.target_id);
        value["created_at"] = json!(now);
        value["updated_at"] = json!(now);
        value["pinned"] = json!(false);
        match item.source.kind {
            ResourceKind::QuickPrompt => {
                crate::db::quick_prompts::insert_quick_prompt(tx, &serde_json::from_value(value)?)?
            }
            ResourceKind::QuickApi => {
                // Retarget to a local API config for the same plugin, reusing
                // the same rebind the legacy workflow-import endpoint uses.
                let mut api: QuickApi = serde_json::from_value(value)?;
                super::super::workflows::rebind_quick_api_config(
                    tx,
                    &mut api,
                    request.project_id.as_deref(),
                );
                crate::db::quick_apis::insert_quick_api(tx, &api)?
            }
            ResourceKind::QuickExec => {
                crate::db::quick_execs::insert_quick_exec(tx, &serde_json::from_value(value)?)?
            }
            ResourceKind::Workflow => {
                let mut workflow: Workflow = serde_json::from_value(value)?;
                workflow.enabled = false;
                for step in workflow
                    .steps
                    .iter_mut()
                    .chain(workflow.on_failure.iter_mut())
                {
                    step.id = Some(Uuid::new_v4().to_string());
                    step.gate_notify_url = None;
                    resolve_step_requirements(step);
                }
                super::super::workflows::rebind_api_configs(
                    tx,
                    &mut workflow.steps,
                    request.project_id.as_deref(),
                );
                super::super::workflows::rebind_api_configs(
                    tx,
                    &mut workflow.on_failure,
                    request.project_id.as_deref(),
                );
                crate::db::workflows::insert_workflow(tx, &workflow)?;
            }
            ResourceKind::Artifact => unreachable!("filtered above"),
        }
        record_origin(tx, &project_key, item)?;
    }
    let mut root = None;
    for (index, item) in plan.resources.iter().enumerate() {
        if item.source.kind != ResourceKind::Artifact
            || item.entry.disposition != ArtifactImportDisposition::Create
        {
            continue;
        }
        let page = create_imported_page(tx, item, &map, request.project_id.as_deref())?;
        record_origin(tx, &project_key, item)?;
        if index == 0 {
            root = Some(page);
        }
    }
    Ok(ArtifactImportResult {
        artifact: root.context("Imported root Artifact missing")?,
        entries: plan.preview.entries,
        allowed_embed_origins: Vec::new(),
        not_allowed_embed_origins: Vec::new(),
        embed_origins_error: None,
    })
}

pub async fn import(
    State(state): State<AppState>,
    Json(request): Json<ArtifactImportRequest>,
) -> Json<ApiResponse<ArtifactImportResult>> {
    // Sites to allow are checked against a snapshot of the configuration
    // before anything is written. The configuration is NOT locked while the
    // import is planned and written: only the final update takes the lock.
    let allowed_now = if request.allow_embed_origins.is_empty() {
        Vec::new()
    } else {
        state.config.read().await.embed_allowed_origins.clone()
    };
    let imported = state
        .db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let plan = prepare_plan(&tx, &request)?;
            if request.preview_digest.as_deref() != Some(plan.preview.digest.as_str()) {
                bail!("Artifact import preview is stale; review the import again");
            }
            if !plan.preview.can_import {
                bail!("Resolve Artifact import conflicts and approve each new Quick Exec first");
            }
            let allow = origins_to_allow(&plan, &request)?;
            crate::core::embed_origins::apply_changes(&allowed_now, &allow, &[]).map_err(
                |error| {
                    anyhow!("The chosen sites cannot be allowed, nothing was imported: {error}")
                },
            )?;
            let mut result = commit_plan(&tx, plan, &request)?;
            tx.commit()?;
            result.allowed_embed_origins = allow;
            Ok(result)
        })
        .await;
    match imported {
        Ok(mut imported) => {
            // The sites the user chose join the allowed list only now that the
            // import is committed; a cancelled import never reaches this point.
            // Anything that keeps them out from here on (the list changed in
            // the meantime, the configuration cannot be saved) is reported.
            if !imported.allowed_embed_origins.is_empty() {
                let mut config = state.config.write().await;
                let outcome = match crate::core::embed_origins::apply_changes(
                    &config.embed_allowed_origins,
                    &imported.allowed_embed_origins,
                    &[],
                ) {
                    Ok(next) => {
                        let previous = std::mem::replace(&mut config.embed_allowed_origins, next);
                        crate::core::config::save(&config).await.map_err(|error| {
                            config.embed_allowed_origins = previous;
                            format!("The configuration could not be saved: {error}")
                        })
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = outcome {
                    tracing::warn!("Imported Artifact, but could not allow its sites: {error}");
                    imported.not_allowed_embed_origins =
                        std::mem::take(&mut imported.allowed_embed_origins);
                    imported.embed_origins_error = Some(error);
                }
            }
            Json(ApiResponse::ok(imported))
        }
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> Remap {
        BTreeMap::from([
            (ResourceKind::Artifact, HashMap::new()),
            (
                ResourceKind::Workflow,
                HashMap::from([("old".into(), "new".into())]),
            ),
            (ResourceKind::QuickPrompt, HashMap::new()),
            (ResourceKind::QuickApi, HashMap::new()),
            (ResourceKind::QuickExec, HashMap::new()),
        ])
    }

    #[test]
    fn remap_html_changes_only_first_action_body_and_preserves_script_boundaries() {
        let html = r#"<p>Équipe 🦀 old</p><script>const literal = 'old';</script>
<script type="application/kronn-action" data-action-id="go">{"kind":"workflow","target_id":"old","values":[{"name":"note","value":"<b>old</b>"}]}</script>
<script type="application/kronn-action" data-action-id="go">{"kind":"workflow","target_id":"ignored"}</script>"#;
        let remapped = remap_html(html, &maps(), None).unwrap();
        assert!(remapped.contains("<p>Équipe 🦀 old</p><script>const literal = 'old';</script>"));
        assert!(remapped.contains(r#""target_id":"new""#));
        assert!(remapped.contains(r#""target_id":"ignored""#));
        let actions = crate::db::live_page_actions::extract_page_action_blocks(&remapped);
        let body: Value = serde_json::from_str(&actions[0].1).unwrap();
        assert_eq!(body["values"][0]["value"], "<b>old</b>");
        assert!(!actions[0].1.contains('<'));
    }

    #[test]
    fn remap_html_is_byte_identical_when_no_reference_changes() {
        let html = r#"<p>🦀</p><script type="application/kronn-action" data-action-id="go">{ "kind": "workflow", "target_id": "old" }</script>"#;
        let mut map = maps();
        map.get_mut(&ResourceKind::Workflow)
            .unwrap()
            .insert("old".into(), "old".into());
        assert_eq!(remap_html(html, &map, None).unwrap(), html);
        assert!(remap_html(
            html,
            &BTreeMap::from([(ResourceKind::Workflow, HashMap::new())]),
            None
        )
        .is_err());
    }
}
