use super::*;
use serial_test::serial;

/// Point `config_dir()` at a scratch directory for the duration of one test.
struct Scratch {
    dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl Scratch {
    fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("KRONN_DATA_DIR", value),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
    }
}

fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("KRONN_DATA_DIR");
    std::env::set_var("KRONN_DATA_DIR", dir.path());
    Scratch { dir, previous }
}

#[test]
#[serial]
fn scratch_restores_the_environment_even_after_a_test_changes_its_path() {
    let before = std::env::var_os("KRONN_DATA_DIR");
    {
        let dir = scratch();
        assert_eq!(std::env::var_os("KRONN_DATA_DIR").unwrap(), dir.path());
        std::env::set_var("KRONN_DATA_DIR", dir.path().join("blocked"));
    }
    assert_eq!(std::env::var_os("KRONN_DATA_DIR"), before);
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

// ── Rotation and recovery ───────────────────────────────────────────────────

#[test]
#[serial]
fn rotation_moves_the_secret_and_the_old_one_stops_working() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();
    let old = read_delivered().unwrap();

    let delivered = rotate(&conn).unwrap();
    let new = read_delivered().unwrap();

    assert_ne!(
        old.expose(),
        new.expose(),
        "rotation that returns the same secret has rotated nothing"
    );
    assert!(
        crate::db::human_credentials::authorise_enrolment(&conn, &new)
            .unwrap()
            .is_some(),
        "the file must hold the secret the database now accepts"
    );
    assert!(
        crate::db::human_credentials::authorise_enrolment(&conn, &old)
            .unwrap()
            .is_none(),
        "a rotated-away secret that still enrols has not been rotated away"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&delivered.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the replacement is as private as the original");
    }
}

#[test]
#[serial]
fn rotation_costs_the_credentials_nothing() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();
    let admin = read_delivered().unwrap();

    let authority = crate::db::human_credentials::authorise_enrolment(&conn, &admin)
        .unwrap()
        .unwrap();
    let (_credential, grant) = crate::db::human_credentials::enrol(
        &conn,
        &authority,
        crate::db::human_credentials::GrantRole::Human,
        "Romu — laptop",
    )
    .unwrap();

    rotate(&conn).unwrap();

    // Rotating the bootstrap moves the power to enrol NEW credentials. It is
    // not a revocation sweep: an operator rotating a secret they still hold
    // must not discover they have logged out every device they enrolled.
    assert!(
        crate::db::human_credentials::authenticate(&conn, &grant).is_ok(),
        "an enrolled grant must survive a rotation of the secret that made it"
    );
}

#[test]
#[serial]
fn there_is_nothing_to_rotate_before_there_is_a_secret() {
    let _dir = scratch();
    let conn = database();
    assert!(
        rotate(&conn).is_err(),
        "rotating nothing must not quietly mint a first secret: that is the bootstrap's job, \
         and it refuses a second"
    );
    assert!(!secret_path().unwrap().exists());
}

#[test]
#[serial]
fn a_rotation_that_cannot_write_the_row_leaves_the_operators_copy_alone() {
    let dir = scratch();
    let db_path = dir.path().join("rotate.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    crate::db::migrations::run(&conn).unwrap();
    let delivered = bootstrap(&conn).unwrap();
    let before = std::fs::read_to_string(&delivered.path).unwrap();

    // Another writer holds the database, so the UPDATE cannot land. The
    // rotation must fail BEFORE it touches the only copy the operator has —
    // a failed rotation that takes the current secret with it turns a working
    // install into a dead one.
    //
    // WAL and BEGIN IMMEDIATE, not BEGIN EXCLUSIVE, and that difference is the
    // whole test. An exclusive lock stops the READS too, so the rotation failed
    // on its very first line and this passed while proving nothing: it never
    // reached the code it is named after.
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    let blocker = rusqlite::Connection::open(&db_path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();

    // The road the guard sits on: reading still works, so what fails below is
    // the write, after the transaction is open and one line before the file.
    assert!(
        crate::db::human_credentials::admin_secret_exists(&conn).unwrap(),
        "the read side must still work, or this test proves nothing"
    );

    assert!(rotate(&conn).is_err());
    assert_eq!(
        std::fs::read_to_string(&delivered.path).unwrap(),
        before,
        "the operator's copy must be exactly what it was"
    );
}

#[test]
#[serial]
fn no_request_means_no_rotation() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();
    let before = read_delivered().unwrap();

    assert_eq!(recover_if_requested(&conn).unwrap(), None);
    // Every boot calls this. One that rotated on its own would invalidate the
    // secret the operator wrote down, on every restart.
    assert_eq!(read_delivered().unwrap().expose(), before.expose());
}

#[test]
#[serial]
fn an_operator_who_lost_the_file_gets_a_new_secret_back() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();

    // The scenario this exists for: the row is in the database, the file is
    // gone, and nothing on the HTTP surface can help — the secret is the thing
    // no route may hand back.
    let path = secret_path().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(read_delivered().is_err());

    std::fs::write(recovery_request_path().unwrap(), b"").unwrap();
    let recovered = recover_if_requested(&conn).unwrap().expect("delivered");
    assert_eq!(recovered, path);

    let secret = read_delivered().unwrap();
    assert!(
        crate::db::human_credentials::authorise_enrolment(&conn, &secret)
            .unwrap()
            .is_some(),
        "recovery that does not restore the ability to enrol has recovered nothing"
    );
}

#[test]
#[serial]
fn a_recovery_request_is_consumed_so_the_next_boot_is_quiet() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();

    let request = recovery_request_path().unwrap();
    std::fs::write(&request, b"").unwrap();
    recover_if_requested(&conn).unwrap().expect("delivered");
    assert!(!request.exists(), "the request must not outlive its answer");

    let after = read_delivered().unwrap();
    assert_eq!(recover_if_requested(&conn).unwrap(), None);
    assert_eq!(
        read_delivered().unwrap().expose(),
        after.expose(),
        "a consumed request must not rotate again on the next boot"
    );
}

#[test]
#[serial]
fn recovery_bootstraps_an_install_that_never_did() {
    let _dir = scratch();
    let conn = database();
    std::fs::write(recovery_request_path().unwrap(), b"").unwrap();

    // Nothing to rotate, so recovery is the bootstrap this install never got.
    let path = recover_if_requested(&conn).unwrap().expect("delivered");
    assert_eq!(path, secret_path().unwrap());
    assert!(crate::db::human_credentials::admin_secret_exists(&conn).unwrap());
}

#[test]
#[serial]
fn a_request_that_is_not_a_regular_file_is_refused_and_left_in_place() {
    let _dir = scratch();
    let conn = database();
    bootstrap(&conn).unwrap();
    let before = read_delivered().unwrap();

    let request = recovery_request_path().unwrap();
    std::fs::create_dir(&request).unwrap();

    // A directory where a request should be is a surprise, not an instruction.
    assert!(recover_if_requested(&conn).is_err());
    assert!(
        request.is_dir(),
        "the surprise is left for the operator to see"
    );
    assert_eq!(read_delivered().unwrap().expose(), before.expose());
}
