//! Local identity table `(project, kind, slug) -> id`, generalising
//! `artifact_import_origins` (migration 188) into a project-scoped mapping
//! with at most one target per key (ADR-005 slice 3,
//! `docs/design/adr-005-project-resource-library.md#identity`).
//!
//! Re-importing the same slug therefore always resolves — and, through
//! `upsert`, updates — the same local row instead of accumulating an
//! unrelated copy. This module also resolves the symbolic cross-resource
//! references the same slice introduces (`prompt:<slug>`, `workflow:<slug>`,
//! `qe:<slug>`, `qa:<slug>`, `skill:<slug>`, `plugin:<server>`).
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub struct ResourceIdentity {
    pub slug: String,
    pub updated_at: String,
}

/// Scope key for the identity table: the destination project's normalised
/// `repo_url` when known, else its path, else `""` for no project — never
/// the local project UUID, which is instance-specific and does not survive
/// a clone onto another machine.
pub fn project_key(conn: &Connection, project_id: Option<&str>) -> Result<String> {
    let Some(project_id) = project_id else {
        return Ok(String::new());
    };
    Ok(match crate::db::projects::get_project(conn, project_id)? {
        Some(project) => project
            .repo_url
            .as_deref()
            .map(crate::api::discover::normalize_repo_url)
            .unwrap_or(project.path),
        None => String::new(),
    })
}

/// Records that `slug` (of `kind`, within `project_key`) now maps to
/// `target_id`. Reimporting the same slug updates this same row rather than
/// creating a duplicate mapping.
pub fn upsert(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    target_id: &str,
) -> Result<()> {
    upsert_at(
        conn,
        project_key,
        kind,
        slug,
        target_id,
        &chrono::Utc::now().to_rfc3339(),
    )
}

/// Same stable identity upsert, using the resource timestamp carried by a
/// repository document. Import and publication use this to keep the file and
/// local baseline on the same explicit clock value.
pub fn upsert_at(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    target_id: &str,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO resource_identities (project_key, kind, slug, target_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(project_key, kind, slug) DO UPDATE SET
            target_id = excluded.target_id, updated_at = excluded.updated_at",
        rusqlite::params![project_key, kind, slug, target_id, updated_at],
    )?;
    Ok(())
}

/// The local id `(project_key, kind, slug)` currently maps to, if any.
pub fn lookup(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT target_id FROM resource_identities WHERE project_key = ?1 AND kind = ?2 AND slug = ?3",
        rusqlite::params![project_key, kind, slug],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Publication/import baseline for a concrete local resource, when known.
pub fn find_by_target(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    target_id: &str,
) -> Result<Option<ResourceIdentity>> {
    conn.query_row(
        "SELECT slug, updated_at FROM resource_identities
          WHERE project_key = ?1 AND kind = ?2 AND target_id = ?3
          ORDER BY updated_at DESC LIMIT 1",
        rusqlite::params![project_key, kind, target_id],
        |row| {
            Ok(ResourceIdentity {
                slug: row.get(0)?,
                updated_at: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// The local id `(kind, slug)` currently maps to, trying `project_key` first
/// and falling back to the global scope (`""`) when it is non-empty and the
/// project-scoped lookup misses. Migration 196 placed every identity
/// recorded before this table existed into the global scope, so a resource
/// previously imported with no project must still resolve when the same
/// slug is later imported into a project — this is the single rule shared
/// by `resolve_symbolic_reference` and the artifact importer's candidate
/// lookups.
pub fn lookup_scoped(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
) -> Result<Option<String>> {
    if let Some(id) = lookup(conn, project_key, kind, slug)? {
        return Ok(Some(id));
    }
    if project_key.is_empty() {
        return Ok(None);
    }
    lookup(conn, "", kind, slug)
}

/// The kinds a symbolic reference may name (`<kind>:<slug>`).
pub const REFERENCE_KINDS: [&str; 7] = [
    "workflow", "qe", "qa", "prompt", "skill", "plugin", "artifact",
];

/// Resolves a symbolic reference (`workflow:<slug>`, `qe:<slug>`, `qa:<slug>`,
/// `prompt:<slug>`, `skill:<slug>`, `plugin:<server>`, `artifact:<slug>`) to
/// the local identifier it names, first within `project_id`, then in the
/// global scope (ADR-005 §Identity). Each scope reads the identity table
/// before the resources' own names, so a resource created here and never
/// published still resolves under the slug publication would give it.
/// `Ok(None)` = nothing matches; `Err` = two resources of one scope match.
pub fn resolve_symbolic_reference(
    conn: &Connection,
    reference: &str,
    project_id: Option<&str>,
) -> Result<Option<String>> {
    let Some((kind, slug)) = reference.split_once(':') else {
        return Ok(None);
    };
    let slug = slug.trim();
    if slug.is_empty() {
        return Ok(None);
    }
    let project_id = project_id.filter(|id| !id.is_empty());
    if kind == "plugin" {
        return crate::db::mcps::find_config_for_server(conn, slug, project_id);
    }
    let table_kind = match kind {
        "prompt" => "quick_prompt",
        "workflow" => "workflow",
        "qe" => "quick_exec",
        "qa" => "quick_api",
        "artifact" => "artifact",
        "skill" => "skill",
        _ => return Ok(None),
    };
    if let Some(project_id) = project_id {
        let key = project_key(conn, Some(project_id))?;
        if let Some(id) = existing_identity(conn, &key, table_kind, slug)? {
            return Ok(Some(id));
        }
        if let Some(id) = match_by_slug(conn, kind, slug, Some(project_id))? {
            return Ok(Some(id));
        }
    }
    if let Some(id) = existing_identity(conn, "", table_kind, slug)? {
        return Ok(Some(id));
    }
    if let Some(id) = match_by_slug(conn, kind, slug, None)? {
        return Ok(Some(id));
    }
    if kind == "skill" {
        // Built-in skills last; another project's own skill never resolves.
        return Ok(crate::core::skills::get_skill(slug)
            .filter(|skill| skill.is_builtin)
            .map(|skill| skill.id));
    }
    Ok(None)
}

/// The identity of `(scope, kind, slug)` when its target still exists. An
/// empty project key outside the global pass would read the global scope.
fn existing_identity(
    conn: &Connection,
    scope_key: &str,
    table_kind: &str,
    slug: &str,
) -> Result<Option<String>> {
    match lookup(conn, scope_key, table_kind, slug)? {
        Some(id) if target_exists(conn, table_kind, &id)? => Ok(Some(id)),
        _ => Ok(None),
    }
}

fn target_exists(conn: &Connection, table_kind: &str, id: &str) -> Result<bool> {
    Ok(match table_kind {
        "workflow" => crate::db::workflows::get_workflow(conn, id)?.is_some(),
        "quick_prompt" => crate::db::quick_prompts::get_quick_prompt(conn, id)?.is_some(),
        "quick_api" => crate::db::quick_apis::get_quick_api(conn, id)?.is_some(),
        "quick_exec" => crate::db::quick_execs::get_quick_exec(conn, id)?.is_some(),
        "artifact" => crate::db::live_pages::get_live_page_summary(conn, id)?.is_some(),
        "skill" => id.starts_with("repository:") || crate::core::skills::get_skill(id).is_some(),
        _ => false,
    })
}

/// The one resource of `scope_project` (`None` = no project) whose slug is
/// `slug`, as publication would derive it from its name.
fn match_by_slug(
    conn: &Connection,
    kind: &str,
    slug: &str,
    scope_project: Option<&str>,
) -> Result<Option<String>> {
    use crate::core::repository_resources::ascii_slug;
    let owned = |owner: Option<&str>| owner == scope_project;
    let candidates: Vec<String> = match kind {
        "workflow" => crate::db::workflows::list_workflows(conn)?
            .into_iter()
            .filter(|item| owned(item.project_id.as_deref()) && ascii_slug(&item.name) == slug)
            .map(|item| item.id)
            .collect(),
        "prompt" => crate::db::quick_prompts::list_quick_prompts(conn)?
            .into_iter()
            .filter(|item| owned(item.project_id.as_deref()) && ascii_slug(&item.name) == slug)
            .map(|item| item.id)
            .collect(),
        "qa" => crate::db::quick_apis::list_quick_apis(conn)?
            .into_iter()
            .filter(|item| owned(item.project_id.as_deref()) && ascii_slug(&item.name) == slug)
            .map(|item| item.id)
            .collect(),
        "qe" => crate::db::quick_execs::list_quick_execs(conn)?
            .into_iter()
            .filter(|item| owned(item.project_id.as_deref()) && ascii_slug(&item.name) == slug)
            .map(|item| item.id)
            .collect(),
        "artifact" => {
            let pages: Vec<_> = crate::db::live_pages::list_live_pages(conn)?
                .into_iter()
                .filter(|page| owned(page.project_id.as_deref()))
                .collect();
            // A page's own slug wins over one derived from its title.
            let by_slug: Vec<String> = pages
                .iter()
                .filter(|page| page.slug == slug)
                .map(|page| page.id.clone())
                .collect();
            if by_slug.is_empty() {
                pages
                    .into_iter()
                    .filter(|page| ascii_slug(&page.title) == slug)
                    .map(|page| page.id)
                    .collect()
            } else {
                by_slug
            }
        }
        "skill" => {
            // The scope's own custom skill (a project's, or a global one), by
            // its id: two of one scope never share it, unlike a name.
            // A published skill's folder already carries the `custom-` prefix.
            let ids = [format!("custom-{slug}"), slug.to_string()];
            let mut found: Vec<String> = ids
                .iter()
                .filter(|id| id.starts_with("custom-"))
                .filter_map(|id| crate::core::skills::get_skill(id))
                .filter(|skill| skill.project_id.as_deref() == scope_project)
                .map(|skill| skill.id)
                .collect();
            if let Some(project_id) = scope_project {
                if let Some(project) = crate::db::projects::get_project(conn, project_id)? {
                    let root = crate::core::scanner::resolve_host_path(&project.path);
                    let present = !slug.contains(['/', '\\'])
                        && slug != "."
                        && slug != ".."
                        && crate::api::projects::resources::PROJECT_SKILL_ROOTS
                            .iter()
                            .filter(|skill_root| {
                                !(**skill_root == ".agents/skills" && slug == "kronn")
                            })
                            .any(|skill_root| {
                                root.join(skill_root).join(slug).join("SKILL.md").is_file()
                            });
                    if present {
                        found.push(format!("repository:{project_id}:{slug}"));
                    }
                }
            }
            found
        }
        _ => Vec::new(),
    };
    match candidates.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(id.clone())),
        _ => anyhow::bail!(
            "`{kind}:{slug}` is ambiguous: {} resources of the same scope have that slug",
            candidates.len()
        ),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    #[test]
    fn upsert_reimporting_the_same_slug_updates_the_same_row_without_a_duplicate() {
        let conn = conn();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-1").unwrap();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-2").unwrap();
        assert_eq!(
            lookup(&conn, "proj-a", "workflow", "triage").unwrap(),
            Some("wf-2".into())
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM resource_identities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 1,
            "reimporting the same slug must not duplicate the row"
        );
    }

    #[test]
    fn upsert_is_scoped_per_project_and_kind() {
        let conn = conn();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-a").unwrap();
        upsert(&conn, "proj-b", "workflow", "triage", "wf-b").unwrap();
        upsert(&conn, "proj-a", "quick_prompt", "triage", "qp-a").unwrap();
        assert_eq!(
            lookup(&conn, "proj-a", "workflow", "triage").unwrap(),
            Some("wf-a".into())
        );
        assert_eq!(
            lookup(&conn, "proj-b", "workflow", "triage").unwrap(),
            Some("wf-b".into())
        );
        assert_eq!(
            lookup(&conn, "proj-a", "quick_prompt", "triage").unwrap(),
            Some("qp-a".into())
        );
        assert_eq!(
            lookup(&conn, "proj-b", "quick_prompt", "triage").unwrap(),
            None
        );
    }

    fn timestamp() -> &'static str {
        "2026-01-01T00:00:00Z"
    }

    pub(crate) fn seed_workflow(conn: &Connection, id: &str, name: &str, project: Option<&str>) {
        let workflow: crate::models::Workflow = serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "project_id": project,
            "trigger": {"type": "Manual"},
            "steps": [{"name": "s", "step_type": {"type": "Agent"}, "prompt_template": "x"}],
            "actions": [], "safety": {}, "workspace_config": null, "concurrency_limit": null,
            "enabled": true, "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::workflows::insert_workflow(conn, &workflow).unwrap();
    }

    pub(crate) fn seed_prompt(conn: &Connection, id: &str, name: &str, project: Option<&str>) {
        let prompt: crate::models::QuickPrompt = serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "icon": "zap", "prompt_template": "x",
            "variables": [], "agent": "ClaudeCode", "project_id": project,
            "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::quick_prompts::insert_quick_prompt(conn, &prompt).unwrap();
    }

    pub(crate) fn seed_api(conn: &Connection, id: &str, name: &str, project: Option<&str>) {
        let api: crate::models::QuickApi = serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "icon": "globe", "project_id": project,
            "api_plugin_slug": "github", "api_config_id": "cfg", "api_endpoint_path": "/user",
            "variables": [], "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::quick_apis::insert_quick_api(conn, &api).unwrap();
    }

    pub(crate) fn seed_exec(conn: &Connection, id: &str, name: &str, project: Option<&str>) {
        let exec: crate::models::QuickExec = serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "icon": "terminal", "project_id": project,
            "command": "cargo", "args": ["check"], "timeout_secs": 30,
            "output_format": "text", "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::quick_execs::insert_quick_exec(conn, &exec).unwrap();
    }

    pub(crate) fn seed_page(
        conn: &Connection,
        id: &str,
        title: &str,
        slug: &str,
        project: Option<&str>,
    ) {
        let now = chrono::Utc::now();
        let page = crate::models::LivePage {
            id: id.into(),
            project_id: project.map(Into::into),
            title: title.into(),
            slug: slug.into(),
            current_revision_id: format!("rev-{id}"),
            data_revision: 0,
            created_at: now,
            updated_at: now,
            last_published_at: None,
            pinned: false,
            archived: false,
        };
        let revision = crate::models::LivePageRevision {
            id: format!("rev-{id}"),
            page_id: id.into(),
            revision: 1,
            html: "<h1>x</h1>".into(),
            created_by_agent: None,
            created_at: now,
        };
        crate::db::live_pages::create_live_page(conn, &page, &revision, &[], None).unwrap();
    }

    pub(crate) fn seed_plugin(conn: &Connection, config_id: &str, server_id: &str) {
        crate::db::mcps::upsert_server(
            conn,
            &crate::models::McpServer {
                id: server_id.into(),
                name: server_id.into(),
                description: String::new(),
                transport: crate::models::McpTransport::Stdio {
                    command: "echo".into(),
                    args: vec![],
                },
                source: crate::models::McpSource::Registry,
                api_spec: None,
            },
        )
        .unwrap();
        crate::db::mcps::insert_config(
            conn,
            &crate::models::McpConfig {
                id: config_id.into(),
                server_id: server_id.into(),
                label: "L".into(),
                env_keys: vec![],
                env_encrypted: "x".into(),
                args_override: None,
                is_global: true,
                config_hash: config_id.into(),
                project_ids: vec![],
                host_sync: crate::models::HostSyncMode::GlobalOnly,
                include_general: true,
            },
        )
        .unwrap();
    }

    fn resolve(conn: &Connection, reference: &str, project: Option<&str>) -> Option<String> {
        resolve_symbolic_reference(conn, reference, project).unwrap()
    }

    #[test]
    fn resolve_symbolic_reference_covers_every_declared_kind() {
        let conn = conn();
        seed_workflow(&conn, "wf-1", "Triage", None);
        seed_prompt(&conn, "qp-1", "Review PR", None);
        seed_exec(&conn, "qe-1", "Lint", None);
        seed_api(&conn, "qa-1", "Fetch", None);
        seed_page(&conn, "page-1", "Team follow-up", "team-follow-up", None);
        assert_eq!(resolve(&conn, "workflow:triage", None), Some("wf-1".into()));
        assert_eq!(
            resolve(&conn, "prompt:review-pr", None),
            Some("qp-1".into())
        );
        assert_eq!(resolve(&conn, "qe:lint", None), Some("qe-1".into()));
        assert_eq!(resolve(&conn, "qa:fetch", None), Some("qa-1".into()));
        assert_eq!(
            resolve(&conn, "artifact:team-follow-up", None),
            Some("page-1".into())
        );
        // Builtin skills are embedded at compile time — no filesystem
        // fixture needed, and no risk of writing under the real config dir.
        assert_eq!(resolve(&conn, "skill:rust", None), Some("rust".into()));
        assert_eq!(resolve(&conn, "skill:does-not-exist", None), None);
        assert_eq!(REFERENCE_KINDS.len(), 7);
    }

    #[test]
    fn an_identity_wins_over_a_name_and_a_dangling_identity_is_skipped() {
        let conn = conn();
        seed_workflow(&conn, "wf-named", "Triage", None);
        seed_workflow(&conn, "wf-imported", "Renamed later", None);
        upsert(&conn, "", "workflow", "triage", "wf-imported").unwrap();
        assert_eq!(
            resolve(&conn, "workflow:triage", None),
            Some("wf-imported".into())
        );
        upsert(&conn, "", "workflow", "triage", "wf-deleted").unwrap();
        assert_eq!(
            resolve(&conn, "workflow:triage", None),
            Some("wf-named".into())
        );
    }

    #[test]
    fn resolve_symbolic_reference_prefers_the_project_then_the_global_scope() {
        let conn = conn();
        let project: crate::models::Project = serde_json::from_value(serde_json::json!({
            "id": "proj-a", "name": "A", "path": "/nonexistent/kronn-proj-a",
            "repo_url": "https://github.com/acme/a",
            "ai_config": {"detected": false, "configs": []},
            "audit_status": "NoTemplate",
            "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::projects::insert_project(&conn, &project).unwrap();
        seed_prompt(&conn, "qp-global", "Review PR", None);
        assert_eq!(
            resolve(&conn, "prompt:review-pr", Some("proj-a")),
            Some("qp-global".into())
        );
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('proj-b','B','/nonexistent/b','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        seed_prompt(&conn, "qp-other", "Review PR", Some("proj-b"));
        seed_prompt(&conn, "qp-local", "Review PR", Some("proj-a"));
        assert_eq!(
            resolve(&conn, "prompt:review-pr", Some("proj-a")),
            Some("qp-local".into())
        );
        assert_eq!(
            resolve(&conn, "prompt:review-pr", None),
            Some("qp-global".into())
        );
    }

    #[test]
    fn a_skill_reference_finds_the_project_repository_skill() {
        let conn = conn();
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join(".agents/skills/block-migration");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: block-migration\n---\nBody",
        )
        .unwrap();
        let project: crate::models::Project = serde_json::from_value(serde_json::json!({
            "id": "proj-s", "name": "S", "path": root.path(),
            "ai_config": {"detected": false, "configs": []},
            "audit_status": "NoTemplate",
            "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::projects::insert_project(&conn, &project).unwrap();
        assert_eq!(
            resolve(&conn, "skill:block-migration", Some("proj-s")),
            Some("repository:proj-s:block-migration".into())
        );
        // Without the project, only the catalogue is searched.
        assert_eq!(resolve(&conn, "skill:block-migration", None), None);
        assert_eq!(resolve(&conn, "skill:../escape", Some("proj-s")), None);
    }

    #[test]
    #[serial_test::serial]
    fn a_skill_reference_resolves_the_project_skill_then_the_repository_then_the_global_one() {
        let data = tempfile::tempdir().unwrap();
        let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
        crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
        let save = |name: &str, project: Option<&str>| {
            crate::core::skills::save_custom_skill(
                name,
                "desc",
                "S",
                &crate::models::SkillCategory::Domain,
                "Body.",
                None,
                None,
                project,
            )
            .unwrap()
        };
        assert_eq!(save("Rev", Some("proj-s")), "custom-rev");
        assert_eq!(save("Glob", None), "custom-glob");
        assert_eq!(save("Other", Some("proj-o")), "custom-other");
        let conn = conn();
        let root = tempfile::tempdir().unwrap();
        let project: crate::models::Project = serde_json::from_value(serde_json::json!({
            "id": "proj-s", "name": "S", "path": root.path(),
            "ai_config": {"detected": false, "configs": []},
            "audit_status": "NoTemplate",
            "created_at": timestamp(), "updated_at": timestamp()
        }))
        .unwrap();
        crate::db::projects::insert_project(&conn, &project).unwrap();

        assert_eq!(
            resolve(&conn, "skill:rev", Some("proj-s")),
            Some("custom-rev".into())
        );
        assert_eq!(
            resolve(&conn, "skill:custom-rev", Some("proj-s")),
            Some("custom-rev".into())
        );
        assert_eq!(
            resolve(&conn, "skill:rev", None),
            None,
            "a project skill is not global"
        );
        assert_eq!(
            resolve(&conn, "skill:other", Some("proj-s")),
            None,
            "nor another project's"
        );
        assert_eq!(
            resolve(&conn, "skill:glob", Some("proj-s")),
            Some("custom-glob".into())
        );
        assert_eq!(
            resolve(&conn, "skill:glob", None),
            Some("custom-glob".into())
        );
        assert_eq!(
            resolve(&conn, "skill:rust", Some("proj-s")),
            Some("rust".into())
        );

        let folder = root.path().join(".agents/skills/rev");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("SKILL.md"), "---\nname: rev\n---\nBody").unwrap();
        let error = resolve_symbolic_reference(&conn, "skill:rev", Some("proj-s")).unwrap_err();
        assert!(error.to_string().contains("ambiguous"), "{error}");

        crate::core::child_env::remove_var("KRONN_DATA_DIR");
        if let Some(value) = previous {
            crate::core::child_env::set_var("KRONN_DATA_DIR", value);
        }
    }

    #[test]
    fn two_resources_with_the_same_slug_in_one_scope_are_ambiguous() {
        let conn = conn();
        seed_workflow(&conn, "wf-1", "Triage", None);
        seed_workflow(&conn, "wf-2", "triage", None);
        let error = resolve_symbolic_reference(&conn, "workflow:triage", None).unwrap_err();
        assert!(error.to_string().contains("ambiguous"), "{error}");
    }

    #[test]
    fn resolve_symbolic_reference_rejects_malformed_or_unknown_input() {
        let conn = conn();
        assert_eq!(resolve(&conn, "not-a-reference", None), None);
        assert_eq!(resolve(&conn, "prompt:", None), None);
        assert_eq!(resolve(&conn, "unknown-kind:slug", None), None);
        assert_eq!(resolve(&conn, "workflow:unknown", None), None);
    }
}
