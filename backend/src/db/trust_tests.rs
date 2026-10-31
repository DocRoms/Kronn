//! KT-1492 — storage, resolution (narrowest explicit setting wins) and journal.
use super::*;
use crate::db::migrations;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&connection).unwrap();
    connection
}

fn set(conn: &Connection, scope: TrustScope, id: Option<&str>, level: TrustLevel) {
    set_level(conn, scope, id, level, "Romu").unwrap();
}

#[test]
fn the_default_is_standard_and_not_explicit() {
    let conn = connection();
    let trust = effective(&conn, &TrustOrigin::room("room")).unwrap();
    assert_eq!(trust.level, TrustLevel::Standard);
    assert_eq!(trust.source, TrustScope::Instance);
    assert!(!trust.explicit);
}

#[test]
fn the_instance_level_is_the_default_for_every_room_and_workflow() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    for origin in [
        TrustOrigin::room("room"),
        TrustOrigin::workflow("wf"),
        TrustOrigin::default(),
    ] {
        let trust = effective(&conn, &origin).unwrap();
        assert_eq!(trust.level, TrustLevel::Trusted);
        assert_eq!(trust.source, TrustScope::Instance);
        assert!(trust.explicit);
    }
}

#[test]
fn a_workflow_overrides_the_instance_and_a_room_overrides_both() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Standard);
    set(&conn, TrustScope::Workflow, Some("wf"), TrustLevel::Full);
    let origin = TrustOrigin {
        discussion_ids: vec!["room".into()],
        workflow_id: Some("wf".into()),
        ..TrustOrigin::default()
    };
    let by_workflow = effective(&conn, &origin).unwrap();
    assert_eq!(
        (by_workflow.level, by_workflow.source),
        (TrustLevel::Full, TrustScope::Workflow)
    );
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Trusted);
    let by_room = effective(&conn, &origin).unwrap();
    assert_eq!(
        (by_room.level, by_room.source, by_room.source_id.as_deref()),
        (TrustLevel::Trusted, TrustScope::Room, Some("room"))
    );
}

#[test]
fn an_explicit_standard_on_a_room_beats_a_wider_instance() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Full);
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Standard);
    let trust = effective(&conn, &TrustOrigin::room("room")).unwrap();
    assert_eq!(trust.level, TrustLevel::Standard);
    assert!(grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "x",
        None,
        None
    )
    .unwrap()
    .is_none());
    // Another room still follows the instance.
    assert_eq!(
        effective(&conn, &TrustOrigin::room("other")).unwrap().level,
        TrustLevel::Full
    );
}

#[test]
fn clearing_a_scope_makes_it_inherit_again() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Full);
    assert!(clear_level(&conn, TrustScope::Room, Some("room"), "Romu").unwrap());
    assert!(!clear_level(&conn, TrustScope::Room, Some("room"), "Romu").unwrap());
    assert_eq!(
        effective(&conn, &TrustOrigin::room("room")).unwrap().level,
        TrustLevel::Trusted
    );
}

#[test]
fn a_run_resolves_the_level_of_its_workflow() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at) \
         VALUES ('wf', 'wf', '{}', '[]', ?1, ?1)",
        [&now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, started_at) \
         VALUES ('run', 'wf', 'Running', ?1)",
        [&now],
    )
    .unwrap();
    set(&conn, TrustScope::Workflow, Some("wf"), TrustLevel::Trusted);
    let origin = TrustOrigin {
        workflow_run_id: Some("run".into()),
        ..TrustOrigin::default()
    };
    let trust = effective(&conn, &origin).unwrap();
    assert_eq!(
        (trust.level, trust.source, trust.source_id.as_deref()),
        (TrustLevel::Trusted, TrustScope::Workflow, Some("wf"))
    );
}

#[test]
fn rooms_that_disagree_resolve_to_the_lowest_level() {
    let conn = connection();
    set(&conn, TrustScope::Room, Some("a"), TrustLevel::Full);
    set(&conn, TrustScope::Room, Some("b"), TrustLevel::Trusted);
    let origin = TrustOrigin {
        discussion_ids: vec!["a".into(), "b".into()],
        ..TrustOrigin::default()
    };
    assert_eq!(
        effective(&conn, &origin).unwrap().level,
        TrustLevel::Trusted
    );
}

/// DoD 2: every approval kind is granted at its minimum level and journalled
/// as "granted by trust level X"; below it nothing is granted or journalled.
#[test]
fn each_approval_kind_is_granted_at_its_level_and_journalled() {
    for approval in TrustApproval::ALL {
        let minimum = policy::minimum_level(approval);
        let conn = connection();
        let origin = TrustOrigin::room("room");

        set(&conn, TrustScope::Instance, None, TrustLevel::Standard);
        assert!(grant(&conn, &origin, approval, "subject", None, None)
            .unwrap()
            .is_none());
        let grants_before = grants_logged(&conn);

        if minimum == TrustLevel::Full {
            set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
            assert!(
                grant(&conn, &origin, approval, "subject", None, None)
                    .unwrap()
                    .is_none(),
                "{approval:?} must still ask under Trusted"
            );
        }
        assert_eq!(
            grants_logged(&conn),
            grants_before,
            "{approval:?}: a refusal is not journalled"
        );

        set(&conn, TrustScope::Instance, None, minimum);
        let trust = grant(&conn, &origin, approval, "the subject", Some("p1"), None)
            .unwrap()
            .unwrap_or_else(|| panic!("{approval:?} must be granted from {minimum:?}"));
        assert_eq!(trust.level, minimum);

        let entry = audit_entries(&conn, 50)
            .unwrap()
            .into_iter()
            .find(|entry| entry.event == "grant")
            .unwrap_or_else(|| panic!("{approval:?} must be journalled"));
        assert_eq!(entry.approval, Some(approval));
        assert_eq!(entry.level, minimum);
        assert_eq!(entry.subject, "the subject");
        assert_eq!(entry.discussion_id.as_deref(), Some("room"));
        assert_eq!(entry.project_id.as_deref(), Some("p1"));
        assert!(
            entry
                .detail
                .starts_with(&format!("granted by trust level {}", minimum.as_str())),
            "{}",
            entry.detail
        );
    }
}

fn grants_logged(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM trust_audit_log WHERE event = 'grant'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn a_setting_change_is_journalled_with_who_and_what_it_replaced() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    set(&conn, TrustScope::Instance, None, TrustLevel::Standard);
    let entries = audit_entries(&conn, 10).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.event == "setting"));
    assert_eq!(entries[0].level, TrustLevel::Standard);
    assert!(
        entries[0].detail.contains("by Romu; was trusted"),
        "{}",
        entries[0].detail
    );
}

#[test]
fn the_journal_survives_deleting_the_room_it_names() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions(id, title, created_at, updated_at) VALUES ('room', 'room', ?1, ?1)",
        [&now],
    )
    .unwrap();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "x",
        None,
        None,
    )
    .unwrap();
    conn.execute("DELETE FROM discussions WHERE id = 'room'", [])
        .unwrap();
    assert_eq!(grants_logged(&conn), 1);
}

#[test]
fn a_gate_fails_closed_when_the_setting_cannot_be_read() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Full);
    conn.execute_batch("DROP TABLE trust_settings").unwrap();
    assert!(grant(
        &conn,
        &TrustOrigin::default(),
        TrustApproval::QuickExecLine,
        "x",
        None,
        None
    )
    .is_err());
}

// ── D: lowering a level withdraws what it granted ─────────────────────────

fn quick_exec_row(conn: &Connection, id: &str, approved: bool, by_trust: bool) {
    use crate::models::{CollectQuickExecOutputFormat, QuickExec};
    crate::db::quick_execs::insert_quick_exec(
        conn,
        &QuickExec {
            id: id.into(),
            name: format!("exec {id}"),
            icon: "x".into(),
            description: String::new(),
            project_id: None,
            command: "aws".into(),
            args: vec!["s3".into(), "ls".into(), "{{bucket}}".into()],
            timeout_secs: 30,
            output_format: CollectQuickExecOutputFormat::Json,
            variables: vec![],
            pinned: false,
            unmodelled_args_approved: approved.then_some(true),
            agent_written: Some(true),
            approved_by_trust: by_trust.then_some(true),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        },
    )
    .unwrap();
}

fn approved(conn: &Connection, id: &str) -> (Option<bool>, Option<bool>) {
    let row = crate::db::quick_execs::get_quick_exec(conn, id)
        .unwrap()
        .unwrap();
    (row.unmodelled_args_approved, row.approved_by_trust)
}

fn withdrawn_logged(conn: &Connection) -> Vec<crate::models::TrustAuditEntry> {
    audit_entries(conn, 100)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "withdrawn")
        .collect()
}

#[test]
fn a_grant_names_its_object_and_stays_active_until_withdrawn() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "Quick Exec « exec qe1 »",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();
    let entry = audit_entries(&conn, 10)
        .unwrap()
        .into_iter()
        .find(|entry| entry.event == "grant")
        .unwrap();
    assert_eq!(entry.object_kind.as_deref(), Some("quick_exec"));
    assert_eq!(entry.object_id.as_deref(), Some("qe1"));
    assert!(entry.active);
    assert_eq!(entry.source, TrustScope::Instance);
}

#[test]
fn lowering_the_level_takes_back_the_quick_exec_approval_and_the_journal_lists_it() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "Quick Exec « exec qe1 »",
        Some("p1"),
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();
    assert_eq!(approved(&conn, "qe1"), (Some(true), Some(true)));

    let withdrawn = set_level(
        &conn,
        TrustScope::Instance,
        None,
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();

    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].kind, "quick_exec");
    assert_eq!(withdrawn[0].id, "qe1");
    assert_eq!(
        approved(&conn, "qe1"),
        (None, None),
        "the line waits for a human again"
    );
    let journal = withdrawn_logged(&conn);
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].object_id.as_deref(), Some("qe1"));
    assert_eq!(journal[0].approval, Some(TrustApproval::QuickExecLine));
    assert!(!journal[0].active);
    assert!(
        journal[0].detail.contains("withdrawn by Romu"),
        "{}",
        journal[0].detail
    );
    // The grant itself is no longer active: a second lowering finds nothing.
    assert!(set_level(
        &conn,
        TrustScope::Instance,
        None,
        TrustLevel::Standard,
        "Romu"
    )
    .unwrap()
    .is_empty());
    assert_eq!(withdrawn_logged(&conn).len(), 1);
}

#[test]
fn a_line_a_human_approved_meanwhile_is_not_taken_back() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    // Approved by a human since: no `by trust` mark.
    quick_exec_row(&conn, "qe1", true, false);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "x",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();
    set_level(
        &conn,
        TrustScope::Instance,
        None,
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();
    assert_eq!(approved(&conn, "qe1"), (Some(true), None));
}

#[test]
fn lowering_withdraws_only_what_the_new_level_no_longer_grants() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Full);
    quick_exec_row(&conn, "trusted-one", true, true);
    quick_exec_row(&conn, "full-one", true, true);
    let origin = TrustOrigin::room("room");
    grant(
        &conn,
        &origin,
        TrustApproval::QuickExecLine,
        "a",
        None,
        Some(TrustObject::quick_exec("trusted-one")),
    )
    .unwrap();
    // A Full-only approval, attached to the second object.
    grant(
        &conn,
        &origin,
        TrustApproval::CodeWriteOtherProject,
        "b",
        None,
        Some(TrustObject::quick_exec("full-one")),
    )
    .unwrap();

    let withdrawn = set_level(
        &conn,
        TrustScope::Instance,
        None,
        TrustLevel::Trusted,
        "Romu",
    )
    .unwrap();

    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].approval, TrustApproval::CodeWriteOtherProject);
    assert_eq!(approved(&conn, "trusted-one"), (Some(true), Some(true)));
    assert_eq!(approved(&conn, "full-one"), (None, None));
}

#[test]
fn a_room_override_withdraws_only_what_that_room_granted() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Full);
    quick_exec_row(&conn, "by-instance", true, true);
    quick_exec_row(&conn, "by-room", true, true);
    grant(
        &conn,
        &TrustOrigin::room("other"),
        TrustApproval::QuickExecLine,
        "i",
        None,
        Some(TrustObject::quick_exec("by-instance")),
    )
    .unwrap();
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "r",
        None,
        Some(TrustObject::quick_exec("by-room")),
    )
    .unwrap();

    // Dropping the room's override falls back to Trusted, which still grants it.
    assert!(clear_level(&conn, TrustScope::Room, Some("room"), "Romu").unwrap());
    assert_eq!(approved(&conn, "by-room"), (Some(true), Some(true)));
    // Setting the room to Standard takes back the room's grants only.
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Standard);
    assert_eq!(approved(&conn, "by-room"), (None, None));
    assert_eq!(approved(&conn, "by-instance"), (Some(true), Some(true)));
}

#[test]
fn clearing_a_room_override_withdraws_what_the_inherited_level_does_not_grant() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Standard);
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "r",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();
    assert!(clear_level(&conn, TrustScope::Room, Some("room"), "Romu").unwrap());
    assert_eq!(approved(&conn, "qe1"), (None, None));
    assert_eq!(withdrawn_logged(&conn).len(), 1);
}

#[test]
fn deleting_a_room_takes_its_setting_with_it_whatever_deleted_it() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions(id, title, created_at, updated_at) VALUES ('room', 'room', ?1, ?1)",
        [&now],
    )
    .unwrap();
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "r",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();

    conn.execute("DELETE FROM discussions WHERE id = 'room'", [])
        .unwrap();

    assert_eq!(
        get_setting(&conn, TrustScope::Room, Some("room"))
            .unwrap()
            .level,
        None
    );
    // SQL cannot take an approval back from its object, so the trigger leaves
    // the grant active instead of orphaning it: flipping the flag alone would
    // leave the Quick Exec approved with no grant a revoke could find.
    assert_eq!(approved(&conn, "qe1"), (Some(true), Some(true)));
    assert!(audit_entries(&conn, 50)
        .unwrap()
        .iter()
        .any(|entry| entry.event == "grant" && entry.active));

    // The next sweep judges it against the level now in force and withdraws it.
    withdraw(&conn, "Romu").unwrap();
    assert_eq!(approved(&conn, "qe1"), (None, None));
    assert!(audit_entries(&conn, 50)
        .unwrap()
        .iter()
        .filter(|entry| entry.event == "grant")
        .all(|entry| !entry.active));
    assert_eq!(withdrawn_logged(&conn).len(), 1);
}

#[test]
fn deleting_one_of_several_rooms_keeps_the_grant_the_others_still_justify() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    for room in ["a", "b"] {
        conn.execute(
            "INSERT INTO discussions(id, title, created_at, updated_at) VALUES (?1, ?1, ?2, ?2)",
            params![room, now],
        )
        .unwrap();
        set(&conn, TrustScope::Room, Some(room), TrustLevel::Trusted);
    }
    quick_exec_row(&conn, "qe1", true, true);
    let origin = TrustOrigin {
        discussion_ids: vec!["a".into(), "b".into()],
        ..TrustOrigin::default()
    };
    grant(
        &conn,
        &origin,
        TrustApproval::QuickExecLine,
        "r",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();

    // A grant carrying several rooms is not matched by a `source = 'room'`
    // predicate: deleting room `a` must not deactivate it behind the journal.
    conn.execute("DELETE FROM discussions WHERE id = 'a'", [])
        .unwrap();
    assert!(audit_entries(&conn, 50)
        .unwrap()
        .iter()
        .any(|entry| entry.event == "grant" && entry.active));

    // Room `b` still holds Trusted, so the sweep keeps the grant; lowering `b`
    // then withdraws it, object included.
    withdraw(&conn, "Romu").unwrap();
    assert_eq!(approved(&conn, "qe1"), (Some(true), Some(true)));
    set(&conn, TrustScope::Room, Some("b"), TrustLevel::Standard);
    assert_eq!(approved(&conn, "qe1"), (None, None));
}

#[test]
fn deleting_a_workflow_takes_its_setting_with_it() {
    let conn = connection();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at) \
         VALUES ('wf', 'wf', '{}', '[]', ?1, ?1)",
        [&now],
    )
    .unwrap();
    set(&conn, TrustScope::Workflow, Some("wf"), TrustLevel::Full);
    conn.execute("DELETE FROM workflows WHERE id = 'wf'", [])
        .unwrap();
    assert_eq!(
        get_setting(&conn, TrustScope::Workflow, Some("wf"))
            .unwrap()
            .level,
        None
    );
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM trust_settings", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn forgetting_a_scope_withdraws_its_grants_and_drops_the_setting() {
    let conn = connection();
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "r",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();
    forget_scope(&conn, TrustScope::Room, "room").unwrap();
    assert_eq!(approved(&conn, "qe1"), (None, None));
    assert_eq!(
        get_setting(&conn, TrustScope::Room, Some("room"))
            .unwrap()
            .level,
        None
    );
    assert_eq!(withdrawn_logged(&conn).len(), 1);
}

// ── F: the journal is bounded ─────────────────────────────────────────────

fn journal_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM trust_audit_log", [], |row| row.get(0))
        .unwrap()
}

fn insert_old_rows(conn: &Connection, count: i64, at: &str, active: i64) {
    let mut statement = conn
        .prepare(
            "INSERT INTO trust_audit_log (id, at, event, approval, level, source, subject, detail, active) \
             VALUES (?1, ?2, 'grant', 'quick_exec_line', 'trusted', 'instance', 's', 'd', ?3)",
        )
        .unwrap();
    for _ in 0..count {
        statement
            .execute(params![uuid::Uuid::new_v4().to_string(), at, active])
            .unwrap();
    }
}

#[test]
fn the_journal_keeps_at_most_ten_thousand_rows() {
    let conn = connection();
    let recent = Utc::now().to_rfc3339();
    insert_old_rows(&conn, AUDIT_MAX_ROWS + 50, &recent, 0);
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    assert!(
        journal_rows(&conn) <= AUDIT_MAX_ROWS,
        "{}",
        journal_rows(&conn)
    );
    // The newest line, the one just written, is the one kept.
    assert_eq!(audit_entries(&conn, 1).unwrap()[0].event, "setting");
}

#[test]
fn the_journal_keeps_rows_for_a_hundred_and_eighty_days() {
    let conn = connection();
    let old = (Utc::now() - chrono::Duration::days(AUDIT_MAX_DAYS + 1)).to_rfc3339();
    let recent = (Utc::now() - chrono::Duration::days(AUDIT_MAX_DAYS - 1)).to_rfc3339();
    insert_old_rows(&conn, 3, &old, 0);
    insert_old_rows(&conn, 2, &recent, 0);
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    assert_eq!(
        journal_rows(&conn),
        2 + 1,
        "three expired rows went, the setting line came"
    );
}

#[test]
fn a_grant_that_can_still_be_withdrawn_is_never_pruned() {
    let conn = connection();
    let old = (Utc::now() - chrono::Duration::days(AUDIT_MAX_DAYS + 30)).to_rfc3339();
    insert_old_rows(&conn, 2, &old, 1);
    insert_old_rows(&conn, 2, &old, 0);
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    let active: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM trust_audit_log WHERE active = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 2, "an active grant outlives the retention");
}

// ── Owner review (P1): grants are judged by their origin's level now ──────

#[test]
fn lowering_a_room_withdraws_the_grants_it_inherited_from_the_instance() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    quick_exec_row(&conn, "mine", true, true);
    quick_exec_row(&conn, "other", true, true);
    for (id, room) in [("mine", "room"), ("other", "other-room")] {
        grant(
            &conn,
            &TrustOrigin::room(room),
            TrustApproval::QuickExecLine,
            id,
            None,
            Some(TrustObject::quick_exec(id)),
        )
        .unwrap();
    }

    let withdrawn = set_level(
        &conn,
        TrustScope::Room,
        Some("room"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();

    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].id, "mine");
    assert_eq!(approved(&conn, "mine"), (None, None));
    assert_eq!(approved(&conn, "other"), (Some(true), Some(true)));
}

#[test]
fn a_room_with_its_own_level_keeps_its_grants_when_the_instance_goes_down() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    set(&conn, TrustScope::Room, Some("room"), TrustLevel::Trusted);
    quick_exec_row(&conn, "mine", true, true);
    quick_exec_row(&conn, "other", true, true);
    for (id, room) in [("mine", "room"), ("other", "other-room")] {
        grant(
            &conn,
            &TrustOrigin::room(room),
            TrustApproval::QuickExecLine,
            id,
            None,
            Some(TrustObject::quick_exec(id)),
        )
        .unwrap();
    }

    set(&conn, TrustScope::Instance, None, TrustLevel::Standard);

    assert_eq!(approved(&conn, "mine"), (Some(true), Some(true)));
    assert_eq!(approved(&conn, "other"), (None, None));
}

// ── Owner review (P1-2): a grant keeps every room of its caller ───────────

fn multi_room_origin() -> TrustOrigin {
    TrustOrigin {
        discussion_ids: vec!["room-a".into(), "room-b".into()],
        ..TrustOrigin::default()
    }
}

fn grant_from_two_rooms(conn: &Connection) {
    set(conn, TrustScope::Instance, None, TrustLevel::Trusted);
    quick_exec_row(conn, "mine", true, true);
    grant(
        conn,
        &multi_room_origin(),
        TrustApproval::QuickExecLine,
        "mine",
        None,
        Some(TrustObject::quick_exec("mine")),
    )
    .unwrap();
}

#[test]
fn lowering_the_non_first_room_of_a_multi_room_grant_withdraws_it() {
    let conn = connection();
    grant_from_two_rooms(&conn);
    assert_eq!(approved(&conn, "mine"), (Some(true), Some(true)));

    let withdrawn = set_level(
        &conn,
        TrustScope::Room,
        Some("room-b"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();

    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].id, "mine");
    assert_eq!(approved(&conn, "mine"), (None, None));
}

#[test]
fn lowering_the_first_room_of_a_multi_room_grant_withdraws_it() {
    let conn = connection();
    grant_from_two_rooms(&conn);

    let withdrawn = set_level(
        &conn,
        TrustScope::Room,
        Some("room-a"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();

    assert_eq!(withdrawn.len(), 1);
    assert_eq!(approved(&conn, "mine"), (None, None));
}

#[test]
fn lowering_a_room_outside_the_grants_rooms_keeps_it() {
    let conn = connection();
    grant_from_two_rooms(&conn);

    let withdrawn = set_level(
        &conn,
        TrustScope::Room,
        Some("room-c"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();

    assert!(withdrawn.is_empty());
    assert_eq!(approved(&conn, "mine"), (Some(true), Some(true)));
}

#[test]
fn a_grant_journalled_before_the_room_set_column_falls_back_to_its_one_room() {
    let conn = connection();
    grant_from_two_rooms(&conn);
    conn.execute("UPDATE trust_audit_log SET discussion_ids = NULL", [])
        .unwrap();

    // Only `discussion_id` (the first room) survives on such a row.
    let kept = set_level(
        &conn,
        TrustScope::Room,
        Some("room-b"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();
    assert!(kept.is_empty());
    let withdrawn = set_level(
        &conn,
        TrustScope::Room,
        Some("room-a"),
        TrustLevel::Standard,
        "Romu",
    )
    .unwrap();
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(approved(&conn, "mine"), (None, None));
}

#[test]
fn a_run_grant_keeps_its_rooms_and_follows_its_workflow() {
    let conn = connection();
    set(&conn, TrustScope::Workflow, Some("wf"), TrustLevel::Trusted);
    quick_exec_row(&conn, "mine", true, true);
    let origin = TrustOrigin {
        workflow_id: Some("wf".into()),
        ..multi_room_origin()
    };
    // Rooms without a level of their own: the workflow's level is in force.
    grant(
        &conn,
        &origin,
        TrustApproval::QuickExecLine,
        "mine",
        None,
        Some(TrustObject::quick_exec("mine")),
    )
    .unwrap();

    set(
        &conn,
        TrustScope::Room,
        Some("room-b"),
        TrustLevel::Standard,
    );

    assert_eq!(approved(&conn, "mine"), (None, None));
}

#[test]
fn a_workflow_run_grant_follows_the_level_of_its_workflow() {
    let conn = connection();
    set(&conn, TrustScope::Workflow, Some("wf"), TrustLevel::Trusted);
    quick_exec_row(&conn, "qe1", true, true);
    grant(
        &conn,
        &TrustOrigin::workflow("wf"),
        TrustApproval::QuickExecLine,
        "w",
        None,
        Some(TrustObject::quick_exec("qe1")),
    )
    .unwrap();

    set(
        &conn,
        TrustScope::Workflow,
        Some("wf"),
        TrustLevel::Standard,
    );

    assert_eq!(approved(&conn, "qe1"), (None, None));
}

#[test]
fn a_grant_is_refused_and_nothing_written_when_the_level_no_longer_grants_it() {
    let conn = connection();
    let written = std::cell::Cell::new(false);
    let granted = grant_and_write(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "late",
        None,
        TrustObject::quick_exec("qe1"),
        |_| {
            written.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(!granted);
    assert!(!written.get());
    assert_eq!(grants_logged(&conn), 0);
}

#[test]
fn a_failed_write_rolls_the_grant_back_with_it() {
    let conn = connection();
    set(&conn, TrustScope::Instance, None, TrustLevel::Trusted);
    let result = grant_and_write(
        &conn,
        &TrustOrigin::room("room"),
        TrustApproval::QuickExecLine,
        "boom",
        None,
        TrustObject::quick_exec("qe1"),
        |_| anyhow::bail!("disk full"),
    );
    assert!(result.is_err());
    assert_eq!(grants_logged(&conn), 0);
}
