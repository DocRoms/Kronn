// Audit lifecycle markers: validate (post-audit user signoff) and
// mark-bootstrapped (post-bootstrap-discussion finalization).
//
// 0.8.4 — state is recorded in `docs/.kronn.json` (see
// `core::kronn_state`). Previously we injected `<!-- KRONN:VALIDATED:date -->`
// and `<!-- KRONN:BOOTSTRAPPED:date -->` HTML comments into
// `docs/AGENTS.md`; those polluted the agent prompt and were easy to
// remove "as noise". `detect_audit_status` still reads the legacy markers
// so existing projects keep their badge without re-running anything.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::{kronn_state, scanner};
use crate::models::*;
use crate::AppState;

/// Whether a validation discussion's last Agent message ends on the terminal
/// signal: the gate below and the audit timeline must agree on "finished".
pub(crate) fn validation_discussion_finished(discussion: &Discussion) -> bool {
    discussion
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, MessageRole::Agent))
        .is_some_and(|message| {
            crate::api::discussions::ends_with_terminal_signal(
                &message.content,
                "KRONN:VALIDATION_COMPLETE",
            )
        })
}

/// POST /api/projects/:id/validate-audit
/// Records `validated_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn validate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // The Validated badge asserts a completed pipeline + a FINISHED human
    // validation — earned, never forged (Codex A5 v2). Gate:
    //   1. the project's LATEST run is Completed;
    //   2. it carries the durable `validation_discussion_id` link (076 —
    //      written in the same transaction as the Completed status; a
    //      manually created look-alike discussion satisfies nothing);
    //   3. that discussion's last Agent message ends on the terminal
    //      signal KRONN:VALIDATION_COMPLETE (same shared parser as the
    //      UI — a POST fired before the validation finished is refused).
    // The project lease is held from the gate through mark_validated so no
    // concurrent run can mutate docs between the check and the write.
    if !state
        .audit_tracker
        .lock()
        .map(|mut t| t.try_acquire_lease(&project.id))
        .unwrap_or(false)
    {
        return Json(ApiResponse::err(
            "An audit is currently running on this project — validate once it has finished.",
        ));
    }
    struct LeaseGuard(
        std::sync::Arc<std::sync::Mutex<crate::AuditTracker>>,
        String,
    );
    impl Drop for LeaseGuard {
        fn drop(&mut self) {
            if let Ok(mut t) = self.0.lock() {
                t.release_lease(&self.1);
            }
        }
    }
    let _lease = LeaseGuard(state.audit_tracker.clone(), project.id.clone());

    let pid = project.id.clone();
    let gate = state
        .db
        .with_conn(move |conn| {
            let latest = crate::db::audit_runs::list_recent(conn, &pid, 1)?
                .into_iter()
                .next();
            let mut disc = match latest
                .as_ref()
                .and_then(|r| r.validation_discussion_id.clone())
            {
                Some(disc_id) => crate::db::discussions::get_discussion(conn, &disc_id)?,
                None => None,
            };
            let linked_is_finished = disc.as_ref().is_some_and(validation_discussion_finished);
            if disc.as_ref().is_some_and(|discussion| discussion.archived) && !linked_is_finished {
                let mut statement = conn.prepare(
                    "SELECT id FROM discussions
                     WHERE project_id=?1 AND archived=0 AND title LIKE 'Validation audit%'
                     ORDER BY updated_at DESC, rowid DESC",
                )?;
                let candidate_ids = statement
                    .query_map([&pid], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(statement);
                for candidate_id in candidate_ids {
                    let Some(candidate) =
                        crate::db::discussions::get_discussion(conn, &candidate_id)?
                    else {
                        continue;
                    };
                    if validation_discussion_finished(&candidate) {
                        if let Some(run) = latest.as_ref() {
                            crate::db::audit_runs::set_validation_discussion(
                                conn,
                                &run.id,
                                &candidate.id,
                            )?;
                        }
                        disc = Some(candidate);
                        break;
                    }
                }
            }
            Ok((latest, disc))
        })
        .await;
    let optimize_documents = match gate {
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        Ok((latest, linked_disc)) => {
            let Some(run) = latest else {
                return Json(ApiResponse::err(
                    "No audit run recorded for this project — run the audit first.",
                ));
            };
            if run.status != "Completed" {
                return Json(ApiResponse::err(format!(
                    "The latest audit run is {} — only a Completed run can be validated (resume or re-run it first).",
                    run.status
                )));
            }
            let Some(disc) = linked_disc else {
                return Json(ApiResponse::err(
                    "The latest completed run carries no linked validation discussion — only the discussion the audit itself created (and linked in the same transaction) can validate it.",
                ));
            };
            if disc.project_id.as_deref() != Some(project.id.as_str()) {
                return Json(ApiResponse::err(
                    "The linked validation discussion belongs to another project — refusing.",
                ));
            }
            if !validation_discussion_finished(&disc) {
                return Json(ApiResponse::err(
                    "The validation discussion has not finished — the agent must end on KRONN:VALIDATION_COMPLETE before the project can be marked validated.",
                ));
            }
            run.kind == "Full"
        }
    };

    // Run filesystem I/O on blocking thread pool
    let validate_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = scanner::detect_docs_entry(&project_path);

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found — run the audit first".into());
        }

        if optimize_documents {
            let report = crate::core::document_optimization::analyze_and_write(&project_path)?;
            let blocking: Vec<String> = report
                .blocking_diagnostics()
                .map(|d| format!("{}: {} ({})", d.code, d.message, d.path))
                .collect();
            if !blocking.is_empty() {
                return Err(format!(
                    "Documentary validation failed; KRONN:VALIDATION_COMPLETE is blocked: {}",
                    blocking.join("; ")
                ));
            }
            let unresolved = unresolved_unknowns(&project_path, &report);
            if !unresolved.is_empty() {
                return Err(format!(
                    "Unresolved unknowns remain (`{{{{...}}}}` or `<!-- TODO: ... -->`); the validation must settle them before the project is validated: {}",
                    unresolved.join(", ")
                ));
            }
        }

        kronn_state::mark_validated(&project_path)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Documents Kronn owns that still carry a placeholder or a `TODO:` marker.
/// The validation's first phase settles every such unknown; TD sheets and
/// dated reports have their own review and are not counted here.
fn unresolved_unknowns(
    project_path: &std::path::Path,
    report: &crate::core::document_optimization::DocumentaryOptimizationReport,
) -> Vec<String> {
    let mut paths: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "placeholder")
        .filter(|d| !d.path.starts_with("docs/tech-debt/") && !d.path.starts_with("docs/reports/"))
        .filter(|d| crate::core::document_optimization::kronn_owns(project_path, &d.path))
        .map(|d| d.path.clone())
        .collect();
    paths.dedup();
    paths
}

/// POST /api/projects/:id/mark-bootstrapped
/// Records `bootstrapped_at` in `docs/.kronn.json` and refreshes the project's audit status.
pub async fn mark_bootstrapped(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = scanner::detect_docs_entry(&project_path);

        if !index_file.exists() {
            return Err("docs/AGENTS.md not found".into());
        }

        kronn_state::mark_bootstrapped(&project_path)
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

#[cfg(test)]
mod validate_gate_tests {
    use super::*;
    use axum::extract::{Path as AxPath, State};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn test_state() -> AppState {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Seed a project whose path is a real tempdir with a docs entry, plus
    /// an audit run and (optionally) a linked validation discussion whose
    /// last Agent message is `last_agent_msg`.
    async fn seed(
        state: &AppState,
        tmp: &std::path::Path,
        run_status: &str,
        link: Option<&str>,
        last_agent_msg: Option<&str>,
    ) {
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::write(tmp.join("docs/AGENTS.md"), "# proj docs\n").unwrap();
        let path = tmp.to_str().unwrap().to_string();
        let link = link.map(|s| s.to_string());
        let msg = last_agent_msg.map(|s| s.to_string());
        let status = run_status.to_string();
        state.db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            crate::db::audit_runs::insert_running(
                conn, "r1", "p1", "Full", "ClaudeCode", chrono::Utc::now(),
            )?;
            if status == "Completed" {
                crate::db::audit_runs::complete(
                    conn, "r1", chrono::Utc::now(), "Completed",
                    0, 0, 0, 0, 0, 0, 0, 100, None, None,
                )?;
            } else if status == "Interrupted" {
                crate::db::audit_runs::update_last_completed_step(conn, "r1", 3)?;
                crate::db::audit_runs::mark_interrupted(conn, "r1", "test")?;
            }
            if let Some(disc_id) = link.as_deref() {
                conn.execute(
                    "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES (?1, 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                    [disc_id],
                )?;
                if let Some(m) = msg.as_deref() {
                    conn.execute(
                        "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
                         VALUES ('m1', ?1, 'Agent', ?2, datetime('now'), 0)",
                        rusqlite::params![disc_id, m],
                    )?;
                }
                crate::db::audit_runs::set_validation_discussion(conn, "r1", disc_id)?;
            }
            Ok(())
        }).await.unwrap();
    }

    async fn call(state: &AppState) -> ApiResponse<AiAuditStatus> {
        validate_audit(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_latest_completed_linked_and_finished() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Tout est validé.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        let resp = call(&state).await;
        assert!(resp.success, "gate must pass: {:?}", resp.error);
        assert!(
            !state.audit_tracker.lock().unwrap().leased.contains("p1"),
            "the lease must be released after validation"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unresolved_unknowns_block_validation_until_settled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        let docs = tmp.path().join("docs");
        std::fs::write(
            docs.join("AGENTS.md"),
            "# Projet\n\nSee [glossary](glossary.md) and [notes](legacy/notes.md).\n",
        )
        .unwrap();
        std::fs::write(
            docs.join("glossary.md"),
            "# Glossaire\n\n<!-- TODO: ask user — what does « lot » mean? -->\n",
        )
        .unwrap();
        // A TD sheet's own unknowns and a human legacy note do not count.
        std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
        std::fs::write(
            docs.join("tech-debt/TD-20260901-x.md"),
            "# TD\n<!-- TODO: verify -->\n",
        )
        .unwrap();
        std::fs::create_dir_all(docs.join("legacy")).unwrap();
        std::fs::write(
            docs.join("legacy/notes.md"),
            "# Notes\n<!-- TODO: later -->\n",
        )
        .unwrap();

        let refused = call(&state).await;
        assert!(!refused.success);
        let error = refused.error.unwrap();
        assert!(error.contains("docs/glossary.md"), "{error}");
        assert!(
            !error.contains("tech-debt") && !error.contains("legacy"),
            "{error}"
        );
        assert!(crate::core::kronn_state::read(tmp.path()).is_none_or(|s| s.validated_at.is_none()));

        std::fs::write(
            docs.join("glossary.md"),
            "# Glossaire\n\n- **lot**: unknown.\n",
        )
        .unwrap();
        let validated = call(&state).await;
        assert!(validated.success, "{:?}", validated.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn specialized_validation_does_not_apply_full_document_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        std::fs::write(tmp.path().join("docs/AGENTS.md"), "# {{BROKEN_TEMPLATE}}\n").unwrap();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE audit_runs SET kind = 'Security' WHERE id = 'r1'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = call(&state).await;
        assert!(
            response.success,
            "specialized validation: {:?}",
            response.error
        );
        assert!(!tmp
            .path()
            .join(crate::core::document_optimization::REPORT_FILE)
            .exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lookalike_unlinked_discussion_is_refused() {
        // A manually created "Validation audit ..." discussion NOT linked to
        // the run satisfies nothing (the old title/date heuristic is gone).
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        state.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('forged', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            Ok(())
        }).await.unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp
            .error
            .unwrap()
            .contains("no linked validation discussion"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn linked_but_unfinished_or_midtext_signal_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("mentioning KRONN:VALIDATION_COMPLETE mid-sentence and going on"),
        )
        .await;
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("has not finished"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_latest_run_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Interrupted", None, None).await;
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("only a Completed run"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn newer_interrupted_run_supersedes_an_old_validated_completed() {
        // The ordering regression the list_recent(...,1) gate must lock:
        // an old Completed run — fully linked and signaled — must NOT
        // validate once a NEWER Interrupted attempt exists (its mutation
        // made the old validation stale).
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                crate::db::audit_runs::insert_running(
                    conn,
                    "r2",
                    "p1",
                    "Full",
                    "ClaudeCode",
                    chrono::Utc::now() + chrono::Duration::minutes(5),
                )?;
                crate::db::audit_runs::update_last_completed_step(conn, "r2", 2)?;
                crate::db::audit_runs::mark_interrupted(conn, "r2", "cut")?;
                Ok(())
            })
            .await
            .unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(
            resp.error.unwrap().contains("only a Completed run"),
            "the newer Interrupted attempt must gate out the stale Completed"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn linked_discussion_owned_by_another_project_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(&state, tmp.path(), "Completed", None, None).await;
        state.db.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p2', 'Other', '/tmp/other', ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('d-foreign', 'p2', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                [],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, tokens_used)
                 VALUES ('m1', 'd-foreign', 'Agent', 'KRONN:VALIDATION_COMPLETE', datetime('now'), 0)",
                [],
            )?;
            crate::db::audit_runs::set_validation_discussion(conn, "r1", "d-foreign")?;
            Ok(())
        }).await.unwrap();
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("another project"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archived_unfinished_validation_can_finish_in_a_replacement_discussion() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d-archived"),
            Some("Validation still pending"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE discussions SET archived=1 WHERE id='d-archived'",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at)
                     VALUES ('d-resumed', 'p1', 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now', '+1 second'))",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, tokens_used)
                     VALUES ('m-resumed', 'd-resumed', 'Agent', 'Reprise terminée.\nKRONN:VALIDATION_COMPLETE', datetime('now'), 0)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = call(&state).await;
        assert!(
            response.success,
            "replacement must validate: {:?}",
            response.error
        );
        let linked = state
            .db
            .with_conn(|conn| {
                Ok(crate::db::audit_runs::get_by_id(conn, "r1")?
                    .and_then(|run| run.validation_discussion_id))
            })
            .await
            .unwrap();
        assert_eq!(linked.as_deref(), Some("d-resumed"));
    }

    async fn timeline_validation(
        state: &AppState,
    ) -> Option<crate::api::audit::run::AuditTimelineValidation> {
        crate::api::audit::run::audit_timeline(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
            .data
            .expect("timeline")
            .latest_validation
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn archived_finished_validation_is_reported_by_the_timeline_and_validates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("Fin.\nKRONN:VALIDATION_COMPLETE"),
        )
        .await;
        state
            .db
            .with_conn(|conn| {
                conn.execute("UPDATE discussions SET archived=1 WHERE id='d1'", [])?;
                Ok(())
            })
            .await
            .unwrap();

        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert_eq!(reported.discussion_id, "d1");
        assert!(reported.finished && reported.archived);
        let response = call(&state).await;
        assert!(response.success, "{:?}", response.error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeline_reports_an_unfinished_validation_and_none_for_an_interrupted_run() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE puis la suite"),
        )
        .await;
        let reported = timeline_validation(&state)
            .await
            .expect("linked validation");
        assert!(!reported.finished && !reported.archived);

        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Interrupted",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        assert!(timeline_validation(&state).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_lease_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = test_state();
        seed(
            &state,
            tmp.path(),
            "Completed",
            Some("d1"),
            Some("KRONN:VALIDATION_COMPLETE"),
        )
        .await;
        assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));
        let resp = call(&state).await;
        assert!(!resp.success);
        assert!(resp.error.unwrap().contains("currently running"));
    }
}
