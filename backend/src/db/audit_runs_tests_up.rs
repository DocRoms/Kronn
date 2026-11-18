use super::*;

fn fresh_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    // Project FK is checked at row insert time, so spin up the bare
    // minimum tables.
    conn.execute_batch(
        "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, path TEXT);
             CREATE TABLE audit_runs (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                kind TEXT NOT NULL DEFAULT 'Full',
                agent_type TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'Running',
                td_critical INTEGER NOT NULL DEFAULT 0,
                td_high INTEGER NOT NULL DEFAULT 0,
                td_medium INTEGER NOT NULL DEFAULT 0,
                td_low INTEGER NOT NULL DEFAULT 0,
                td_total INTEGER NOT NULL DEFAULT 0,
                td_resolved_since_last INTEGER NOT NULL DEFAULT 0,
                td_new_since_last INTEGER NOT NULL DEFAULT 0,
                td_carried_over INTEGER NOT NULL DEFAULT 0,
                health_score INTEGER,
                report_path TEXT,
                recommendations_json TEXT,
                last_completed_step INTEGER NOT NULL DEFAULT 0,
                validation_discussion_id TEXT,
                step_outcomes_json TEXT,
                head_sha TEXT,
                branch TEXT,
                source_fingerprint TEXT,
                model TEXT,
                project_profile_draft TEXT,
                validation_completed_at TEXT,
                validation_completed_message_id TEXT,
                cancelled_at_step INTEGER,
                resume_drift TEXT,
                tier TEXT,
                connection_id TEXT,
                documentary_notes TEXT,
                agent_pgid INTEGER,
                agent_started_marker TEXT
            );
            CREATE TABLE audit_step_snapshots (
                audit_run_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                target_file TEXT NOT NULL,
                existed INTEGER NOT NULL,
                content TEXT,
                observed_hash TEXT,
                PRIMARY KEY (audit_run_id, step_index)
            );
            CREATE TABLE audit_run_steps (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                audit_run_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                file_label TEXT NOT NULL,
                started_at DATETIME NOT NULL,
                ended_at DATETIME,
                duration_ms INTEGER,
                step_tokens INTEGER,
                cumulative_tokens INTEGER,
                cli_success INTEGER NOT NULL DEFAULT 1,
                step_warning TEXT,
                step_repaired_from_template INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                cost_usd_micros INTEGER,
                carried_from_run_id TEXT,
                token_accounting TEXT,
                unchanged INTEGER NOT NULL DEFAULT 0,
                accepted_at TEXT,
                accepted_by TEXT,
                accepted_reason TEXT,
                target_state TEXT,
                FOREIGN KEY (audit_run_id) REFERENCES audit_runs(id) ON DELETE CASCADE
            );
            CREATE UNIQUE INDEX idx_audit_run_steps_run
                ON audit_run_steps(audit_run_id, step_index);
            INSERT INTO projects (id, name, path) VALUES ('p1', 'Test', '/tmp/test');",
    )
    .unwrap();
    conn
}

#[test]
fn documentary_failure_invalidates_prior_success_without_losing_usage() {
    let conn = fresh_conn();
    let now = Utc::now();
    insert_running(&conn, "repair", "p1", "Full", "LiteLlm", now).unwrap();
    let attempt = StepTokens {
        input: Some(100),
        output: Some(10),
        cache_read: Some(50),
        cache_write: None,
        ..StepTokens::UNKNOWN
    };
    let combined = attempt.plus(attempt);
    assert_eq!(combined.total(), Some(220), "input as reported");
    assert_eq!(combined.cache_write, None);
    for step in 1..=3 {
        insert_audit_step_start(&conn, "repair", step, "docs/a.md", now).unwrap();
        finalize_audit_step(
            &conn,
            "repair",
            step,
            now,
            500,
            &combined,
            Some(660),
            true,
            None,
            false,
        )
        .unwrap();
    }
    invalidate_documentary_steps(
        &conn,
        "repair",
        &[
            (1, "docs/a.md: invalid line".into()),
            (3, "docs/c.md: invented path".into()),
        ],
    )
    .unwrap();
    let steps = list_audit_steps(&conn, "repair").unwrap();
    assert_eq!(
        crate::api::audit::full::already_succeeded_step_indices(&steps),
        [2].into_iter().collect()
    );
    assert_eq!(steps[0].step_tokens, Some(220));
    assert_eq!(steps[0].duration_ms, Some(500));
    assert!(steps[2]
        .step_warning
        .as_ref()
        .unwrap()
        .contains("invented path"));
    assert_eq!(
        get_by_id(&conn, "repair")
            .unwrap()
            .unwrap()
            .last_completed_step,
        1
    );
}
