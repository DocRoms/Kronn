
use super::*;

fn database_with_signal() -> (crate::db::Database, Arc<ResourceChanges>) {
    let database = crate::db::Database::open_in_memory().unwrap();
    let changes = database.resource_changes();
    (database, changes)
}

#[tokio::test]
async fn writing_a_resource_raises_the_signal_and_writing_anything_else_does_not() {
    let (database, changes) = database_with_signal();
    let quiet = changes.raised();

    // A row of a table no rendering is made of.
    database
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TABLE unrelated_rows (value TEXT);
                     INSERT INTO unrelated_rows VALUES ('a');
                     UPDATE unrelated_rows SET value = 'b';
                     DELETE FROM unrelated_rows;",
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();
    assert_eq!(
        changes.raised(),
        quiet,
        "rows of another table are not a resource"
    );

    database
        .with_conn(|conn| {
            crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();
    assert!(changes.raised() > quiet, "a Quick Exec is");
    tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
        .await
        .expect("a waiter is woken by the change");
}

/// The production shape: a file, a write connection and a read companion.
/// The signal comes from the writer, and the reader sees what it announced.
#[tokio::test]
async fn a_database_file_raises_the_signal_and_its_reader_sees_the_change() {
    let directory = tempfile::tempdir().unwrap();
    let database = crate::db::Database::open_path(&directory.path().join("kronn.db")).unwrap();
    let changes = database.resource_changes();
    let quiet = changes.raised();

    database
        .with_conn(|conn| {
            crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();
    assert!(changes.raised() > quiet);
    tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
        .await
        .expect("the writer's change is announced");
    let seen = database
        .with_read_conn(|conn| Ok(crate::db::quick_execs::get_quick_exec(conn, "qe-1")?.is_some()))
        .await
        .unwrap();
    assert!(seen, "and the read connection already holds it");
}

#[tokio::test]
async fn changes_raised_while_nobody_waits_are_not_lost() {
    let (database, changes) = database_with_signal();
    database
        .with_conn(|conn| {
            crate::db::quick_execs::insert_quick_exec(conn, &exec())?;
            crate::db::quick_execs::update_quick_exec(conn, &exec())?;
            crate::db::quick_execs::delete_quick_exec(conn, "qe-1")?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();
    assert!(changes.raised() >= 3, "insert, update and delete all count");
    // Three changes, one wake-up: whoever waits next runs once, not thrice.
    tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
        .await
        .expect("the change made before anyone waited is still there");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), changes.changed())
            .await
            .is_err(),
        "and it is consumed by that wake-up"
    );
}

fn exec() -> crate::models::QuickExec {
    let timestamp = chrono::Utc::now();
    crate::models::QuickExec {
        approved_by_trust: None,
        id: "qe-1".into(),
        name: "Lint".into(),
        icon: "terminal".into(),
        description: String::new(),
        project_id: None,
        command: "cargo".into(),
        args: vec!["check".into()],
        timeout_secs: 30,
        output_format: Default::default(),
        variables: Vec::new(),
        pinned: false,
        created_at: timestamp,
        updated_at: timestamp,
        unmodelled_args_approved: None,
        agent_written: None,
    }
}
