//! KT-1629 — the database the Windows end-to-end job upgrades.
//!
//! `tests/fixtures/db-previous-release.sqlite` is a database as the previous
//! release (0.15.2: migrations through `287_quota_fallback_refusals`) left it,
//! WAL mode and a project with a discussion in it. The Windows job boots Kronn
//! on a copy: every later migration is pending, so the boot takes the
//! pre-migration backup path that stopped on os error 5 in KT-1625.
//!
//! The fixture never changes while the code moves on: each release only adds
//! pending migrations to it. To rebuild it (never needed for a new release):
//! `cargo test --lib upgrade_fixture_tests::regenerate -- --ignored`.
#![cfg(all(test, feature = "full-tests"))]

use super::migrations;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// The last migration the previous release shipped.
const PREVIOUS_RELEASE_LAST_MIGRATION: &str = "287_quota_fallback_refusals";
const FIXTURE_PROJECT_ID: &str = "kt1629-fixture-project";

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
    let name: String = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            params![FIXTURE_PROJECT_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "Fixture project");
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
        let now = "2026-11-01T09:00:00+00:00";
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES (?1, 'Fixture project', '/fixture/project', ?2, ?2)",
            params![FIXTURE_PROJECT_ID, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
             VALUES ('kt1629-fixture-discussion', ?1, 'Fixture discussion', ?2, ?2)",
            params![FIXTURE_PROJECT_ID, now],
        )
        .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
