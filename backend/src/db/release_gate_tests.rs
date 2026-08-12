//! 0.9.2-F — Release gate: stateful integration tests that exercise the durable
//! transitions of the reliability core (B clock, C queue, D revise, G presence)
//! TOGETHER, under restart / double-submit / cancel / rich-DB migration.
//!
//! Individual invariants are unit-tested in their own modules (agent_dispatch,
//! discussions, discussion_sessions); WS reconnect is covered on the frontend
//! (`useWebSocket.test.ts` + `ws-reconnect.spec.ts`). This gate asserts they
//! hold on a POPULATED database — the release condition being zero lost or
//! duplicated message/job. Run: `cargo test --lib release_gate`.
#![cfg(test)]

use super::agent_dispatch::{self, DispatchStatus};
use super::{discussion_sessions, discussions, migrations};
use crate::models::{AgentType, DiscussionMessage, MessageRole};
use chrono::Utc;
use rusqlite::{params, Connection};

fn gate_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p1', 'Gate', '/tmp/gate', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
         VALUES ('d1', 'p1', 'Gate disc', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn
}

fn message(id: &str, role: MessageRole, agent: Option<AgentType>) -> DiscussionMessage {
    DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        model: None,
        lint_report: None,
        id: id.to_string(),
        role,
        channel: crate::models::MessageChannel::Main,
        content: format!("content of {id}"),
        agent_type: agent,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
    }
}

fn user_msg(id: &str) -> DiscussionMessage {
    message(id, MessageRole::User, None)
}

fn agent_msg(id: &str) -> DiscussionMessage {
    message(id, MessageRole::Agent, Some(AgentType::Codex))
}

/// DoD #1 — a durable dispatch obligation survives a mid-flight process death:
/// the Running job returns to Pending on restart (attempts preserved), the
/// drain sees it again, and the triggering message is intact. No orphan, no
/// loss.
#[test]
fn durable_dispatch_survives_restart_and_redrains_without_orphan() {
    let conn = gate_db();
    discussions::insert_message_with_dispatch(&conn, "d1", &agent_msg("m1"), "j1").unwrap();
    // Mid-flight: the worker claimed it (Running) when the process died.
    agent_dispatch::claim(&conn, "j1").unwrap().unwrap();
    assert_eq!(
        agent_dispatch::get(&conn, "j1").unwrap().unwrap().status,
        DispatchStatus::Running
    );

    // Restart recovery (wired at main.rs boot): Running -> Pending, attempts kept.
    assert_eq!(
        agent_dispatch::reset_running_after_restart(&conn).unwrap(),
        1
    );
    let recovered = agent_dispatch::get(&conn, "j1").unwrap().unwrap();
    assert_eq!(recovered.status, DispatchStatus::Pending);
    assert_eq!(recovered.attempts, 1, "the spent attempt must be preserved");

    // The drain scan picks it up again — the obligation is not orphaned.
    assert!(agent_dispatch::list_runnable_ids(&conn, 16)
        .unwrap()
        .contains(&"j1".to_string()));
    // And the triggering message survived untouched.
    let msgs: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages WHERE id = 'm1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(msgs, 1);
}

/// DoD #2 — a double-submitted user turn (same message id, e.g. a client retry
/// after a 502) collapses to a single message and a single dispatch job.
#[test]
fn double_submitted_user_turn_is_idempotent() {
    let conn = gate_db();
    let msg = user_msg("u1");
    let first =
        discussions::insert_user_message_with_dispatch(&conn, "d1", &msg, "j1", None).unwrap();
    let discussions::InsertUserMessageOutcome::Inserted {
        sort_order: inserted_so,
        ..
    } = first
    else {
        panic!("the first submit must insert");
    };
    // Retry with the SAME message id but a fresh job id.
    let retry =
        discussions::insert_user_message_with_dispatch(&conn, "d1", &msg, "j2", None).unwrap();
    let discussions::InsertUserMessageOutcome::Duplicate { sort_order: dup_so } = retry else {
        panic!("a re-submitted user turn must be recognised as a duplicate");
    };
    assert_eq!(
        dup_so, inserted_so,
        "the duplicate submit must return the SAME receipt (sort_order), not a new one"
    );

    let msgs: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages WHERE id = 'u1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(msgs, 1, "exactly one message persisted");
    assert!(agent_dispatch::get(&conn, "j1").unwrap().is_some());
    assert!(
        agent_dispatch::get(&conn, "j2").unwrap().is_none(),
        "the duplicate submit must NOT create a second dispatch job"
    );
}

/// DoD #3 — a job cancelled in-flight stays cancelled across a restart: recovery
/// must never resurrect it, so a stopped run is not silently re-executed.
#[test]
fn cancelled_job_is_never_resurrected_by_restart() {
    let conn = gate_db();
    discussions::insert_message_with_dispatch(&conn, "d1", &agent_msg("m1"), "j1").unwrap();
    agent_dispatch::claim(&conn, "j1").unwrap().unwrap();
    conn.execute(
        "UPDATE agent_dispatch_jobs SET status = 'Cancelled' WHERE id = 'j1'",
        [],
    )
    .unwrap();

    // Restart: only Running jobs requeue; Cancelled is sticky.
    agent_dispatch::reset_running_after_restart(&conn).unwrap();
    assert_eq!(
        agent_dispatch::get(&conn, "j1").unwrap().unwrap().status,
        DispatchStatus::Cancelled
    );
    assert!(
        !agent_dispatch::list_runnable_ids(&conn, 16)
            .unwrap()
            .contains(&"j1".to_string()),
        "a cancelled job must not be runnable"
    );
}

/// DoD #1 (revise arm) — a durable REVISE obligation survives a mid-flight
/// restart: the revised content and its revision-event row persist, and the
/// revise's dispatch job requeues Running -> Pending (revise claims its job
/// inline, so it is Running immediately after the call).
#[test]
fn revise_dispatch_survives_restart() {
    let conn = gate_db();
    // The last user turn, then revise it with a local dispatch.
    discussions::insert_message(&conn, "d1", &user_msg("u1")).unwrap();
    // Read the stored timestamp back so the CAS `expected_revision` matches
    // exactly, independent of the on-disk datetime format.
    let expected_revision: String = conn
        .query_row("SELECT timestamp FROM messages WHERE id = 'u1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let dispatches = [discussions::UserDispatchSpec {
        job_id: "jr1",
        agent_override: None,
    }];
    discussions::revise_message_with_dispatch(
        &conn,
        discussions::ReviseMessageParams {
            discussion_id: "d1",
            message_id: "u1",
            content: "revised body",
            expected_revision: &expected_revision,
            idempotency_key: "idem-revise-1",
            targets: &[],
            dispatches: &dispatches,
        },
    )
    .unwrap();
    assert_eq!(
        agent_dispatch::get(&conn, "jr1").unwrap().unwrap().status,
        DispatchStatus::Running,
        "revise claims its dispatch inline"
    );

    // Restart mid-flight.
    agent_dispatch::reset_running_after_restart(&conn).unwrap();

    let content: String = conn
        .query_row("SELECT content FROM messages WHERE id = 'u1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(content, "revised body", "the revised content survived");
    let events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_revision_events WHERE target_message_id = 'u1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(events, 1, "the revision-event row persists across restart");
    assert_eq!(
        agent_dispatch::get(&conn, "jr1").unwrap().unwrap().status,
        DispatchStatus::Pending,
        "the revise obligation requeued, not lost"
    );
}

/// DoD #1 (defer arm) — a DEFERRED runtime obligation is durable across restart:
/// it stays Pending (never resurrected as Running, never lost) and keeps its
/// runtime_unavailable reason, so it re-drains when a runtime returns.
#[test]
fn deferred_obligation_survives_restart() {
    let conn = gate_db();
    discussions::insert_message_with_dispatch(&conn, "d1", &agent_msg("m1"), "j1").unwrap();
    agent_dispatch::claim(&conn, "j1").unwrap().unwrap();
    agent_dispatch::defer_runtime_unavailable(&conn, "j1", 30, "runtime_unavailable: absent")
        .unwrap();
    assert_eq!(
        agent_dispatch::get(&conn, "j1").unwrap().unwrap().status,
        DispatchStatus::Pending,
        "defer moves the job back to Pending"
    );

    // Restart must leave the Pending obligation exactly as-is (only Running requeues).
    agent_dispatch::reset_running_after_restart(&conn).unwrap();
    let after = agent_dispatch::get(&conn, "j1").unwrap().unwrap();
    assert_eq!(
        after.status,
        DispatchStatus::Pending,
        "the deferred obligation survives the restart"
    );
    assert!(
        after
            .last_error
            .as_deref()
            .unwrap_or_default()
            .starts_with("runtime_unavailable"),
        "the runtime-unavailable reason is preserved for re-drain"
    );
}

/// DoD #4 (db-observable half) — the message clock is strictly monotone, so a
/// client reconnecting and re-fetching with `since_sort_order` can never see a
/// duplicate or a gap. (The WS transport reconnect itself is covered on the
/// frontend: `useWebSocket.test.ts` + `ws-reconnect.spec.ts`.)
#[test]
fn sort_order_is_strictly_monotone_for_dup_free_delta_resync() {
    let conn = gate_db();
    let mut last = -1_i64;
    for i in 0..12 {
        let so = discussions::insert_message(&conn, "d1", &user_msg(&format!("u{i}"))).unwrap();
        assert!(
            so > last,
            "sort_order must strictly increase (delta resync relies on it)"
        );
        last = so;
    }
}

/// DoD #5 — migrations are idempotent and lossless on a RICH populated DB. This
/// is the real boot path: every start re-checks migrations against a database
/// full of prior data. Re-running must be a no-op that preserves every row and
/// keeps the newest (086) columns usable.
#[test]
fn migrations_are_idempotent_and_preserve_a_rich_database() {
    let conn = gate_db();
    // Seed data across the reliability tables.
    for i in 0..20 {
        discussions::insert_message(&conn, "d1", &user_msg(&format!("seed{i}"))).unwrap();
    }
    discussion_sessions::create_session(&conn, "d1", "Codex", Some("s1"), "peer").unwrap();
    discussion_sessions::create_session(&conn, "d1", "ClaudeCode", Some("s2"), "peer").unwrap();
    discussions::insert_message_with_dispatch(&conn, "d1", &agent_msg("mj"), "j1").unwrap();

    let counts_before = table_counts(&conn);
    // The boot-time re-migration path.
    migrations::run(&conn).unwrap();
    let counts_after = table_counts(&conn);
    assert_eq!(
        counts_before, counts_after,
        "re-running migrations on a populated DB must not add, drop or alter rows"
    );

    // The 086 presence columns remain usable after the re-run.
    let views = discussion_sessions::list_participant_views(&conn, "d1").unwrap();
    assert_eq!(views.len(), 2, "both seeded sessions still surface");
}

/// DoD #5 (the real upgrade) — build the schema at 081 (before B/C/D/G), seed
/// data using ONLY pre-082 columns, then run 082→086 through the real migration
/// path and assert: pre-migration rows survive, `next_message_seq` backfills
/// from existing sort_orders (082), the durable-queue (083) and revision (085)
/// tables appear, and the presence columns (086) exist with their defaults.
#[test]
fn migrations_082_to_086_upgrade_a_rich_pre_migration_database() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "081_planning_tasks").unwrap();
    // The 082-086 objects must not exist yet — proves we are genuinely pre-082.
    assert!(!column_exists(&conn, "discussions", "next_message_seq"));
    assert!(!table_exists(&conn, "agent_dispatch_jobs"));
    assert!(!column_exists(&conn, "discussion_sessions", "write_state"));

    // Seed rich data at the 081 schema (raw SQL, pre-082 columns only).
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p1', 'P', '/tmp/p', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
         VALUES ('d1', 'p1', 'D', ?1, ?1)",
        params![now],
    )
    .unwrap();
    for i in 1..=5_i64 {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
             VALUES (?1, 'd1', 'User', ?2, ?3, ?4)",
            params![format!("m{i}"), format!("content {i}"), now, i],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO discussion_sessions (disc_id, agent_type, session_id, role, status, joined_at)
         VALUES ('d1', 'Codex', 's1', 'peer', 'active', ?1)",
        params![now],
    )
    .unwrap();

    // Run the remaining migrations (082→086) through the REAL path.
    migrations::run(&conn).unwrap();

    // Pre-migration rows survived.
    let msgs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = 'd1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(msgs, 5, "all pre-migration messages survived the upgrade");
    // 082 backfilled next_message_seq = MAX(sort_order) + 1 = 6.
    let seq: i64 = conn
        .query_row(
            "SELECT next_message_seq FROM discussions WHERE id = 'd1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        seq, 6,
        "082 backfilled the sequence from existing sort_orders"
    );
    // 083 queue + 085 revisions tables now exist.
    assert!(table_exists(&conn, "agent_dispatch_jobs"));
    assert!(table_exists(&conn, "message_revision_events"));
    // 086 presence columns exist with defaults and the row reads back cleanly.
    assert!(column_exists(&conn, "discussion_sessions", "write_state"));
    let views = discussion_sessions::list_participant_views(&conn, "d1").unwrap();
    assert_eq!(
        views.len(),
        1,
        "the pre-migration session survives + is readable"
    );
    assert_eq!(
        views[0].write_state,
        discussion_sessions::WriteState::Unknown,
        "086 default (unknown write-state) applied to the pre-existing session"
    );
}

/// DoD #5 (regression) — a dev DB stuck at 087 (before the decision columns
/// existed) must gain them from 088. This is the exact bug the H dogfood caught:
/// the columns were first (wrongly) added by editing 087 after it was applied,
/// which the runner never re-runs; 088 ALTERs them in so both a 087-only DB and
/// a fresh 086→087→088 upgrade converge.
#[test]
fn migration_088_adds_decision_columns_to_an_existing_087_db() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "087_planning_proposals").unwrap();
    assert!(
        table_exists(&conn, "planning_proposal_items"),
        "087 created the items table"
    );
    assert!(
        !column_exists(&conn, "planning_proposal_items", "decision_idempotency_key"),
        "087 alone must NOT carry the decision columns"
    );
    assert!(!column_exists(
        &conn,
        "planning_proposal_items",
        "receipt_message_id"
    ));

    // Apply the remaining migration(s) — 088 ALTERs the columns in.
    migrations::run(&conn).unwrap();
    assert!(
        column_exists(&conn, "planning_proposal_items", "decision_idempotency_key"),
        "088 adds the idempotency key column to a 087-only DB"
    );
    assert!(column_exists(
        &conn,
        "planning_proposal_items",
        "receipt_message_id"
    ));
}

#[test]
fn migration_092_versions_and_deduplicates_open_cli_session_bindings() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "091_mcp_preferred_interface").unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at, source_agent, source_session_id)
         VALUES ('source-a', 'A', ?1, ?1, 'Codex', 'shared-session')",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at, source_agent, source_session_id)
         VALUES ('source-b', 'B', ?1, ?1, 'Codex', 'shared-session')",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO disc_source_history
            (disc_id, source_agent, source_session_id, linked_at)
         VALUES ('source-a', 'Codex', 'shared-session', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO disc_source_history
            (disc_id, source_agent, source_session_id, linked_at)
         VALUES ('source-b', 'Codex', 'shared-session', '2026-01-02T00:00:00Z')",
        [],
    )
    .unwrap();

    migrations::run(&conn).unwrap();

    assert!(column_exists(
        &conn,
        "discussions",
        "source_binding_version"
    ));
    assert!(column_exists(
        &conn,
        "disc_source_history",
        "binding_version"
    ));
    let open_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM disc_source_history
              WHERE source_agent = 'Codex'
                AND source_session_id = 'shared-session'
                AND unlinked_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open_count, 1);
    let current_disc: String = conn
        .query_row(
            "SELECT disc_id FROM disc_source_history
              WHERE source_agent = 'Codex'
                AND source_session_id = 'shared-session'
                AND unlinked_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(current_disc, "source-b", "the newest open binding wins");
    let cleared: (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT source_agent, source_binding_version
               FROM discussions WHERE id = 'source-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cleared, (None, None));
    let current_version: i64 = conn
        .query_row(
            "SELECT source_binding_version FROM discussions WHERE id = 'source-b'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(current_version, 1);
}

#[test]
fn migration_093_adds_an_idempotent_discussion_import_ledger() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "092_disc_source_binding_contract").unwrap();
    assert!(!table_exists(&conn, "discussion_imports"));
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at)
         VALUES ('import-target', 'Imported', ?1, ?1)",
        params![now],
    )
    .unwrap();

    migrations::run(&conn).unwrap();
    assert!(table_exists(&conn, "discussion_imports"));
    conn.execute(
        "INSERT INTO discussion_imports
         (source_discussion_id, content_sha256, imported_discussion_id, imported_at)
         VALUES ('source-disc', 'abc123', 'import-target', ?1)",
        params![now],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO discussion_imports
         (source_discussion_id, content_sha256, imported_discussion_id, imported_at)
         VALUES ('source-disc', 'different', 'import-target', ?1)",
        params![now],
    );
    assert!(duplicate.is_err(), "a source discussion imports only once");

    conn.execute("DELETE FROM discussions WHERE id = 'import-target'", [])
        .unwrap();
    let ledger_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM discussion_imports", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        ledger_rows, 0,
        "deleting the imported copy clears its ledger"
    );
}

#[test]
fn migration_094_adds_secret_free_plugin_bundle_audit_and_import_ledger() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "093_discussion_imports").unwrap();
    assert!(!table_exists(&conn, "plugin_bundle_events"));
    assert!(!table_exists(&conn, "plugin_bundle_imports"));

    migrations::run(&conn).unwrap();
    assert!(table_exists(&conn, "plugin_bundle_events"));
    assert!(table_exists(&conn, "plugin_bundle_imports"));
    conn.execute(
        "INSERT INTO plugin_bundle_events
         (id, action, bundle_id, config_ids_json, includes_values, success, detail_json)
         VALUES ('event-1', 'export', 'bundle-1', '[\"config-1\"]', 1, 1, '{\"count\":1}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO plugin_bundle_imports
         (source_bundle_id, content_sha256, report_json)
         VALUES ('bundle-1', 'sha256', '{\"imported\":1}')",
        [],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO plugin_bundle_imports
         (source_bundle_id, content_sha256, report_json)
         VALUES ('bundle-1', 'different', '{}')",
        [],
    );
    assert!(
        duplicate.is_err(),
        "one import ledger row per source bundle"
    );

    let mut statement = conn
        .prepare("PRAGMA table_info(plugin_bundle_events)")
        .unwrap();
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for forbidden in ["passphrase", "secret", "env", "ciphertext", "payload"] {
        assert!(
            !columns.iter().any(|column| column.contains(forbidden)),
            "audit schema must not contain `{forbidden}`"
        );
    }
}

/// KT-74 — a discussion imported before 096 must still read as a portable
/// bundle afterwards, with no author rather than an invented one.
#[test]
fn migration_096_backfills_import_provenance_without_inventing_an_author() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "095_message_replies").unwrap();
    assert!(!column_exists(
        &conn,
        "discussion_imports",
        "provenance_kind"
    ));

    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at)
         VALUES ('legacy-target', 'Imported before 096', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_imports
         (source_discussion_id, content_sha256, imported_discussion_id, imported_at)
         VALUES ('legacy-source', 'sha-legacy', 'legacy-target', ?1)",
        params![now],
    )
    .unwrap();

    migrations::run(&conn).unwrap();
    for column in [
        "provenance_kind",
        "imported_by_pseudo",
        "imported_by_avatar_email",
    ] {
        assert!(column_exists(&conn, "discussion_imports", column));
    }

    let (kind, pseudo, avatar): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT provenance_kind, imported_by_pseudo, imported_by_avatar_email
             FROM discussion_imports WHERE source_discussion_id = 'legacy-source'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "portable_bundle");
    assert_eq!(pseudo, None, "an unknown exporter stays unknown");
    assert_eq!(avatar, None);

    // The reserved route is a distinct value, not a reinterpretation of the
    // rows above.
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at)
         VALUES ('transcript-target', 'Future', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_imports
         (source_discussion_id, content_sha256, imported_discussion_id, imported_at,
          provenance_kind, imported_by_pseudo, imported_by_avatar_email)
         VALUES ('transcript-source', 'sha-t', 'transcript-target', ?1,
                 'agent_transcript', 'Romu', 'romu@example.test')",
        params![now],
    )
    .unwrap();
    let kinds: Vec<String> = conn
        .prepare("SELECT provenance_kind FROM discussion_imports ORDER BY source_discussion_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(kinds, vec!["portable_bundle", "agent_transcript"]);
}

/// KT-37 — the provenance columns MUST live in their own migration (089): 088
/// was already applied on dev DBs, and the runner never re-runs a recorded
/// migration. 089 ALTERs them in so a 088-only DB and a fresh 086→…→089 upgrade
/// converge on one schema.
#[test]
fn migration_089_adds_model_provenance_columns_to_an_existing_088_db() {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run_through(&conn, "088_proposal_decision_idempotency").unwrap();
    assert!(
        !column_exists(&conn, "discussions", "partial_response_agent_type"),
        "088 alone must NOT carry the checkpoint provenance columns"
    );
    assert!(!column_exists(
        &conn,
        "discussions",
        "partial_response_model"
    ));
    assert!(
        !column_exists(&conn, "discussion_sessions", "model"),
        "088 alone must NOT carry the declared-model column"
    );

    // Apply the remaining migration(s) — 089 ALTERs the columns in.
    migrations::run(&conn).unwrap();
    assert!(
        column_exists(&conn, "discussions", "partial_response_agent_type"),
        "089 adds the checkpoint agent column to a 088-only DB"
    );
    assert!(column_exists(
        &conn,
        "discussions",
        "partial_response_model"
    ));
    assert!(
        column_exists(&conn, "discussion_sessions", "model"),
        "089 adds the declared-model column to discussion_sessions"
    );
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

fn column_exists(conn: &Connection, table: &str, col: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|c| c.ok())
        .collect();
    cols.iter().any(|c| c == col)
}

fn table_counts(conn: &Connection) -> Vec<(&'static str, i64)> {
    [
        "messages",
        "discussions",
        "discussion_sessions",
        "agent_dispatch_jobs",
    ]
    .iter()
    .map(|t| {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
            .unwrap();
        (*t, n)
    })
    .collect()
}
