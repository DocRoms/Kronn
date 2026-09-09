use super::*;
use serial_test::serial;

/// Point `config_dir()` at a scratch directory for the duration of one test.
fn scratch() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("KRONN_DATA_DIR", dir.path());
    dir
}

fn database() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn
}

#[test]
#[serial]
fn the_operator_gets_a_file_only_they_can_read() {
    let _dir = scratch();
    let conn = database();

    let delivered = bootstrap(&conn).unwrap();
    assert!(delivered.path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&delivered.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the secret must not be readable by anyone else"
        );
    }

    // What the operator reads is what authorises enrolment. If these two ever
    // disagree the install is bricked with a hash claiming otherwise.
    let from_file = read_delivered().unwrap();
    assert!(
        crate::db::human_credentials::authorise_enrolment(&conn, &from_file)
            .unwrap()
            .is_some(),
        "the delivered file must be the secret the database accepts"
    );
}

#[test]
#[serial]
fn nothing_is_committed_when_the_file_cannot_be_written() {
    let dir = scratch();
    let conn = database();

    // The directory exists but the file cannot: a path whose parent is a file.
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    std::env::set_var("KRONN_DATA_DIR", &blocked);

    assert!(bootstrap(&conn).is_err());
    // The forbidden state: a key in the database the operator has no copy of.
    assert!(
        !crate::db::human_credentials::admin_secret_exists(&conn).unwrap(),
        "a secret must not survive in the database when its file was never written"
    );
}

#[test]
#[serial]
fn a_second_bootstrap_is_refused_and_leaves_the_first_intact() {
    let _dir = scratch();
    let conn = database();

    let first = bootstrap(&conn).unwrap();
    let delivered = std::fs::read_to_string(&first.path).unwrap();

    assert!(bootstrap(&conn).is_err());
    // The refusal must not have rewritten the operator's copy on its way out.
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), delivered);
    let from_file = read_delivered().unwrap();
    assert!(
        crate::db::human_credentials::authorise_enrolment(&conn, &from_file)
            .unwrap()
            .is_some()
    );
}

#[test]
#[serial]
fn a_secret_readable_by_others_is_refused_rather_than_used() {
    let _dir = scratch();
    let conn = database();
    let delivered = bootstrap(&conn).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&delivered.path, std::fs::Permissions::from_mode(0o644)).unwrap();
        // Loosened permissions mean somebody else may already have read it.
        // Returning its contents anyway would be pretending that did not happen.
        assert!(read_delivered().is_err());
    }
}

#[test]
#[serial]
fn a_symlink_is_refused_rather_than_followed() {
    let dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();

    #[cfg(unix)]
    {
        let path = secret_path().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "kr-admin-somebody-elses").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

        // Following it would read a file whose permissions belong to whoever
        // planted the link.
        assert!(read_delivered().is_err());
    }
}

#[test]
#[serial]
fn an_empty_file_is_not_a_delivered_secret() {
    let _dir = scratch();
    let conn = database();
    let delivered = bootstrap(&conn).unwrap();
    std::fs::write(&delivered.path, b"").unwrap();
    assert!(read_delivered().is_err());
}

#[test]
#[serial]
fn the_row_records_where_the_operator_can_look_and_not_what_is_there() {
    let _dir = scratch();
    let conn = database();
    let delivered = bootstrap(&conn).unwrap();

    let recorded = crate::db::human_credentials::admin_secret_location(&conn)
        .unwrap()
        .expect("the location is recorded");
    assert_eq!(recorded, delivered.path.to_string_lossy());

    let secret = std::fs::read_to_string(&delivered.path).unwrap();
    assert!(
        !recorded.contains(secret.trim()),
        "the recorded location must be a path, never the secret"
    );
}

#[test]
#[serial]
fn delivery_survives_a_restart_because_the_file_outlives_the_process() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();
    let before = read_delivered().unwrap();

    // A restart is a new connection over the same directory and database file.
    // Nothing about the delivery lives in memory.
    let after = read_delivered().unwrap();
    assert_eq!(before.expose(), after.expose());
}
