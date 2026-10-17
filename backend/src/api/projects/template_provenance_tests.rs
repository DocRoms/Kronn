//! Standalone template writers must identify their output before an audit edits it.
use crate::core::doc_provenance::{self, DocWriter};
use std::path::Path;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn verify_generated_then_human_edits(project: &Path) {
    let entry = project.join("docs/AGENTS.md");
    assert!(entry.exists(), "the template writer must create its entry");
    let snapshot = doc_provenance::snapshot_tracked(project).unwrap();
    write(&entry, "# Generated audit entry\nAgent-discovered facts.\n");
    doc_provenance::note_snapshot_writes(project, &snapshot, DocWriter::FullAudit).unwrap();
    assert_eq!(
        doc_provenance::report(project, true).unwrap().to_reconfirm,
        0,
        "editing a newly installed template must not invent a human baseline"
    );

    let human = "# Generated audit entry\nAgent-discovered facts.\n\nHuman note: preserve café.\n";
    write(&entry, human);
    let snapshot = doc_provenance::snapshot_tracked(project).unwrap();
    write(&entry, "# Generated audit entry\nAgent-discovered facts.\n");
    doc_provenance::note_snapshot_writes(project, &snapshot, DocWriter::PartialAudit).unwrap();
    let report = doc_provenance::report(project, true).unwrap();
    assert_eq!(report.to_reconfirm, 1);
    let entry = report
        .docs
        .iter()
        .find(|doc| doc.doc == "docs/AGENTS.md")
        .unwrap();
    assert_eq!(entry.changes[0].before.as_deref(), Some(human));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial(kronn_templates_env)]
async fn standalone_install_records_machine_output_before_the_first_audit() {
    struct RestoreTemplatesEnv(Option<std::ffi::OsString>);
    impl Drop for RestoreTemplatesEnv {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", value),
                None => crate::core::child_env::remove_var("KRONN_TEMPLATES_DIR"),
            }
        }
    }
    let _restore = RestoreTemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let templates = tmp.path().join("templates");
    write(&templates.join("docs/AGENTS.md"), "# Template entry\n");
    crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", &templates);
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let path = project.to_string_lossy().into_owned();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('template-provenance', 'Template provenance', ?1, 'now', 'now')",
            [path],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let state = crate::AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let response = super::template::install_template(
        axum::extract::State(state),
        axum::extract::Path("template-provenance".into()),
    )
    .await
    .0;
    assert!(response.success, "{:?}", response.error);
    verify_generated_then_human_edits(&project);
}

#[test]
fn standalone_reinstall_records_machine_output_before_the_first_audit() {
    let tmp = tempfile::tempdir().unwrap();
    let templates = tmp.path().join("templates/docs");
    write(&templates.join("AGENTS.md"), "# Template entry\n");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join("docs")).unwrap();
    let report = super::template_reinstall::reinstall(&project, &templates, "first", None).unwrap();
    assert_eq!(report.created, vec!["docs/AGENTS.md"]);
    verify_generated_then_human_edits(&project);
}
