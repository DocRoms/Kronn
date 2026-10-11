use super::*;
use tempfile::TempDir;

const HUMAN_OPEN: &str = "<!-- kronn:section name=\"team\" owner=\"human\" -->";
const SECTION_END: &str = "<!-- kronn:section:end -->";

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

/// A two-file template (one nested) and a project that already has `docs/`.
fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let template = tmp.path().join("templates/docs");
    write(&template, "AGENTS.md", "# Template entry\n");
    write(
        &template,
        "architecture/overview.md",
        "# Template overview\n",
    );
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join("docs")).unwrap();
    (tmp, project, template)
}

#[test]
fn a_replaced_file_is_backed_up_with_its_old_bytes_before_the_template_lands() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "damaged {{PROJECT}} entry");
    write(
        &project,
        "docs/architecture/overview.md",
        "# Template overview\n",
    );

    let report = reinstall(&project, &template, "20261011T120000Z", None).unwrap();

    assert_eq!(read(&project, "docs/AGENTS.md"), "# Template entry\n");
    assert_eq!(report.replaced.len(), 1);
    let replaced = &report.replaced[0];
    assert_eq!(replaced.path, "docs/AGENTS.md");
    assert_eq!(
        replaced.backup,
        ".kronn/audit/template-backups/20261011T120000Z/docs/AGENTS.md"
    );
    assert_eq!(
        std::fs::read(project.join(&replaced.backup)).unwrap(),
        "damaged {{PROJECT}} entry".as_bytes(),
        "the backup holds the old bytes"
    );
    assert_eq!(
        report.backup_dir.as_deref(),
        Some(".kronn/audit/template-backups/20261011T120000Z")
    );
    assert_eq!(report.unchanged, vec!["docs/architecture/overview.md"]);
}

#[test]
fn only_the_templates_own_files_are_replaced() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    let others = [
        ("docs/custom-notes.md", "my own notes"),
        ("docs/conventions/agents-md-format-v1.md", "the spec copy"),
        ("README.md", "# readme"),
        ("src/main.rs", "fn main() {}"),
        ("CLAUDE.md", "root adapter"),
    ];
    for (rel, body) in others {
        write(&project, rel, body);
    }

    let report = reinstall(&project, &template, "s1", None).unwrap();

    for (rel, body) in others {
        assert_eq!(
            std::fs::read(project.join(rel)).unwrap(),
            body.as_bytes(),
            "{rel} is not a template file"
        );
    }
    assert_eq!(report.replaced.len(), 1);
    assert_eq!(report.created, vec!["docs/architecture/overview.md"]);
    // Nothing else lands in the backup folder either.
    let backed_up: Vec<_> = walk(&project.join(".kronn/audit/template-backups/s1"));
    assert_eq!(backed_up, vec!["docs/AGENTS.md".to_string()]);
}

fn walk(dir: &Path) -> Vec<String> {
    fn go(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                go(base, &path, out);
            } else {
                out.push(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn a_human_owned_section_is_rewritten_never_kept_back_and_marked_to_reconfirm() {
    let (_tmp, project, template) = fixture();
    let human = format!("{HUMAN_OPEN}\nOur team rules, hand written.\n{SECTION_END}");
    write(
        &project,
        "docs/AGENTS.md",
        &format!("# Old entry {{{{X}}}}\n{human}\nstale tail\n"),
    );
    let plan = plan_reinstall(&project, &template, false).unwrap();
    let entry = plan
        .files
        .iter()
        .find(|f| f.path == "docs/AGENTS.md")
        .unwrap();
    assert_eq!(entry.human_sections, vec!["team"], "notice before writing");

    let report = reinstall(&project, &template, "s1", None).unwrap();

    let after = read(&project, "docs/AGENTS.md");
    assert_eq!(
        after, "# Template entry\n",
        "the reinstall is applied in full"
    );
    assert_eq!(report.to_reconfirm, vec!["docs/AGENTS.md"]);
    let provenance = crate::core::doc_provenance::report(&project, true).unwrap();
    assert_eq!(provenance.to_reconfirm, 1);
    let doc = &provenance.docs[0];
    assert_eq!(
        doc.kronn_writes[0].writer,
        crate::core::doc_provenance::DocWriter::TemplateReinstall
    );
    assert!(doc.changes[0]
        .before
        .as_deref()
        .unwrap()
        .contains("Our team rules"));
    assert!(
        read(&project, ".kronn/audit/template-backups/s1/docs/AGENTS.md").contains("stale tail"),
        "the backup is the whole original"
    );
}

#[test]
fn replacing_a_human_validated_doc_is_announced_then_marked() {
    let (_tmp, project, template) = fixture();
    write(
        &project,
        "docs/AGENTS.md",
        "# Validated entry\nReal content.\n",
    );
    crate::core::doc_provenance::record_all(
        &project,
        crate::core::doc_provenance::ValidationReason::AuditValidation,
    )
    .unwrap();
    let plan = plan_reinstall(&project, &template, false).unwrap();
    let entry = plan
        .files
        .iter()
        .find(|f| f.path == "docs/AGENTS.md")
        .unwrap();
    assert!(entry.human_validated);

    let report = reinstall(
        &project,
        &template,
        "s2",
        Some(&["docs/AGENTS.md".to_string()]),
    )
    .unwrap();
    assert_eq!(read(&project, "docs/AGENTS.md"), "# Template entry\n");
    assert_eq!(report.to_reconfirm, vec!["docs/AGENTS.md"]);
}

#[test]
fn the_plan_lists_what_would_change_and_writes_nothing() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");

    let plan = plan_reinstall(&project, &template, false).unwrap();

    let actions: Vec<_> = plan
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.action))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("docs/AGENTS.md", ReinstallAction::Replace),
            ("docs/architecture/overview.md", ReinstallAction::Create),
        ]
    );
    assert_eq!(read(&project, "docs/AGENTS.md"), "mine {{X}}");
    assert!(!project.join(".kronn").exists());
    assert!(!project.join("docs/architecture").exists());
}

#[test]
fn a_second_reinstall_replaces_and_backs_up_nothing() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    reinstall(&project, &template, "s1", None).unwrap();

    let again = reinstall(&project, &template, "s2", None).unwrap();

    assert!(again.replaced.is_empty() && again.created.is_empty());
    assert_eq!(again.backup_dir, None);
    assert!(!project.join(".kronn/audit/template-backups/s2").exists());
}

#[test]
fn two_runs_in_the_same_second_keep_separate_backup_folders() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "first damage {{X}}");
    reinstall(&project, &template, "same", None).unwrap();
    write(&project, "docs/AGENTS.md", "second damage {{X}}");

    let second = reinstall(&project, &template, "same", None).unwrap();

    assert_eq!(
        second.backup_dir.as_deref(),
        Some(".kronn/audit/template-backups/same-1")
    );
    assert_eq!(
        read(
            &project,
            ".kronn/audit/template-backups/same/docs/AGENTS.md"
        ),
        "first damage {{X}}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_target_refuses_the_whole_reinstall_before_any_write() {
    let (tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    let outside = tmp.path().join("outside.md");
    std::fs::write(&outside, "outside").unwrap();
    std::fs::create_dir_all(project.join("docs/architecture")).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("docs/architecture/overview.md")).unwrap();

    let error = reinstall(&project, &template, "s1", None).unwrap_err();

    assert!(error.contains("symlink"), "{error}");
    assert_eq!(read(&project, "docs/AGENTS.md"), "mine {{X}}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
    assert!(!project.join(".kronn/audit/template-backups").exists());
}

#[test]
fn the_real_template_reinstalls_cleanly_and_is_stable() {
    let tmp = TempDir::new().unwrap();
    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    write(tmp.path(), "docs/AGENTS.md", "# broken {{X}}");
    write(tmp.path(), "docs/coding-rules.md", "");

    let first = reinstall(tmp.path(), &template, "s1", None).unwrap();

    assert_eq!(first.replaced.len(), 2);
    assert!(first.created.len() > 10, "{:?}", first.created);
    let second = reinstall(tmp.path(), &template, "s2", None).unwrap();
    // Only a file whose placeholders were filled in differs from the raw template.
    for file in &second.replaced {
        let source = template.join(file.path.trim_start_matches("docs/"));
        assert!(
            std::fs::read_to_string(source).unwrap().contains("{{"),
            "{} was replaced twice without carrying placeholders",
            file.path
        );
    }
}

const AUDITED: &str = "# Entry\n\nReal audit content, no placeholder.\n";

#[test]
fn the_plan_tells_filled_files_from_damaged_ones() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", AUDITED);
    write(&project, "docs/architecture/overview.md", "   \n");

    let plan = plan_reinstall(&project, &template, false).unwrap();

    let kinds: Vec<_> = plan
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("docs/AGENTS.md", Some(ReinstallKind::Filled)),
            (
                "docs/architecture/overview.md",
                Some(ReinstallKind::DamagedOrTemplate)
            ),
        ]
    );
    // Unbalanced section markers and non-text files are damaged too.
    assert_eq!(
        classify(b"<!-- kronn:section name=\"a\" owner=\"audit\" -->\ntext"),
        ReinstallKind::DamagedOrTemplate
    );
    assert_eq!(
        classify(&[0xff, 0xfe, 0x00]),
        ReinstallKind::DamagedOrTemplate
    );
    assert_eq!(
        classify(b"# Fill {{PROJECT_NAME}}"),
        ReinstallKind::DamagedOrTemplate
    );
    assert_eq!(
        classify(b"# Done\n{ not a placeholder }"),
        ReinstallKind::Filled
    );
}

#[test]
fn documentation_of_placeholder_syntax_in_code_is_not_damage() {
    let fenced = "# Templating\n\n```hbs\nHello {{var}}\n```\n\nDone.\n";
    let marker_example =
        "# Markers\n\n```md\n<!-- kronn:section name=\"a\" owner=\"human\" -->\n```\n";
    let inline = "# Time\n\nUse `{{time.now}}` in the prompt.\n";
    for doc in [fenced, marker_example, inline] {
        assert_eq!(classify(doc.as_bytes()), ReinstallKind::Filled, "{doc}");
    }
    // The real thing, outside code, still counts, even next to a fence.
    for doc in [
        "# Entry\n\nProject: {{PROJECT_NAME}}\n",
        "# Entry\n\n```\nfine\n```\nStill {{X}} here\n",
        "# Entry `code` then {{X}}\n",
        "# Entry\n<!-- kronn:section name=\"a\" owner=\"audit\" -->\ntext\n```\n<!-- kronn:section:end -->\n```\n",
    ] {
        assert_eq!(classify(doc.as_bytes()), ReinstallKind::DamagedOrTemplate, "{doc}");
    }
}

#[test]
fn a_default_reinstall_leaves_a_filled_doc_alone_and_repairs_the_damaged_one() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", AUDITED);
    write(&project, "docs/architecture/overview.md", "broken {{X}}");

    let report = reinstall(&project, &template, "s1", None).unwrap();

    assert_eq!(read(&project, "docs/AGENTS.md"), AUDITED);
    assert_eq!(
        read(&project, "docs/architecture/overview.md"),
        "# Template overview\n"
    );
    assert_eq!(report.skipped, vec!["docs/AGENTS.md"]);
    assert_eq!(report.replaced.len(), 1);
    assert!(!project
        .join(".kronn/audit/template-backups/s1/docs/AGENTS.md")
        .exists());
}

#[test]
fn a_filled_doc_is_replaced_only_when_selected_and_is_backed_up() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", AUDITED);
    write(&project, "docs/architecture/overview.md", "broken {{X}}");

    let selection = vec!["docs/AGENTS.md".to_string()];
    let report = reinstall(&project, &template, "s1", Some(&selection)).unwrap();

    assert_eq!(read(&project, "docs/AGENTS.md"), "# Template entry\n");
    assert_eq!(
        read(&project, ".kronn/audit/template-backups/s1/docs/AGENTS.md"),
        AUDITED
    );
    // The damaged file was not in the explicit list: it stays as it was.
    assert_eq!(
        read(&project, "docs/architecture/overview.md"),
        "broken {{X}}"
    );
    assert_eq!(report.skipped, vec!["docs/architecture/overview.md"]);
}

#[test]
fn a_path_the_plan_does_not_offer_is_refused_before_anything_is_written() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", AUDITED);
    write(&project, "docs/custom-notes.md", "mine");
    for bad in [
        "docs/custom-notes.md",
        "../outside.md",
        "docs/architecture/overview.md",
    ] {
        let selection = vec!["docs/AGENTS.md".to_string(), bad.to_string()];
        let error = reinstall(&project, &template, "s1", Some(&selection)).unwrap_err();
        assert!(error.contains("nothing was touched"), "{bad}: {error}");
    }
    assert_eq!(read(&project, "docs/AGENTS.md"), AUDITED);
    assert_eq!(read(&project, "docs/custom-notes.md"), "mine");
    assert!(!project.join(".kronn/audit/template-backups").exists());
}

async fn state_with_project(root: &Path) -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let path = root.to_string_lossy().to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, datetime('now'), datetime('now'))",
                [path],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_audit_lease_refuses_the_reinstall_and_touches_nothing() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    let state = state_with_project(&project).await;
    assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));

    let response = reinstall_for(&state, "p1".into(), template.clone(), None)
        .await
        .0;

    assert!(!response.success);
    assert_eq!(response.error_code.as_deref(), Some("conflict"));
    assert!(response.error.unwrap().contains("audit is running"));
    assert_eq!(read(&project, "docs/AGENTS.md"), "mine {{X}}");
    assert!(!project.join(".kronn").exists());
    // The plan says so too, so the UI can disable the action.
    let plan = plan_for(&state, "p1".into(), template)
        .await
        .0
        .data
        .unwrap();
    assert!(plan.audit_running);
    // The refused call did not release the audit's own lease.
    assert!(state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reinstall_takes_the_lease_and_gives_it_back() {
    let (_tmp, project, template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    let state = state_with_project(&project).await;

    let response = reinstall_for(&state, "p1".into(), template, None).await.0;

    assert!(response.success, "{:?}", response.error);
    assert_eq!(read(&project, "docs/AGENTS.md"), "# Template entry\n");
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bridge_caller_is_refused_and_the_routes_are_off_the_bridge_list() {
    let (_tmp, project, _template) = fixture();
    write(&project, "docs/AGENTS.md", "mine {{X}}");
    let state = state_with_project(&project).await;
    let caller = || {
        Some(axum::Extension(crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: Some("p1".into()),
            own_discussions: Vec::new(),
            own_run: None,
            agent: None,
        }))
    };

    let run = template_reinstall(State(state.clone()), AxPath("p1".into()), caller(), None)
        .await
        .0;
    let plan = template_reinstall_plan(State(state), AxPath("p1".into()), caller())
        .await
        .0;

    assert!(!run.success && !plan.success);
    assert!(run.error.unwrap().contains("Only a human"));
    assert_eq!(read(&project, "docs/AGENTS.md"), "mine {{X}}");
    assert!(!project.join(".kronn").exists());
    for (method, pattern) in [
        ("POST", "/api/projects/{id}/template-reinstall"),
        ("GET", "/api/projects/{id}/template-reinstall/plan"),
    ] {
        assert!(
            crate::core::bridge_token::route_for(method, pattern).is_none(),
            "{method} {pattern} must stay off the bridge route list"
        );
    }
}
