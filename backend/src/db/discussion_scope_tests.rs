//! KT-1270 — room grants: journal, replacement, requests and decisions.
use super::*;
use crate::db::migrations;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&connection).unwrap();
    let now = Utc::now().to_rfc3339();
    for project in ["p1", "p2", "p3"] {
        connection
            .execute(
                "INSERT INTO projects(id, name, path, created_at, updated_at) VALUES (?1, ?1, ?1, ?2, ?2)",
                params![project, now],
            )
            .unwrap();
    }
    for (room, project) in [("room", Some("p1")), ("general", None)] {
        connection
            .execute(
                "INSERT INTO discussions(id, title, project_id, created_at, updated_at) VALUES (?1, ?1, ?2, ?3, ?3)",
                params![room, project, now],
            )
            .unwrap();
    }
    connection
}

fn room() -> Vec<String> {
    vec!["room".to_owned()]
}

#[test]
fn a_grant_opens_its_project_and_a_revoke_closes_it_for_good() {
    let conn = connection();
    let read = grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Read,
        ScopeGrantSource::Link,
        "romu",
    )
    .unwrap();
    let granted = granted_projects(&conn, &room()).unwrap();
    assert!(granted.read.contains("p2") && granted.write.is_empty());

    revoke(&conn, "room", &read.id, "romu", Some("done")).unwrap();
    assert_eq!(
        granted_projects(&conn, &room()).unwrap(),
        GrantedProjects::default()
    );
    assert!(
        revoke(&conn, "room", &read.id, "romu", None).is_err(),
        "no un-revoke, no double revoke"
    );

    let scope = scope_of(&conn, "room").unwrap().unwrap();
    assert!(scope.grants.is_empty());
    assert_eq!(
        scope.revoked.len(),
        1,
        "the journal keeps the revoked grant"
    );
    assert_eq!(scope.revoked[0].revoked_reason.as_deref(), Some("done"));
}

#[test]
fn a_new_grant_on_the_same_project_replaces_the_old_one_in_the_journal() {
    let conn = connection();
    grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Read,
        ScopeGrantSource::Link,
        "romu",
    )
    .unwrap();
    grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Write,
        ScopeGrantSource::Link,
        "romu",
    )
    .unwrap();
    let granted = granted_projects(&conn, &room()).unwrap();
    assert!(granted.write.contains("p2") && granted.read.contains("p2"));
    let scope = scope_of(&conn, "room").unwrap().unwrap();
    assert_eq!(scope.grants.len(), 1);
    assert_eq!(scope.revoked[0].revoked_reason.as_deref(), Some("replaced"));
}

#[test]
fn grants_refuse_the_primary_project_a_general_room_and_an_unknown_project() {
    let conn = connection();
    for (room, project) in [
        ("room", Some("p1")),
        ("general", Some("p2")),
        ("room", Some("nope")),
    ] {
        assert!(
            grant(
                &conn,
                room,
                project,
                ScopeAccess::Read,
                ScopeGrantSource::Link,
                "romu"
            )
            .is_err(),
            "{room} {project:?}"
        );
    }
    assert!(grant(
        &conn,
        "room",
        None,
        ScopeAccess::Read,
        ScopeGrantSource::Link,
        "romu"
    )
    .is_err());
    assert!(grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Read,
        ScopeGrantSource::All,
        "romu"
    )
    .is_err());
}

#[test]
fn every_project_is_one_wildcard_grant() {
    let conn = connection();
    grant(
        &conn,
        "room",
        None,
        ScopeAccess::Read,
        ScopeGrantSource::All,
        "romu",
    )
    .unwrap();
    assert_eq!(
        granted_projects(&conn, &room()).unwrap().all,
        Some(ScopeAccess::Read)
    );
    grant(
        &conn,
        "room",
        None,
        ScopeAccess::Write,
        ScopeGrantSource::All,
        "romu",
    )
    .unwrap();
    let granted = granted_projects(&conn, &room()).unwrap();
    assert_eq!(granted.all, Some(ScopeAccess::Write));
    assert_eq!(scope_of(&conn, "room").unwrap().unwrap().grants.len(), 1);
}

#[test]
fn a_request_stays_pending_until_a_human_decides() {
    let conn = connection();
    let Filed::New(request) = file_request(
        &conn,
        "room",
        "p2",
        ScopeAccess::Write,
        "read the API tasks",
        "@claude",
    )
    .unwrap() else {
        panic!("expected a new request");
    };
    assert_eq!(
        granted_projects(&conn, &room()).unwrap(),
        GrantedProjects::default(),
        "filing grants nothing"
    );
    assert!(matches!(
        file_request(&conn, "room", "p2", ScopeAccess::Write, "again", "@claude").unwrap(),
        Filed::AlreadyPending(_)
    ));
    assert!(decide(&conn, "room", &request.id, Some(ScopeAccess::Write), "romu").is_ok());
    let granted = granted_projects(&conn, &room()).unwrap();
    assert!(granted.write.contains("p2"));
    assert!(
        decide(&conn, "room", &request.id, None, "romu").is_err(),
        "decided once"
    );
    assert!(matches!(
        file_request(&conn, "room", "p2", ScopeAccess::Read, "x", "@claude").unwrap(),
        Filed::AlreadyGranted
    ));
}

#[test]
fn a_human_may_narrow_a_request_never_widen_it() {
    let conn = connection();
    let Filed::New(read) =
        file_request(&conn, "room", "p2", ScopeAccess::Read, "look", "@claude").unwrap()
    else {
        panic!()
    };
    assert!(decide(&conn, "room", &read.id, Some(ScopeAccess::Write), "romu").is_err());
    let Filed::New(write) =
        file_request(&conn, "room", "p3", ScopeAccess::Write, "edit", "@claude").unwrap()
    else {
        panic!()
    };
    let (decided, grant) =
        decide(&conn, "room", &write.id, Some(ScopeAccess::Read), "romu").unwrap();
    assert_eq!(decided.granted_access, Some(ScopeAccess::Read));
    assert_eq!(grant.unwrap().access, ScopeAccess::Read);
    let (declined, none) = decide(&conn, "room", &read.id, None, "romu").unwrap();
    assert_eq!(declined.status, ScopeRequestStatus::Declined);
    assert!(none.is_none());
    assert!(!granted_projects(&conn, &room())
        .unwrap()
        .read
        .contains("p2"));
}

#[test]
fn pending_requests_are_capped_per_room() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    for index in 0..MAX_PENDING_REQUESTS {
        let project = format!("extra-{index}");
        conn.execute(
            "INSERT INTO projects(id, name, path, created_at, updated_at) VALUES (?1, ?1, ?1, ?2, ?2)",
            params![project, now],
        )
        .unwrap();
        file_request(&conn, "room", &project, ScopeAccess::Read, "why", "@claude").unwrap();
    }
    assert!(file_request(&conn, "room", "p2", ScopeAccess::Read, "why", "@claude").is_err());
}

#[test]
fn a_request_from_another_room_cannot_be_decided_here() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions(id, title, project_id, created_at, updated_at) VALUES ('other', 'other', 'p1', ?1, ?1)",
        [&now],
    )
    .unwrap();
    let Filed::New(request) =
        file_request(&conn, "other", "p2", ScopeAccess::Read, "why", "@claude").unwrap()
    else {
        panic!()
    };
    assert!(decide(&conn, "room", &request.id, Some(ScopeAccess::Read), "romu").is_err());
}

#[test]
fn only_a_write_grant_lets_a_room_host_another_project_s_run() {
    let conn = connection();
    assert!(!room_writes(&conn, "room", "p2").unwrap());
    grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Read,
        ScopeGrantSource::Link,
        "romu",
    )
    .unwrap();
    assert!(!room_writes(&conn, "room", "p2").unwrap());
    grant(
        &conn,
        "room",
        Some("p2"),
        ScopeAccess::Write,
        ScopeGrantSource::Link,
        "romu",
    )
    .unwrap();
    assert!(room_writes(&conn, "room", "p2").unwrap());
    assert!(!room_writes(&conn, "room", "p3").unwrap());
    grant(
        &conn,
        "room",
        None,
        ScopeAccess::Write,
        ScopeGrantSource::All,
        "romu",
    )
    .unwrap();
    assert!(room_writes(&conn, "room", "p3").unwrap());
}
