//! KT-1213: the repository briefing is authoritative, including an empty file.
use super::*;
use crate::{models::AgentType, AppState};
use axum::extract::{Path as AxumPath, State};
use std::{path::Path, sync::Arc};

async fn fixture(root: &Path, notes: Option<&str>) -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let path = root.to_string_lossy().into_owned();
    let notes = notes.map(str::to_string);
    state.db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO projects (id, name, path, briefing_notes, created_at, updated_at) VALUES ('p-briefing', 'Briefing', ?1, ?2, 'now', 'now')",
            rusqlite::params![path, notes],
        )?;
        Ok(())
    }).await.unwrap();
    state
}

#[tokio::test]
async fn briefing_reads_and_project_cards_follow_the_file_even_when_empty() {
    let repo = tempfile::tempdir().unwrap();
    let docs = repo.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let stale = "# Stale DB briefing\n";
    let state = fixture(repo.path(), Some(stale)).await;
    for body in ["# Committed briefing\nÉquipe portable.\n", " \n\t"] {
        std::fs::write(docs.join("briefing.md"), body).unwrap();
        let expected = (!body.trim().is_empty()).then(|| body.to_string());
        let response = briefing::get_briefing(State(state.clone()), AxumPath("p-briefing".into()))
            .await
            .0;
        assert!(response.success);
        assert_eq!(
            response.data,
            Some(expected.clone()),
            "GET /briefing must reflect the file"
        );
        let mut project = state
            .db
            .with_conn(|conn| crate::db::projects::get_project(conn, "p-briefing"))
            .await
            .unwrap()
            .unwrap();
        crate::api::projects::enrich_audit_status_with_runs(&mut project, true);
        assert_eq!(
            project.briefing_notes, expected,
            "audited cards must use the real briefing"
        );
    }
    std::fs::remove_file(docs.join("briefing.md")).unwrap();
    let response = briefing::get_briefing(State(state), AxumPath("p-briefing".into()))
        .await
        .0;
    assert_eq!(
        response.data,
        Some(Some(stale.into())),
        "DB notes remain usable before installation"
    );
}

#[test]
fn briefing_residuals_empty_or_unreadable_file_never_resurrects_db_options() {
    let repo = tempfile::tempdir().unwrap();
    let docs = repo.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let stale = Some(briefing::with_all_kronn_options(
        "# Old\n".into(),
        true,
        &[AgentType::ClaudeCode, AgentType::Codex],
    ));
    for bytes in [b" \n".as_slice(), &[0xff]] {
        std::fs::write(docs.join("briefing.md"), bytes).unwrap();
        assert_eq!(
            crate::api::projects::resolve_briefing_notes(repo.path(), &stale),
            None
        );
    }
    assert_eq!(
        crate::api::projects::resolve_briefing_notes(repo.path(), &Some(" \n".into())),
        None
    );
    std::fs::remove_file(docs.join("briefing.md")).unwrap();
    assert_eq!(
        crate::api::projects::resolve_briefing_notes(repo.path(), &Some(" \n".into())),
        None
    );
    assert_eq!(
        crate::api::projects::resolve_briefing_notes(repo.path(), &stale),
        stale
    );
}

#[test]
fn briefing_residuals_removing_an_option_in_the_file_overrides_stale_db_notes() {
    let checked = briefing::with_all_kronn_options(
        "# Briefing\n".into(),
        true,
        &[AgentType::ClaudeCode, AgentType::Codex],
    );
    for file in ["# Briefing\n", ""] {
        assert!(!td_tasks::briefing_option_set(Some(file), Some(&checked)));
        assert_eq!(
            cross_audit::briefing_cross_agents(Some(file), Some(&checked)),
            None
        );
    }
    assert!(td_tasks::briefing_option_set(None, Some(&checked)));
    assert_eq!(
        cross_audit::briefing_cross_agents(None, Some(&checked)),
        Some(vec![AgentType::ClaudeCode, AgentType::Codex])
    );
}

#[tokio::test]
async fn briefing_residuals_fresh_clone_keeps_the_generated_project_doc_without_answers() {
    let repo = tempfile::tempdir().unwrap();
    let docs = repo.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let body = format!(
        "{} -->\n# Project\n\n## People\nÉquipe: Alice.\nExample variable: {{{{TENANT}}}}.\n",
        project_doc::GENERATED_MARKER
    );
    std::fs::write(docs.join("project.md"), &body).unwrap();
    let state = fixture(repo.path(), None).await;
    for notes in [
        None,
        Some("# Portable briefing\nNo closed answer payload."),
        Some("<!-- kronn:project-answers v1 broken -->"),
    ] {
        let report = project_doc::fill_project_doc_for_audit(
            &state,
            "p-briefing",
            repo.path(),
            Some("https://github.com/example/new-remote"),
            notes,
        )
        .await;
        assert!(!report.written, "{report:?}");
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(
            std::fs::read_to_string(docs.join("project.md")).unwrap(),
            body
        );
    }
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs/project.md"),
        docs.join("project.md"),
    )
    .unwrap();
    let report =
        project_doc::fill_project_doc_for_audit(&state, "p-briefing", repo.path(), None, None)
            .await;
    assert!(
        report.written,
        "a newly installed template still gets its placeholders filled: {report:?}"
    );
    assert_eq!(
        validation::count_raw_placeholders(
            &std::fs::read_to_string(docs.join("project.md")).unwrap()
        ),
        0
    );
}
