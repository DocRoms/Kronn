//! KT-1629 — the database the Windows end-to-end job upgrades.
//!
//! `tests/fixtures/db-previous-release.sqlite` is a database as the previous
//! release (0.15.2: migrations through `287_quota_fallback_refusals`) left it,
//! WAL mode, holding what a real install holds so that a data-dependent
//! migration has rows to rewrite: three projects (one with linked repositories
//! and a docs directory), four discussions with their messages (one archived,
//! one pinned, one in no project), a workflow, two planning tasks with a
//! definition of done and a linked discussion, and one credential row sealed
//! with a test key. The Windows job boots Kronn on a copy: every later
//! migration is pending, so the boot takes the pre-migration backup path that
//! stopped on os error 5 in KT-1625.
//!
//! Nothing in it is a secret: names and paths are invented, and the credential
//! is the sentence [`TEST_CREDENTIAL`] sealed with [`TEST_KEY`], both public in
//! this file. Kronn cannot open that row (its own key differs), which is also
//! the state of a database moved to another machine: the boot must still come
//! up. `the_fixture_holds_no_real_secret` guards the committed bytes.
//!
//! The fixture never changes while the code moves on: each release only adds
//! pending migrations to it. To rebuild it (never needed for a new release),
//! after changing the rows below:
//! `cargo test --lib upgrade_fixture_tests::regenerate -- --ignored`
//! then commit `backend/tests/fixtures/db-previous-release.sqlite`.
#![cfg(all(test, feature = "full-tests"))]

use super::migrations;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// The last migration the previous release shipped.
const PREVIOUS_RELEASE_LAST_MIGRATION: &str = "287_quota_fallback_refusals";
const FIXTURE_PROJECT_ID: &str = "kt1629-fixture-project";
const NOW: &str = "2026-11-01T09:00:00+00:00";

/// A test key, public on purpose: the sealed row proves nothing about secrecy.
const TEST_KEY: [u8; 32] = [0x4b; 32];
const TEST_CREDENTIAL: &str = "kt1629 test credential, not a secret";

const PROJECTS: [(&str, &str, &str); 3] = [
    (FIXTURE_PROJECT_ID, "Fixture project", "/fixture/project"),
    (
        "kt1629-fixture-docs",
        "Fixture docs site",
        "/fixture/docs-site",
    ),
    (
        "kt1629-fixture-library",
        "Fixture library",
        "/fixture/library",
    ),
];

/// (id, project, title, archived, pinned, messages as (role, content)).
type Discussion = (
    &'static str,
    Option<&'static str>,
    &'static str,
    i64,
    i64,
    &'static [(&'static str, &'static str)],
);
const DISCUSSIONS: [Discussion; 4] = [
    (
        "kt1629-fixture-discussion",
        Some(FIXTURE_PROJECT_ID),
        "Fixture discussion",
        0,
        0,
        &[
            ("User", "What does this project do?"),
            ("Agent", "It is an invented project for an upgrade fixture."),
            ("User", "Thanks, and its tests?"),
            ("Agent", "None: there is no code behind it."),
        ],
    ),
    (
        "kt1629-fixture-discussion-docs",
        Some("kt1629-fixture-docs"),
        "Docs structure",
        0,
        1,
        &[
            ("User", "Outline the docs."),
            ("Agent", "An index, a guide and a reference."),
            ("System", "Context refreshed."),
        ],
    ),
    (
        "kt1629-fixture-discussion-archived",
        Some("kt1629-fixture-library"),
        "Old release notes",
        1,
        0,
        &[
            ("User", "Draft the notes."),
            ("Agent", "Done: nothing changed."),
        ],
    ),
    (
        "kt1629-fixture-discussion-global",
        None,
        "A question without a project",
        0,
        0,
        &[("User", "Is this thing on?")],
    ),
];

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/db-previous-release.sqlite")
}

fn applied(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
        .unwrap()
}

fn every_migration() -> i64 {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    applied(&conn)
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// What a workflow row holds. Parsed with the current models before it is
/// written, so a model change that breaks the fixture breaks the regeneration.
fn workflow_steps() -> String {
    let steps = serde_json::json!([{
        "name": "Say hello",
        "step_type": {"type": "Exec"},
        "exec_command": "echo",
        "exec_args": ["hello"]
    }]);
    serde_json::from_value::<Vec<crate::models::WorkflowStep>>(steps.clone())
        .expect("the fixture workflow step parses with the current models");
    steps.to_string()
}

#[test]
fn the_previous_release_database_has_migrations_left_to_apply() {
    let conn = Connection::open(fixture_path()).unwrap();
    let last: String = conn
        .query_row(
            "SELECT name FROM _migrations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(last, PREVIOUS_RELEASE_LAST_MIGRATION);
    assert!(
        applied(&conn) < every_migration(),
        "the fixture must stay older than the code, or the Windows upgrade job \
         stops exercising the pre-migration backup"
    );
}

#[test]
fn the_upgrade_backs_the_previous_release_database_up_and_keeps_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("kronn.db");
    std::fs::copy(fixture_path(), &db).unwrap();

    let conn = Connection::open(&db).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    migrations::run_with_backup(&conn, Some(&db)).unwrap();

    assert!(
        dir.path().join("kronn.db.backup").is_file(),
        "a pending migration must back the database up first"
    );
    assert_eq!(applied(&conn), every_migration());

    // Every kind of row the fixture holds is still there, and still reads with
    // the current models.
    assert_eq!(count(&conn, "projects"), PROJECTS.len() as i64);
    let names: Vec<String> = conn
        .prepare("SELECT name FROM projects ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        names,
        ["Fixture docs site", "Fixture library", "Fixture project"]
    );
    assert_eq!(count(&conn, "discussions"), DISCUSSIONS.len() as i64);
    let messages: usize = DISCUSSIONS.iter().map(|d| d.5.len()).sum();
    assert_eq!(count(&conn, "messages"), messages as i64);
    assert_eq!(count(&conn, "planning_tasks"), 2);
    assert_eq!(count(&conn, "planning_task_dod_items"), 2);
    assert_eq!(count(&conn, "planning_task_discussions"), 1);
    let workflows = crate::db::workflows::list_workflows(&conn).unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].steps.len(), 1);
    // Columns a pending migration added start empty, never invented.
    let seeds: Option<String> = conn
        .query_row(
            "SELECT lane_seeds_json FROM projects WHERE id = ?1",
            params![FIXTURE_PROJECT_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(seeds, None);
    // The sealed credential crossed the upgrade byte for byte.
    let sealed: String = conn
        .query_row(
            "SELECT value_encrypted FROM stored_credentials WHERE id = 'kt1629-fixture-key'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        crate::core::crypto::decrypt(&sealed, &TEST_KEY).unwrap(),
        TEST_CREDENTIAL
    );
}

#[test]
fn the_fixture_holds_no_real_secret() {
    let bytes = std::fs::read(fixture_path()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains(TEST_CREDENTIAL),
        "the credential must be in the file sealed, not in the clear"
    );
    for shape in [
        "sk-ant-",
        "sk-proj-",
        "ghp_",
        "github_pat_",
        "AKIA",
        "-----BEGIN",
        "eyJhbGci",
    ] {
        assert!(
            !text.contains(shape),
            "the fixture contains a `{shape}` secret shape"
        );
    }
    let conn = Connection::open(fixture_path()).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM stored_credentials WHERE kind = 'auth_token'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "no auth token belongs in the fixture");
}

#[test]
#[ignore = "rewrites the committed fixture; run by hand with --ignored"]
fn regenerate_the_previous_release_fixture() {
    let path = fixture_path();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrations::run_through(&conn, PREVIOUS_RELEASE_LAST_MIGRATION).unwrap();

        // Raw SQL against the previous release's schema: the insert helpers write
        // columns a later migration adds (`projects.lane_seeds_json`).
        for (id, name, project_path) in PROJECTS {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id, name, project_path, NOW],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE projects SET repo_url = 'https://example.invalid/fixture.git',
                    briefing_notes = 'An invented project for an upgrade fixture.'
             WHERE id = ?1",
            params![FIXTURE_PROJECT_ID],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET docs_dir = 'documentation',
                    linked_repos_json = '[{\"id\":\"lr-1\",\"name\":\"fixture-design\",\"kind\":\"design\",\"location\":\"/fixture/design\",\"description\":\"design system\"}]'
             WHERE id = 'kt1629-fixture-docs'",
            [],
        )
        .unwrap();

        for (id, project, title, archived, pinned, messages) in DISCUSSIONS {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, participants_json,
                        created_at, updated_at, archived, pinned, message_count)
                 VALUES (?1, ?2, ?3, 'ClaudeCode', 'en', '[\"ClaudeCode\"]', ?4, ?4, ?5, ?6, ?7)",
                params![id, project, title, NOW, archived, pinned, messages.len() as i64],
            )
            .unwrap();
            for (index, (role, content)) in messages.iter().enumerate() {
                let message_id = format!("{id}-m{index}");
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp,
                            sort_order, tokens_used, cost_usd)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        message_id,
                        id,
                        role,
                        content,
                        (*role == "Agent").then_some("ClaudeCode"),
                        NOW,
                        index as i64,
                        if *role == "Agent" { 120_i64 } else { 0 },
                        if *role == "Agent" { 0.0042_f64 } else { 0.0 },
                    ],
                )
                .unwrap();
                if *role == "Agent" {
                    conn.execute(
                        "INSERT INTO message_usage (message_id, input_tokens, output_tokens)
                         VALUES (?1, 90, 30)",
                        params![message_id],
                    )
                    .unwrap();
                }
            }
        }

        conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, enabled,
                    created_at, updated_at)
             VALUES ('kt1629-fixture-workflow', 'Fixture workflow', ?1, '{\"type\":\"Manual\"}', ?2, 1, ?3, ?3)",
            params![FIXTURE_PROJECT_ID, workflow_steps(), NOW],
        )
        .unwrap();

        for (number, id, title, status) in [
            (
                1_i64,
                "kt1629-fixture-task-1",
                "Write the fixture notes",
                "todo",
            ),
            (2, "kt1629-fixture-task-2", "Archive the library", "idea"),
        ] {
            conn.execute(
                "INSERT INTO planning_tasks (id, task_number, title, description, status, priority, rank,
                        created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'An invented task.', ?4, 'normal', ?2, ?5, ?5)",
                params![id, number, title, status, NOW],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO planning_task_dod_items (id, task_id, sentence, position, created_at, updated_at)
                 VALUES (?1, ?2, 'The notes are written.', 0, ?3, ?3)",
                params![format!("{id}-dod"), id, NOW],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO planning_task_projects (task_id, project_id)
             VALUES ('kt1629-fixture-task-1', ?1)",
            params![FIXTURE_PROJECT_ID],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO planning_task_discussions (task_id, discussion_id, is_primary, created_at)
             VALUES ('kt1629-fixture-task-1', 'kt1629-fixture-discussion', 1, ?1)",
            params![NOW],
        )
        .unwrap();

        let sealed = crate::core::crypto::encrypt(TEST_CREDENTIAL, &TEST_KEY).unwrap();
        conn.execute(
            "INSERT INTO stored_credentials (kind, id, name, provider, active, position, value_encrypted)
             VALUES ('provider_key', 'kt1629-fixture-key', 'Fixture key', 'anthropic', 0, 0, ?1)",
            params![sealed],
        )
        .unwrap();

        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
