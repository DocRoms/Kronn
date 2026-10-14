use super::*;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let template = tmp.path().join("templates/docs");
    write(
        &template,
        "AGENTS.md",
        "<!-- kronn:doc-version=\"1.1\" -->\n# Template",
    );
    write(
        &project,
        "docs/AGENTS.md",
        "<!-- kronn:doc-version=\"1.0\" -->\n# Filled project",
    );
    (tmp, project, template)
}

#[test]
fn legacy_audited_project_gets_new_files_and_current_owning_steps_without_invented_renames() {
    let (_tmp, project, template) = fixture();
    write(&template, "environments.md", "{{ENVIRONMENTS}}");
    write(&template, "glossary.md", "{{GLOSSARY}}");
    write(&template, "operations/mcp-servers.md", "{{MCP_SERVERS}}");
    let update = check(&project, &template).unwrap();
    assert!(update.available);
    assert_eq!(update.installed_version.as_deref(), Some("1.0"));
    assert_eq!(update.current_version.as_deref(), Some("1.1"));
    assert!(update.renames.is_empty());
    let chain = crate::api::audit::steps_for(crate::models::AuditKind::Full, &DocsRoot::default());
    for path in ["docs/environments.md", "docs/operations/mcp-servers.md"] {
        let missing = update
            .missing_files
            .iter()
            .find(|f| f.path == path)
            .unwrap();
        assert_eq!(
            missing.step,
            chain
                .iter()
                .position(|s| s.target_file == path)
                .map(|i| i + 1)
        );
        assert_eq!(missing.step, None, "unowned docs must not invent a step");
    }
    assert_eq!(
        update
            .missing_files
            .iter()
            .find(|f| f.path == "docs/glossary.md")
            .unwrap()
            .step,
        Some(2)
    );
}

#[test]
fn install_records_upstream_bytes_and_does_not_flag_filled_human_edits() {
    let (_tmp, project, template) = fixture();
    record_install(&project, &template).unwrap();
    write(
        &project,
        "docs/AGENTS.md",
        "# Human content with Vue {{ value }}",
    );
    let state = kronn_state::read(&project).unwrap();
    assert_eq!(
        state.template.as_ref().unwrap().version.as_deref(),
        Some("1.1")
    );
    assert_eq!(
        state.template.as_ref().unwrap().files["AGENTS.md"],
        crate::core::docs_merge::sha256_hex(&std::fs::read(template.join("AGENTS.md")).unwrap())
    );
    assert!(!check(&project, &template).unwrap().available);
    write(&template, "environments.md", "{{ENVIRONMENTS}}");
    assert!(
        check(&project, &template).unwrap().available,
        "new files matter even when the version was not bumped"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("docs/AGENTS.md")).unwrap(),
        "# Human content with Vue {{ value }}"
    );
}

#[test]
fn proven_rename_preserves_local_content_and_an_occupied_destination() {
    let (_tmp, project, template) = fixture();
    write(&template, "old.md", "# Upstream content");
    write(&project, "docs/old.md", "# Local human additions — 日本語");
    record_install(&project, &template).unwrap();
    std::fs::rename(template.join("old.md"), template.join("new.md")).unwrap();
    let report = check(&project, &template).unwrap();
    assert_eq!(
        report.renames,
        vec![TemplateRename {
            from: "docs/old.md".into(),
            to: "docs/new.md".into(),
            conflict: false
        }]
    );
    apply_renames(&project, &template).unwrap();
    assert!(!project.join("docs/old.md").exists());
    assert_eq!(
        std::fs::read_to_string(project.join("docs/new.md")).unwrap(),
        "# Local human additions — 日本語"
    );
    write(&project, "docs/old.md", "other human text");
    assert!(check(&project, &template).unwrap().renames[0].conflict);
    apply_renames(&project, &template).unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("docs/old.md")).unwrap(),
        "other human text"
    );
}

#[test]
fn ambiguous_rename_is_reported_and_survives_recording_without_deletion() {
    let (_tmp, project, template) = fixture();
    write(&template, "old.md", "same");
    write(&project, "docs/old.md", "human");
    record_install(&project, &template).unwrap();
    std::fs::remove_file(template.join("old.md")).unwrap();
    write(&template, "one.md", "same");
    write(&template, "two.md", "same");
    let report = check(&project, &template).unwrap();
    assert!(report.renames.is_empty());
    assert_eq!(report.unresolved_files, vec!["docs/old.md"]);
    record_install(&project, &template).unwrap();
    assert_eq!(
        check(&project, &template).unwrap().unresolved_files,
        vec!["docs/old.md"]
    );
    assert_eq!(
        std::fs::read_to_string(project.join("docs/old.md")).unwrap(),
        "human"
    );
}

#[test]
fn owned_spec_updates_but_arbitrary_human_edits_survive() {
    let (_tmp, project, template) = fixture();
    record_install(&project, &template).unwrap();
    let spec = project.join("docs").join(SPEC_PATH);
    let mut old = kronn_state::read(&project).unwrap();
    std::fs::write(&spec, "previous upstream spec").unwrap();
    old.template.as_mut().unwrap().installed_spec_sha256 = Some(
        crate::core::docs_merge::sha256_hex(b"previous upstream spec"),
    );
    kronn_state::write(&project, &mut old).unwrap();
    record_install(&project, &template).unwrap();
    assert_eq!(
        std::fs::read_to_string(&spec).unwrap(),
        crate::core::anti_halluc::SPEC_AGENTS_MD_V1
    );
    std::fs::write(&spec, "Human convention additions").unwrap();
    record_install(&project, &template).unwrap();
    assert_eq!(
        std::fs::read_to_string(&spec).unwrap(),
        "Human convention additions"
    );
    assert!(check(&project, &template).unwrap().spec_modified);
}

#[test]
fn corrupt_state_and_forged_manifest_paths_are_refused() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/.kronn.json", "{ corrupt");
    assert!(record_install(&project, &template).is_err());
    assert_eq!(
        std::fs::read_to_string(project.join("docs/.kronn.json")).unwrap(),
        "{ corrupt"
    );
    write(
        &project,
        "docs/.kronn.json",
        r#"{"template":{"version":"1.0","files":{"../../outside.md":"hash"}}}"#,
    );
    assert!(check(&project, &template).is_err());
}

#[test]
fn relocated_root_uses_project_relative_paths_and_matching_steps() {
    let (_tmp, project, template) = fixture();
    crate::core::docs_location::set_project_override(project.to_str().unwrap(), Some(".ai"));
    write(&template, "glossary.md", "{{GLOSSARY}}");
    let report = check(&project, &template).unwrap();
    assert!(report
        .missing_files
        .iter()
        .any(|f| f.path == ".ai/glossary.md" && f.step.is_some()));
    record_install(&project, &template).unwrap();
    assert!(project.join(".ai/.kronn.json").is_file());
}

#[test]
fn an_exact_legacy_bundled_spec_is_upgraded_without_adopting_human_edits() {
    let (_tmp, project, template) = fixture();
    let legacy = include_str!("fixtures/agents-md-format-v1-legacy.md");
    write(&project, "docs/conventions/agents-md-format-v1.md", legacy);
    assert!(!check(&project, &template).unwrap().spec_modified);
    record_install(&project, &template).unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("docs").join(SPEC_PATH)).unwrap(),
        crate::core::anti_halluc::SPEC_AGENTS_MD_V1
    );
    write(
        &project,
        "docs/conventions/agents-md-format-v1.md",
        &format!("{legacy}\nHuman additions"),
    );
    record_install(&project, &template).unwrap();
    assert!(
        std::fs::read_to_string(project.join("docs").join(SPEC_PATH))
            .unwrap()
            .ends_with("Human additions")
    );
}

#[test]
fn version_mismatch_with_identical_catalogue_and_no_missing_files_is_reported() {
    let (_tmp, project, template) = fixture();
    record_install(&project, &template).unwrap();
    let mut state = kronn_state::read(&project).unwrap();
    state.template.as_mut().unwrap().version = Some("1.0".into());
    kronn_state::write(&project, &mut state).unwrap();
    let update = check(&project, &template).unwrap();
    assert!(update.missing_files.is_empty());
    assert!(update.available);
}

#[test]
fn audit_metadata_preserves_spec_ownership_without_installing_files() {
    let (_tmp, project, template) = fixture();
    let recorded = for_audit(&project, &template, None).unwrap();
    assert_eq!(recorded.version.as_deref(), Some("1.1"));
    assert_eq!(recorded.installed_spec_sha256, None);
    assert!(!project.join("docs").join(SPEC_PATH).exists());
}

#[test]
fn a_rename_outside_the_selected_additions_stays_put() {
    let (_tmp, project, template) = fixture();
    write(&template, "old.md", "upstream");
    write(&project, "docs/old.md", "human");
    record_install(&project, &template).unwrap();
    std::fs::rename(template.join("old.md"), template.join("new.md")).unwrap();
    apply_renames_in(&project, &template, Some(&[])).unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("docs/old.md")).unwrap(),
        "human"
    );
    assert!(!project.join("docs/new.md").exists());
}
