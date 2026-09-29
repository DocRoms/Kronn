//! The reference graph between a project's repository resources (KT-905).
//!
//! A workflow points at Quick Prompts, Quick APIs, Quick Execs, sub-workflows
//! and Artifacts by id; an Artifact's action blocks point at workflows and
//! Quick resources. Selecting or transferring one of them without the ones it
//! points at leaves it half working, so the listing exposes both ends of every
//! edge (`uses`, `used_by`) in stable identities, and a reference to something
//! the project does not hold is kept and flagged `missing`.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

use crate::models::{
    ProjectRepositoryResource, ProjectRepositoryResourceKind, RepositoryResourceLink, Workflow,
};

/// The kind and key (an id, or an Artifact's slug) a definition points at.
pub(super) type Reference = (ProjectRepositoryResourceKind, String);

/// What the graph needs to know about one listed resource.
#[derive(Debug, Default, Clone)]
pub(super) struct ResourceReferences {
    /// Other keys definitions use for this resource: the id its repository
    /// file was written under (not the local one after a clone onto another
    /// machine) and an Artifact's page slug.
    pub(super) aliases: Vec<String>,
    /// What its definitions (Kronn side and repository side) point at.
    pub(super) references: Vec<Reference>,
}

/// Listing key of a resource: its kind and its listing `id`.
pub(super) type ResourceKey = (&'static str, String);

fn extend(
    references: &mut Vec<Reference>,
    kind: ProjectRepositoryResourceKind,
    keys: impl IntoIterator<Item = String>,
) {
    references.extend(keys.into_iter().map(|key| (kind, key)));
}

fn workflow_references(resource: &Value) -> Vec<Reference> {
    // A definition that no longer parses as a workflow has no readable edges;
    // the alignment status already reports the file.
    let Ok(workflow) = serde_json::from_value::<Workflow>(resource.clone()) else {
        return Vec::new();
    };
    let dependencies = crate::api::workflows::workflow_dependency_ids([&workflow]);
    let mut references = Vec::new();
    extend(
        &mut references,
        ProjectRepositoryResourceKind::QuickPrompt,
        dependencies.quick_prompts,
    );
    extend(
        &mut references,
        ProjectRepositoryResourceKind::QuickApi,
        dependencies.quick_apis,
    );
    extend(
        &mut references,
        ProjectRepositoryResourceKind::QuickExec,
        dependencies.quick_execs,
    );
    extend(
        &mut references,
        ProjectRepositoryResourceKind::Artifact,
        dependencies.pages,
    );
    extend(
        &mut references,
        ProjectRepositoryResourceKind::Workflow,
        crate::api::workflows::workflow_sub_workflow_child_ids(&workflow),
    );
    references
}

fn artifact_references(resource: &Value) -> Vec<Reference> {
    let Some(html) = resource.get("html").and_then(Value::as_str) else {
        return Vec::new();
    };
    let mut references = Vec::new();
    for (_, range) in crate::db::live_page_actions::page_action_block_ranges(html) {
        let Ok(action) =
            serde_json::from_str::<crate::db::discussion_actions::ActionFence>(&html[range])
        else {
            continue;
        };
        let kind = match action.kind {
            crate::db::discussion_actions::DiscussionActionKind::Workflow => {
                ProjectRepositoryResourceKind::Workflow
            }
            crate::db::discussion_actions::DiscussionActionKind::QuickPrompt => {
                ProjectRepositoryResourceKind::QuickPrompt
            }
            crate::db::discussion_actions::DiscussionActionKind::QuickApi => {
                ProjectRepositoryResourceKind::QuickApi
            }
            crate::db::discussion_actions::DiscussionActionKind::QuickExec => {
                ProjectRepositoryResourceKind::QuickExec
            }
            crate::db::discussion_actions::DiscussionActionKind::Invalid => continue,
        };
        if !action.target_id.trim().is_empty() {
            references.push((kind, action.target_id));
        }
    }
    references
}

/// Every resource a definition points at. Quick Prompts, Quick APIs and Quick
/// Execs are leaves: they point at no other saved resource.
pub(super) fn definition_references(
    kind: ProjectRepositoryResourceKind,
    resource: &Value,
) -> Vec<Reference> {
    match kind {
        ProjectRepositoryResourceKind::Workflow => workflow_references(resource),
        ProjectRepositoryResourceKind::Artifact => artifact_references(resource),
        _ => Vec::new(),
    }
}

fn link_to(target: &ProjectRepositoryResource) -> RepositoryResourceLink {
    RepositoryResourceLink {
        kind: target.kind,
        id: target.id.clone(),
        slug: Some(target.slug.clone()),
        name: target.name.clone(),
        missing: false,
    }
}

fn missing_link(kind: ProjectRepositoryResourceKind, key: String) -> RepositoryResourceLink {
    RepositoryResourceLink {
        kind,
        name: key.clone(),
        id: key,
        slug: None,
        missing: true,
    }
}

fn by_name(link: &RepositoryResourceLink) -> (bool, String, String) {
    (link.missing, link.name.to_lowercase(), link.id.clone())
}

/// Fills `uses` and `used_by` on every resource. A reference resolves to the
/// listed resource of that kind whose `id` or alias it names; failing that it
/// is `missing`. A resource never links to itself, and a target reached twice
/// is listed once.
pub(super) fn link_resources(
    resources: &mut [ProjectRepositoryResource],
    known: &HashMap<ResourceKey, ResourceReferences>,
) {
    let key_of = |resource: &ProjectRepositoryResource| -> ResourceKey {
        (resource.kind.identity_kind(), resource.id.clone())
    };
    let mut index: HashMap<ResourceKey, usize> = HashMap::new();
    for (position, resource) in resources.iter().enumerate() {
        index.insert(key_of(resource), position);
    }
    // Real ids first: an alias never takes the place of another resource's id.
    for (position, resource) in resources.iter().enumerate() {
        for alias in known
            .get(&key_of(resource))
            .map(|entry| entry.aliases.as_slice())
            .unwrap_or_default()
        {
            index
                .entry((resource.kind.identity_kind(), alias.clone()))
                .or_insert(position);
        }
    }

    let mut uses: Vec<Vec<RepositoryResourceLink>> = vec![Vec::new(); resources.len()];
    let mut used_by: Vec<Vec<RepositoryResourceLink>> = vec![Vec::new(); resources.len()];
    for (source, resource) in resources.iter().enumerate() {
        let Some(entry) = known.get(&key_of(resource)) else {
            continue;
        };
        let mut seen: BTreeSet<(bool, String)> = BTreeSet::new();
        for (kind, key) in &entry.references {
            match index.get(&(kind.identity_kind(), key.clone())) {
                Some(&target) if target == source => {}
                Some(&target) => {
                    if seen.insert((false, resources[target].id.clone())) {
                        uses[source].push(link_to(&resources[target]));
                        used_by[target].push(link_to(resource));
                    }
                }
                None => {
                    if seen.insert((true, format!("{}:{key}", kind.identity_kind()))) {
                        uses[source].push(missing_link(*kind, key.clone()));
                    }
                }
            }
        }
    }
    for (position, resource) in resources.iter_mut().enumerate() {
        let mut outgoing = std::mem::take(&mut uses[position]);
        let mut incoming = std::mem::take(&mut used_by[position]);
        outgoing.sort_by_key(by_name);
        incoming.sort_by_key(by_name);
        resource.uses = outgoing;
        resource.used_by = incoming;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ProjectRepositoryResourceLevel, ProjectRepositoryResourceStatus, ResourceAdrLevel,
    };
    use serde_json::json;

    fn resource(
        kind: ProjectRepositoryResourceKind,
        id: &str,
        name: &str,
    ) -> ProjectRepositoryResource {
        ProjectRepositoryResource {
            id: id.to_string(),
            name: name.to_string(),
            slug: name.to_lowercase().replace(' ', "-"),
            kind,
            level: ProjectRepositoryResourceLevel::KronnRequired,
            adr_level: ResourceAdrLevel::N2,
            status: ProjectRepositoryResourceStatus::KronnOnly,
            approval_required: false,
            approved: false,
            diff: None,
            file_diffs: Vec::new(),
            field_diff: Vec::new(),
            repository_paths: Vec::new(),
            write_preview: Vec::new(),
            required_secrets: Vec::new(),
            repository_updated_at: None,
            repository_updated_by: None,
            kronn_updated_at: None,
            aligned_at: None,
            repository_fingerprint: None,
            kronn_fingerprint: None,
            uses: Vec::new(),
            used_by: Vec::new(),
        }
    }

    fn workflow_json(steps: Value) -> Value {
        json!({
            "id": "wf-1", "name": "Nightly", "project_id": null,
            "trigger": {"type": "Manual"},
            "steps": steps,
            "actions": [], "safety": {}, "workspace_config": null,
            "concurrency_limit": null, "enabled": true,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        })
    }

    fn known(
        entries: Vec<(&ProjectRepositoryResource, Vec<&str>, Vec<Reference>)>,
    ) -> HashMap<ResourceKey, ResourceReferences> {
        entries
            .into_iter()
            .map(|(resource, aliases, references)| {
                (
                    (resource.kind.identity_kind(), resource.id.clone()),
                    ResourceReferences {
                        aliases: aliases.into_iter().map(str::to_string).collect(),
                        references,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn workflow_definitions_point_at_every_kind_of_saved_resource() {
        let workflow = workflow_json(json!([
            {"name": "prompt", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-1"},
            {"name": "batch", "step_type": {"type": "BatchQuickPrompt"},
             "batch_quick_prompt_id": "qp-2", "batch_chain_prompt_ids": ["qp-3"]},
            {"name": "api", "step_type": {"type": "ApiCall"}, "quick_api_id": "qa-1"},
            {"name": "collect", "step_type": {"type": "CollectApiData"},
             "collect_api_data": {"sources": [
                 {"alias": "jira", "quick_api_id": "qa-2"},
                 {"alias": "lint", "quick_exec_id": "qe-1"}
             ]}},
            {"name": "page", "step_type": {"type": "PublishPageData"},
             "page_publish": {"page_id": "page-1"}},
            {"name": "dynamic", "step_type": {"type": "PublishPageData"},
             "page_publish": {"page_id": "{{steps.x.output}}"}},
            {"name": "child", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-2"}
        ]));
        let references = definition_references(ProjectRepositoryResourceKind::Workflow, &workflow);
        let identities: BTreeSet<(&str, String)> = references
            .iter()
            .map(|(kind, key)| (kind.identity_kind(), key.clone()))
            .collect();
        assert_eq!(
            identities,
            BTreeSet::from([
                ("quick_prompt", "qp-1".to_string()),
                ("quick_prompt", "qp-2".to_string()),
                ("quick_prompt", "qp-3".to_string()),
                ("quick_api", "qa-1".to_string()),
                ("quick_api", "qa-2".to_string()),
                ("quick_exec", "qe-1".to_string()),
                ("artifact", "page-1".to_string()),
                ("workflow", "wf-2".to_string()),
            ]),
            "a dynamic page id names nothing saved and must not become a reference"
        );
    }

    #[test]
    fn an_unreadable_workflow_definition_has_no_edges() {
        assert!(definition_references(
            ProjectRepositoryResourceKind::Workflow,
            &json!({"not": "a workflow"})
        )
        .is_empty());
    }

    #[test]
    fn artifact_action_blocks_point_at_the_resources_they_launch() {
        let html = r#"<p>x</p>
<script type="application/kronn-action" data-action-id="run">{"kind":"workflow","target_id":"wf-1","values":[]}</script>
<script type="application/kronn-action" data-action-id="ask">{"kind":"quick_prompt","target_id":"qp-1","values":[]}</script>
<script>const notAnAction = {"kind":"workflow","target_id":"wf-9"};</script>"#;
        let references = definition_references(
            ProjectRepositoryResourceKind::Artifact,
            &json!({"html": html}),
        );
        let identities: Vec<(&str, String)> = references
            .iter()
            .map(|(kind, key)| (kind.identity_kind(), key.clone()))
            .collect();
        assert_eq!(
            identities,
            vec![
                ("workflow", "wf-1".to_string()),
                ("quick_prompt", "qp-1".to_string())
            ]
        );
    }

    #[test]
    fn quick_resources_point_at_nothing() {
        assert!(definition_references(
            ProjectRepositoryResourceKind::QuickPrompt,
            &json!({"id": "qp-1", "quick_prompt_id": "qp-2"})
        )
        .is_empty());
    }

    #[test]
    fn both_ends_of_an_edge_are_exposed() {
        let mut items = vec![
            resource(
                ProjectRepositoryResourceKind::Workflow,
                "wf-1",
                "Nightly triage",
            ),
            resource(ProjectRepositoryResourceKind::QuickPrompt, "qp-1", "Review"),
            resource(ProjectRepositoryResourceKind::QuickApi, "qa-1", "Fetch"),
        ];
        let graph = known(vec![(
            &items[0],
            vec![],
            vec![
                (ProjectRepositoryResourceKind::QuickPrompt, "qp-1".into()),
                (ProjectRepositoryResourceKind::QuickApi, "qa-1".into()),
            ],
        )]);
        link_resources(&mut items, &graph);

        let names: Vec<&str> = items[0]
            .uses
            .iter()
            .map(|link| link.name.as_str())
            .collect();
        assert_eq!(names, vec!["Fetch", "Review"]);
        assert!(items[0].used_by.is_empty());
        for leaf in &items[1..] {
            assert!(leaf.uses.is_empty());
            assert_eq!(leaf.used_by.len(), 1);
            assert_eq!(leaf.used_by[0].id, "wf-1");
            assert_eq!(leaf.used_by[0].slug.as_deref(), Some("nightly-triage"));
            assert!(!leaf.used_by[0].missing);
        }
    }

    #[test]
    fn a_reference_to_an_absent_resource_is_kept_and_flagged_missing() {
        let mut items = vec![
            resource(ProjectRepositoryResourceKind::Workflow, "wf-1", "Nightly"),
            resource(ProjectRepositoryResourceKind::QuickPrompt, "qp-1", "Review"),
        ];
        let graph = known(vec![(
            &items[0],
            vec![],
            vec![
                (ProjectRepositoryResourceKind::QuickPrompt, "qp-1".into()),
                (ProjectRepositoryResourceKind::QuickPrompt, "gone".into()),
                (ProjectRepositoryResourceKind::QuickPrompt, "gone".into()),
                // Same key, other kind: a different resource, still missing.
                (ProjectRepositoryResourceKind::QuickApi, "qp-1".into()),
            ],
        )]);
        link_resources(&mut items, &graph);

        assert_eq!(
            items[0].uses.len(),
            3,
            "present first, then each missing once"
        );
        assert!(!items[0].uses[0].missing);
        let missing: Vec<(&str, ProjectRepositoryResourceKind)> = items[0]
            .uses
            .iter()
            .filter(|link| link.missing)
            .map(|link| (link.id.as_str(), link.kind))
            .collect();
        assert_eq!(
            missing,
            vec![
                ("gone", ProjectRepositoryResourceKind::QuickPrompt),
                ("qp-1", ProjectRepositoryResourceKind::QuickApi),
            ]
        );
        assert!(items[0]
            .uses
            .iter()
            .filter(|link| link.missing)
            .all(|link| link.slug.is_none()));
        assert_eq!(
            items[1].used_by.len(),
            1,
            "a missing target has no other end"
        );
    }

    #[test]
    fn a_repository_side_id_resolves_through_the_alias_of_the_local_copy() {
        // The workflow was written on another machine: its file names the
        // Quick Prompt by the id it had there.
        let mut items = vec![
            resource(
                ProjectRepositoryResourceKind::Workflow,
                "repository:workflow:nightly",
                "Nightly",
            ),
            resource(
                ProjectRepositoryResourceKind::QuickPrompt,
                "local-qp",
                "Review",
            ),
            resource(
                ProjectRepositoryResourceKind::Artifact,
                "page-local",
                "Board",
            ),
        ];
        let graph = known(vec![
            (
                &items[0],
                vec![],
                vec![
                    (
                        ProjectRepositoryResourceKind::QuickPrompt,
                        "foreign-qp".into(),
                    ),
                    (ProjectRepositoryResourceKind::Artifact, "board".into()),
                ],
            ),
            (&items[1], vec!["foreign-qp"], vec![]),
            (&items[2], vec!["board"], vec![]),
        ]);
        link_resources(&mut items, &graph);

        let ids: Vec<&str> = items[0].uses.iter().map(|link| link.id.as_str()).collect();
        assert_eq!(ids, vec!["page-local", "local-qp"]);
        assert!(items[0].uses.iter().all(|link| !link.missing));
        assert_eq!(items[1].used_by[0].id, "repository:workflow:nightly");
    }

    #[test]
    fn an_alias_never_takes_the_place_of_another_resources_id() {
        let mut items = vec![
            resource(ProjectRepositoryResourceKind::Workflow, "wf-1", "Caller"),
            resource(
                ProjectRepositoryResourceKind::QuickPrompt,
                "qp-real",
                "Real",
            ),
            resource(
                ProjectRepositoryResourceKind::QuickPrompt,
                "qp-other",
                "Other",
            ),
        ];
        let graph = known(vec![
            (
                &items[0],
                vec![],
                vec![(ProjectRepositoryResourceKind::QuickPrompt, "qp-real".into())],
            ),
            (&items[2], vec!["qp-real"], vec![]),
        ]);
        link_resources(&mut items, &graph);
        assert_eq!(items[0].uses[0].id, "qp-real");
    }

    #[test]
    fn a_loop_is_listed_from_both_sides_and_a_self_reference_is_dropped() {
        let mut items = vec![
            resource(ProjectRepositoryResourceKind::Workflow, "wf-a", "A"),
            resource(ProjectRepositoryResourceKind::Workflow, "wf-b", "B"),
        ];
        let graph = known(vec![
            (
                &items[0],
                vec![],
                vec![
                    (ProjectRepositoryResourceKind::Workflow, "wf-b".into()),
                    (ProjectRepositoryResourceKind::Workflow, "wf-a".into()),
                ],
            ),
            (
                &items[1],
                vec![],
                vec![(ProjectRepositoryResourceKind::Workflow, "wf-a".into())],
            ),
        ]);
        link_resources(&mut items, &graph);

        assert_eq!(items[0].uses.len(), 1);
        assert_eq!(items[0].uses[0].id, "wf-b");
        assert_eq!(items[0].used_by.len(), 1);
        assert_eq!(items[0].used_by[0].id, "wf-b");
        assert_eq!(items[1].uses[0].id, "wf-a");
        assert_eq!(items[1].used_by[0].id, "wf-a");
    }

    #[test]
    fn the_links_serialize_in_stable_identities() {
        let mut items = vec![resource(
            ProjectRepositoryResourceKind::Workflow,
            "wf-1",
            "Nightly",
        )];
        let graph = known(vec![(
            &items[0],
            vec![],
            vec![(ProjectRepositoryResourceKind::QuickExec, "gone".into())],
        )]);
        link_resources(&mut items, &graph);
        let value = serde_json::to_value(&items[0]).unwrap();
        assert_eq!(
            value["uses"],
            json!([{"kind": "quick_exec", "id": "gone", "name": "gone", "missing": true}])
        );
        assert_eq!(value["used_by"], json!([]));
    }
}
