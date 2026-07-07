use chrono::Utc;
use rusqlite::Connection;
use crate::db::migrations;
use crate::models::*;

/// Create an in-memory database with all migrations applied
fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&conn).unwrap();
    conn
}

fn sample_project(id: &str, name: &str) -> Project {
    let now = Utc::now();
    Project {
        id: id.into(),
        name: name.into(),
        path: format!("/tmp/{}", name),
        repo_url: Some("https://github.com/test/repo".into()),
        token_override: None,
        ai_config: AiConfigStatus { detected: false, configs: vec![] },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
            tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
            linked_repos: vec![],
        created_at: now,
        updated_at: now,
    }
}

fn sample_discussion(id: &str, project_id: Option<&str>) -> Discussion {
    let now = Utc::now();
    Discussion {
        id: id.into(),
        project_id: project_id.map(|s| s.into()),
        title: "Test Discussion".into(),
        agent: AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![AgentType::ClaudeCode],
        messages: vec![],
        message_count: 0, non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    }
}

fn sample_message(id: &str, role: MessageRole) -> DiscussionMessage {
    DiscussionMessage {
        model: None,
        lint_report: None,
        id: id.into(),
        role,
        content: format!("Message {}", id),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
        source_msg_id: None, duration_ms: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Migrations
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn migrations_idempotent() {
    let conn = test_db();
    // Running migrations again should not fail
    migrations::run(&conn).unwrap();
    migrations::run(&conn).unwrap();
}

// P0-7 of the QA roadmap — strengthen `migrations_idempotent`.
//
// The bare "run twice without panic" check from above doesn't actually
// PROVE idempotence : a migration could silently re-CREATE a table
// (no error if `IF NOT EXISTS` is used, but inserting duplicate seed
// rows would corrupt state) or add a second index on the same column.
// Pin the stronger invariant : the final schema (tables + indices +
// triggers + views) is BYTE-IDENTICAL between run 1 and run 2.
//
// This catches:
//   - a `CREATE INDEX` without `IF NOT EXISTS` that silently fails on
//     the 2nd run (would be caught by the bare test today — sqlite
//     raises) — but ALSO :
//   - a `CREATE TABLE ... IF NOT EXISTS` where the 2nd-run schema
//     diverges (e.g. column order change between branches),
//   - a migration that adds the same row twice via INSERT OR IGNORE
//     vs INSERT (count mismatch on 2nd run).
//
// Failure mode the bare test misses : deploy → migrations re-run on
// restart → silently doubled-rows or schema drift → user data corrupted
// invisibly until a downstream query breaks.
#[test]
fn migrations_idempotent_schema_stable_across_two_runs() {
    // First run.
    let conn = test_db();
    let schema_run_1 = capture_schema(&conn);
    let row_counts_run_1 = capture_seed_row_counts(&conn);

    // Re-apply migrations on the SAME connection.
    migrations::run(&conn).expect("re-run of migrations must succeed");
    let schema_run_2 = capture_schema(&conn);
    let row_counts_run_2 = capture_seed_row_counts(&conn);

    // Schema is the authoritative invariant — `sqlite_master` lists every
    // table, index, trigger, view + its CREATE statement. A diff here =
    // a non-idempotent migration.
    assert_eq!(
        schema_run_1, schema_run_2,
        "Schema diverged after the 2nd migrations::run.\nRun 1:\n{}\n---\nRun 2:\n{}",
        schema_run_1, schema_run_2,
    );

    // Seed rows (e.g. builtin agents, default skills) must not duplicate.
    assert_eq!(
        row_counts_run_1, row_counts_run_2,
        "Seed row count diverged after 2nd run — a migration is INSERT-ing without OR IGNORE",
    );
}

/// Dump `sqlite_master` to a comparable string. Sorting + filtering out
/// the auto-generated `sqlite_autoindex_*` rows (they're a consequence of
/// `UNIQUE` / `PRIMARY KEY` declarations, not authoritative on their own).
fn capture_schema(conn: &rusqlite::Connection) -> String {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_autoindex_%' \
             ORDER BY type, name",
        )
        .unwrap();
    let rows: Vec<String> = stmt
        .query_map([], |r| {
            let ty: String = r.get(0)?;
            let name: String = r.get(1)?;
            // `sql` is NULL for some auto-objects ; coalesce to empty.
            let sql: Option<String> = r.get(2)?;
            Ok(format!("[{}] {}: {}", ty, name, sql.unwrap_or_default()))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    rows.join("\n")
}

/// Capture row counts on a curated set of tables that migrations are
/// known to seed (or could plausibly seed). A divergence here = a
/// migration inserts seed rows without `INSERT OR IGNORE` and silently
/// doubles them on re-run.
fn capture_seed_row_counts(conn: &rusqlite::Connection) -> Vec<(String, i64)> {
    let candidate_tables = [
        "projects", "discussions", "messages", "mcp_servers", "mcp_configs",
        "workflows", "workflow_runs", "quick_prompts", "skills", "profiles",
        "directives", "contacts", "discussion_sessions",
    ];
    candidate_tables
        .iter()
        .filter_map(|t| {
            let n: rusqlite::Result<i64> = conn.query_row(
                &format!("SELECT COUNT(*) FROM {}", t),
                [],
                |r| r.get(0),
            );
            n.ok().map(|c| (t.to_string(), c))
        })
        .collect()
}

#[test]
fn migrations_create_all_tables() {
    let conn = test_db();
    let tables: Vec<String> = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB '_*' ORDER BY name"
    ).unwrap()
    .query_map([], |r| r.get(0)).unwrap()
    .filter_map(|r| r.ok()).collect();

    assert!(tables.contains(&"projects".into()), "Missing 'projects' table");
    assert!(tables.contains(&"discussions".into()), "Missing 'discussions' table");
    assert!(tables.contains(&"messages".into()), "Missing 'messages' table");
    assert!(tables.contains(&"mcp_servers".into()), "Missing 'mcp_servers' table");
    assert!(tables.contains(&"mcp_configs".into()), "Missing 'mcp_configs' table");
    assert!(tables.contains(&"workflows".into()), "Missing 'workflows' table");
    assert!(tables.contains(&"workflow_runs".into()), "Missing 'workflow_runs' table");
}

// ═══════════════════════════════════════════════════════════════════════════
// Projects CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_insert_and_list() {
    let conn = test_db();
    let p = sample_project("p1", "MyProject");
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "MyProject");
    assert_eq!(projects[0].path, "/tmp/MyProject");
}

#[test]
fn projects_get_by_id() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "B")).unwrap();

    let found = crate::db::projects::get_project(&conn, "p2").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "B");

    let missing = crate::db::projects::get_project(&conn, "p999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn projects_delete() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let deleted = crate::db::projects::delete_project(&conn, "p1").unwrap();
    assert!(deleted);

    let deleted_again = crate::db::projects::delete_project(&conn, "p1").unwrap();
    assert!(!deleted_again);

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert!(projects.is_empty());
}

#[test]
fn projects_update_ai_config() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let new_config = AiConfigStatus {
        detected: true,
        configs: vec![AiConfigType::ClaudeMd, AiConfigType::AiDir],
    };
    crate::db::projects::update_project_ai_config(&conn, "p1", &new_config).unwrap();

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.ai_config.detected);
    assert_eq!(p.ai_config.configs.len(), 2);
}

#[test]
fn projects_list_ordered_by_name() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Zebra")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "Alpha")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p3", "Middle")).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    assert_eq!(projects[0].name, "Alpha");
    assert_eq!(projects[1].name, "Middle");
    assert_eq!(projects[2].name, "Zebra");
}

// ═══════════════════════════════════════════════════════════════════════════
// Projects — default skills / profile / cascade delete
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_update_default_skills() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    let skills = vec!["skill-rust-expert".to_string(), "skill-testing".to_string()];
    let updated = crate::db::projects::update_project_default_skills(&conn, "p1", &skills).unwrap();
    assert!(updated, "update should affect one row");

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(p.default_skill_ids, skills, "Default skills must persist after update");
}

#[test]
fn projects_update_default_skills_to_empty() {
    let conn = test_db();
    let mut proj = sample_project("p1", "A");
    proj.default_skill_ids = vec!["old-skill".into()];
    crate::db::projects::insert_project(&conn, &proj).unwrap();

    crate::db::projects::update_project_default_skills(&conn, "p1", &[]).unwrap();

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.default_skill_ids.is_empty(), "Skills should be clearable to empty");
}

#[test]
fn projects_update_default_profile() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Set a profile
    let updated = crate::db::projects::update_project_default_profile(&conn, "p1", Some("profile-senior")).unwrap();
    assert!(updated);

    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(p.default_profile_id.as_deref(), Some("profile-senior"));

    // Clear the profile
    crate::db::projects::update_project_default_profile(&conn, "p1", None).unwrap();
    let p = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert!(p.default_profile_id.is_none(), "Profile should be clearable to None");
}

#[test]
fn projects_delete_cascade_discussions() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Insert discussions linked to project
    let d1 = sample_discussion("d1", Some("p1"));
    let d2 = sample_discussion("d2", Some("p1"));
    let d3 = sample_discussion("d3", None); // unlinked
    crate::db::discussions::insert_discussion(&conn, &d1).unwrap();
    crate::db::discussions::insert_discussion(&conn, &d2).unwrap();
    crate::db::discussions::insert_discussion(&conn, &d3).unwrap();

    // Cascade delete project discussions
    crate::db::projects::delete_project_discussions(&conn, "p1").unwrap();

    let all = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(all.len(), 1, "Only unlinked discussion should remain");
    assert_eq!(all[0].id, "d3");
}

#[test]
fn disc_no_agent_flag_round_trips() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    assert!(!crate::db::discussions::disc_is_no_agent(&conn, "d1").unwrap(), "default agent-capable");
    assert!(crate::db::discussions::set_disc_no_agent(&conn, "d1", true).unwrap());
    assert!(crate::db::discussions::disc_is_no_agent(&conn, "d1").unwrap(), "flag set");
    crate::db::discussions::set_disc_no_agent(&conn, "d1", false).unwrap();
    assert!(!crate::db::discussions::disc_is_no_agent(&conn, "d1").unwrap(), "flag cleared");
}

#[test]
fn discussion_model_override_round_trips() {
    // 070/2c — the explicit per-discussion model column persists through
    // insert → get and via the list mapping. Default (None) stays None.
    let conn = test_db();
    let mut d = sample_discussion("d-model", None);
    d.model = Some("qwen3:8b".into());
    crate::db::discussions::insert_discussion(&conn, &d).unwrap();
    let got = crate::db::discussions::get_discussion(&conn, "d-model").unwrap().unwrap();
    assert_eq!(got.model, Some("qwen3:8b".into()));

    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d-none", None)).unwrap();
    let none = crate::db::discussions::get_discussion(&conn, "d-none").unwrap().unwrap();
    assert_eq!(none.model, None, "no override → None, resolve from tier");
}

#[test]
fn ensure_mirror_by_shared_id_is_idempotent_and_formats_title() {
    let conn = test_db();
    // First call creates the mirror and returns its local id.
    let id1 = crate::db::discussions::ensure_mirror_by_shared_id(
        &conn, "shared-abc", "Topic", "PeerAlpha",
    )
    .unwrap();
    // Second call with the same shared_id returns the SAME local disc — no dup
    // (this is what makes the HTTP-create + late-WS-invite paths converge).
    let id2 = crate::db::discussions::ensure_mirror_by_shared_id(
        &conn, "shared-abc", "Topic", "PeerAlpha",
    )
    .unwrap();
    assert_eq!(id1, id2, "idempotent on shared_id");

    let all = crate::db::discussions::list_discussions(&conn).unwrap();
    let mirrors = all
        .iter()
        .filter(|d| d.shared_id.as_deref() == Some("shared-abc"))
        .count();
    assert_eq!(mirrors, 1, "exactly one mirror disc for the shared_id");

    let disc = crate::db::discussions::get_discussion(&conn, &id1)
        .unwrap()
        .unwrap();
    assert_eq!(
        disc.title, "Topic (shared by PeerAlpha)",
        "title must match the WS-invite creation format so both paths converge"
    );
    assert_eq!(disc.shared_id.as_deref(), Some("shared-abc"));
}

#[test]
fn federated_context_file_insert_exists_and_get() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    assert!(!crate::db::discussions::context_file_exists(&conn, "f1").unwrap());

    // F8: insert a file received from a peer, pinned to a message.
    crate::db::discussions::insert_federated_context_file(
        &conn, "f1", "d1", "m1", "doc.pdf", "application/pdf", 1234, "/tmp/x.pdf",
    )
    .unwrap();
    assert!(crate::db::discussions::context_file_exists(&conn, "f1").unwrap());

    let cf = crate::db::discussions::get_context_file(&conn, "f1").unwrap().unwrap();
    assert_eq!(cf.filename, "doc.pdf");
    assert_eq!(cf.mime_type, "application/pdf");
    assert_eq!(cf.original_size, 1234);
    assert_eq!(cf.message_id.as_deref(), Some("m1"), "pinned to the right message");
    assert_eq!(cf.disk_path.as_deref(), Some("/tmp/x.pdf"));
    assert!(crate::db::discussions::get_context_file(&conn, "missing").unwrap().is_none());
}

#[test]
fn list_shared_sync_points_lists_only_shared_discs() {
    let conn = test_db();
    let mut shared = sample_discussion("s1", None);
    shared.shared_id = Some("shared-1".into());
    crate::db::discussions::insert_discussion(&conn, &shared).unwrap();
    // A purely local disc must NOT appear in the sync points.
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("l1", None)).unwrap();

    let pts = crate::db::discussions::list_shared_sync_points(&conn).unwrap();
    assert_eq!(pts.len(), 1, "only the shared disc is a sync point");
    assert_eq!(pts[0].0, "shared-1");
    assert_eq!(pts[0].1, 0, "a disc with no messages reports since = 0");
}

#[test]
fn shared_id_unique_index_rejects_duplicate_and_ensure_mirror_absorbs_it() {
    let conn = test_db();
    let mut d1 = sample_discussion("d1", None);
    d1.shared_id = Some("dup-shared".into());
    let mut d2 = sample_discussion("d2", None);
    d2.shared_id = Some("dup-shared".into());
    crate::db::discussions::insert_discussion(&conn, &d1).unwrap();
    // Migration 068's UNIQUE partial index forbids a second disc on the same
    // shared_id — the invariant find_discussion_by_shared_id relies on.
    let second = crate::db::discussions::insert_discussion(&conn, &d2);
    assert!(
        second.is_err(),
        "two discs with the same shared_id must violate the UNIQUE index"
    );
    // ensure_mirror_by_shared_id never trips it: it returns the existing disc.
    let id = crate::db::discussions::ensure_mirror_by_shared_id(&conn, "dup-shared", "T", "Peer")
        .unwrap();
    assert_eq!(id, "d1", "ensure_mirror returns the existing local disc, no duplicate");
}

#[test]
fn projects_update_nonexistent_returns_false() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_default_skills(&conn, "nonexistent", &["s1".into()]).unwrap();
    assert!(!updated, "Updating nonexistent project should return false");
}

#[test]
fn projects_briefing_notes_set_and_get() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Initially None
    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert!(notes.is_none(), "Briefing notes should be None initially");

    // Set notes
    let updated = crate::db::projects::update_project_briefing_notes(
        &conn, "p1", Some("This is a React app with a REST API backend")
    ).unwrap();
    assert!(updated);

    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert_eq!(notes.as_deref(), Some("This is a React app with a REST API backend"));

    // Verify it's also returned in get_project
    let project = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(project.briefing_notes.as_deref(), Some("This is a React app with a REST API backend"));
}

#[test]
fn projects_briefing_notes_clear() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "A")).unwrap();

    // Set then clear
    crate::db::projects::update_project_briefing_notes(&conn, "p1", Some("Some notes")).unwrap();
    crate::db::projects::update_project_briefing_notes(&conn, "p1", None).unwrap();

    let notes = crate::db::projects::get_project_briefing_notes(&conn, "p1").unwrap();
    assert!(notes.is_none(), "Briefing notes should be clearable to None");
}

#[test]
fn projects_briefing_notes_nonexistent_project() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_briefing_notes(&conn, "nonexistent", Some("notes")).unwrap();
    assert!(!updated, "Updating briefing notes for nonexistent project should return false");
}

#[test]
fn projects_briefing_notes_persisted_in_insert() {
    let conn = test_db();
    let mut p = sample_project("p1", "WithNotes");
    p.briefing_notes = Some("Pre-filled briefing".into());
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let loaded = crate::db::projects::get_project(&conn, "p1").unwrap().unwrap();
    assert_eq!(loaded.briefing_notes.as_deref(), Some("Pre-filled briefing"));
}

#[test]
fn projects_briefing_notes_in_list() {
    let conn = test_db();
    let mut p = sample_project("p1", "A");
    p.briefing_notes = Some("Project A notes".into());
    crate::db::projects::insert_project(&conn, &p).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "B")).unwrap();

    let projects = crate::db::projects::list_projects(&conn).unwrap();
    let a = projects.iter().find(|p| p.id == "p1").unwrap();
    let b = projects.iter().find(|p| p.id == "p2").unwrap();
    assert_eq!(a.briefing_notes.as_deref(), Some("Project A notes"));
    assert!(b.briefing_notes.is_none());
}

// ═══════════════════════════════════════════════════════════════════════════
// Discussions CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn discussions_insert_and_list() {
    let conn = test_db();
    let disc = sample_discussion("d1", None);
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0].title, "Test Discussion");
}

#[test]
fn discussions_get_by_id() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let found = crate::db::discussions::get_discussion(&conn, "d1").unwrap();
    assert!(found.is_some());

    let missing = crate::db::discussions::get_discussion(&conn, "d999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn discussions_delete() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    let deleted = crate::db::discussions::delete_discussion(&conn, "d1").unwrap();
    assert!(deleted);

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert!(discussions.is_empty());
}

#[test]
fn discussions_with_project() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Proj")).unwrap();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", Some("p1"))).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, "d1").unwrap().unwrap();
    assert_eq!(disc.project_id, Some("p1".into()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Messages CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn messages_insert_and_list() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let msg = sample_message("m1", MessageRole::User);
    crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "Message m1");
    assert!(matches!(messages[0].role, MessageRole::User));
}

#[test]
fn messages_sort_order() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    for i in 1..=5 {
        let msg = sample_message(&format!("m{}", i), MessageRole::User);
        crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();
    }

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 5);
    // Check ordering is preserved
    for (i, msg) in messages.iter().enumerate() {
        assert_eq!(msg.content, format!("Message m{}", i + 1));
    }
}

#[test]
fn messages_with_tokens_and_auth_mode() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    let mut msg = sample_message("m1", MessageRole::Agent);
    msg.tokens_used = 1500;
    msg.auth_mode = Some("override".into());
    msg.agent_type = Some(AgentType::ClaudeCode);
    crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].tokens_used, 1500);
    assert_eq!(messages[0].auth_mode, Some("override".into()));
}

#[test]
fn messages_update_tokens() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::Agent)).unwrap();

    crate::db::discussions::update_message_tokens(&conn, "m1", 999, Some("local")).unwrap();

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].tokens_used, 999);
    assert_eq!(messages[0].auth_mode, Some("local".into()));
}

#[test]
fn messages_delete_last_agent() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();

    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m3", MessageRole::System)).unwrap();

    let deleted = crate::db::discussions::delete_last_agent_messages(&conn, "d1").unwrap();
    assert_eq!(deleted, 2); // Agent + System messages after last User

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages.len(), 1);
    assert!(matches!(messages[0].role, MessageRole::User));
}

#[test]
fn messages_edit_last_user() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();

    let edited = crate::db::discussions::edit_last_user_message(&conn, "d1", "updated content").unwrap();
    assert!(edited);

    let messages = crate::db::discussions::list_messages(&conn, "d1").unwrap();
    assert_eq!(messages[0].content, "updated content");
}

#[test]
fn discussions_loaded_with_messages() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, "d1").unwrap().unwrap();
    assert_eq!(disc.messages.len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP CRUD
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_server_upsert_and_list() {
    let conn = test_db();
    let server = McpServer {
        id: "test-server".into(),
        name: "Test".into(),
        description: "A test server".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let servers = crate::db::mcps::list_servers(&conn).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "Test");

    // Upsert again (update)
    let updated = McpServer { name: "Updated Test".into(), ..server };
    crate::db::mcps::upsert_server(&conn, &updated).unwrap();

    let servers = crate::db::mcps::list_servers(&conn).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "Updated Test");
}

#[test]
fn sync_registry_refreshes_api_spec_on_existing_rows_only() {
    // Regression for the GitHub case: a user configures a plugin BEFORE
    // the registry gains `api_spec` for it. The DB row sticks with the
    // old shape (api_spec: None) and the workflow wizard's plugin picker
    // silently filters it out. The startup sync must re-mirror the
    // registry's current api_spec onto every existing row, without
    // creating rows for plugins the user never configured.
    use crate::models::{ApiAuthKind, ApiEndpoint, ApiSpec, McpDefinition, McpServer, McpSource, McpTransport};
    let conn = test_db();

    // Pre-existing row, no api_spec — mirrors the "configured before
    // registry enrichment" state.
    let stale = McpServer {
        id: "mcp-foo".into(),
        name: "Foo".into(),
        description: "old".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "foo".into()] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &stale).unwrap();

    // Registry gained api_spec for this plugin + lists a brand-new
    // plugin the user has never configured.
    let registry = vec![
        McpDefinition {
            id: "mcp-foo".into(),
            name: "Foo".into(),
            description: "fresh".into(),
            transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "foo".into()] },
            env_keys: vec!["FOO_TOKEN".into()],
            tags: vec![],
            token_url: None,
            token_help: None,
            publisher: "Test".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: Some(ApiSpec {
                base_url: "https://api.foo".into(),
                auth: ApiAuthKind::Bearer { env_key: "FOO_TOKEN".into() },
                docs_url: None,
                config_keys: vec![],
                endpoints: vec![ApiEndpoint { path: "/me".into(), method: "GET".into(), description: "x".into() }],
            }),
        },
        // User never created a config for this one — sync must NOT
        // insert it (the user picks plugins explicitly in Settings).
        McpDefinition {
            id: "mcp-never-added".into(),
            name: "Never Added".into(),
            description: "x".into(),
            transport: McpTransport::Stdio { command: "x".into(), args: vec![] },
            env_keys: vec![],
            tags: vec![],
            token_url: None,
            token_help: None,
            publisher: "Test".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: Some(ApiSpec {
                base_url: "https://api.never".into(),
                auth: ApiAuthKind::None,
                docs_url: None,
                config_keys: vec![],
                endpoints: vec![],
            }),
        },
    ];

    let updated = crate::db::mcps::sync_registry_servers_to_db(&conn, &registry).unwrap();
    assert_eq!(updated, 1, "only the existing row gets refreshed");

    let after = crate::db::mcps::list_servers(&conn).unwrap();
    assert_eq!(after.len(), 1, "sync must NOT create rows for unconfigured plugins");
    let foo = &after[0];
    assert!(foo.api_spec.is_some(), "stale row gets the new api_spec");
    assert_eq!(foo.api_spec.as_ref().unwrap().base_url, "https://api.foo");
    assert_eq!(foo.description, "fresh", "description also re-mirrored");
}

#[test]
fn mcp_config_insert_with_projects() {
    let conn = test_db();
    // Create server and projects first
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Sse { url: "http://localhost".into() },
        source: McpSource::Manual,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Proj1")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "My Config".into(),
        env_keys: vec!["KEY1".into()], env_encrypted: "enc".into(),
        args_override: None, is_global: false, include_general: true, config_hash: "hash1".into(),
        project_ids: vec!["p1".into()], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    let configs = crate::db::mcps::list_configs(&conn).unwrap();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].project_ids, vec!["p1".to_string()]);
}

#[test]
fn mcp_configs_for_project_includes_global() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();

    // Global config
    let global = McpConfig {
        id: "cfg-global".into(), server_id: "srv1".into(), label: "Global".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: true, include_general: true, config_hash: "h1".into(),
        project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &global).unwrap();

    // Project-specific config
    let specific = McpConfig {
        id: "cfg-proj".into(), server_id: "srv1".into(), label: "Proj".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true, config_hash: "h2".into(),
        project_ids: vec!["p1".into()], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &specific).unwrap();

    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert_eq!(for_p1.len(), 2); // Global + specific
}

#[test]
fn mcp_encrypt_decrypt_env() {
    let secret = crate::core::crypto::generate_secret();
    let mut env = std::collections::HashMap::new();
    env.insert("KEY1".into(), "value1".into());
    env.insert("KEY2".into(), "value2".into());

    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
    assert!(!encrypted.is_empty());

    let decrypted = crate::db::mcps::decrypt_env(&encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("KEY1").unwrap(), "value1");
    assert_eq!(decrypted.get("KEY2").unwrap(), "value2");
}

#[test]
fn mcp_encrypt_empty_env() {
    let secret = crate::core::crypto::generate_secret();
    let env = std::collections::HashMap::new();
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
    assert!(encrypted.is_empty());

    let decrypted = crate::db::mcps::decrypt_env(&encrypted, &secret).unwrap();
    assert!(decrypted.is_empty());
}

#[test]
fn mcp_config_hash_deterministic() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
        api_spec: None,
    };
    let mut env = std::collections::HashMap::new();
    env.insert("K".into(), "V".into());

    let h1 = crate::db::mcps::compute_config_hash(&server, &env, None);
    let h2 = crate::db::mcps::compute_config_hash(&server, &env, None);
    assert_eq!(h1, h2);
}

#[test]
fn mcp_config_hash_differs_on_env() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    let mut env1 = std::collections::HashMap::new();
    env1.insert("K".into(), "V1".into());
    let mut env2 = std::collections::HashMap::new();
    env2.insert("K".into(), "V2".into());

    let h1 = crate::db::mcps::compute_config_hash(&server, &env1, None);
    let h2 = crate::db::mcps::compute_config_hash(&server, &env2, None);
    assert_ne!(h1, h2);
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP Config Update & Sync
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_config_update_env_persists() {
    let conn = test_db();
    let secret = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv1".into(), name: "GitHub".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let mut env = std::collections::HashMap::new();
    env.insert("TOKEN".into(), "old-value".into());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "My GitHub".into(),
        env_keys: vec!["TOKEN".into()], env_encrypted: encrypted,
        args_override: None, is_global: false, include_general: true,
        config_hash: "h1".into(), project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Update env with new value
    let mut new_env = std::collections::HashMap::new();
    new_env.insert("TOKEN".into(), "new-secret-value".into());
    let new_encrypted = crate::db::mcps::encrypt_env(&new_env, &secret).unwrap();
    let new_keys = vec!["TOKEN".to_string()];

    let updated = crate::db::mcps::update_config(
        &conn, "cfg1", None, Some(&new_encrypted), Some(&new_keys),
        None, None, None, None, None,
    ).unwrap();
    assert!(updated, "update_config should return true");

    // Verify the stored encrypted value decrypts to the new value
    let loaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    let decrypted = crate::db::mcps::decrypt_env(&loaded.env_encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("TOKEN").unwrap(), "new-secret-value",
        "Updated env value must persist after update_config");
}

#[test]
fn mcp_config_update_nonexistent_returns_false() {
    let conn = test_db();
    let result = crate::db::mcps::update_config(
        &conn, "nonexistent", Some("label"), None, None, None, None, None, None, None,
    ).unwrap();
    assert!(!result, "Updating a nonexistent config should return false");
}

#[test]
fn mcp_decrypt_wrong_secret_fails() {
    let secret1 = crate::core::crypto::generate_secret();
    let secret2 = crate::core::crypto::generate_secret();

    let mut env = std::collections::HashMap::new();
    env.insert("KEY".into(), "secret-value".into());

    let encrypted = crate::db::mcps::encrypt_env(&env, &secret1).unwrap();
    let result = crate::db::mcps::decrypt_env(&encrypted, &secret2);
    assert!(result.is_err(), "Decrypting with wrong secret must fail");
}

#[test]
fn mcp_config_global_visible_to_all_projects() {
    let conn = test_db();
    let secret = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv1".into(), name: "Sentry".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "ProjectA")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "ProjectB")).unwrap();

    // Create a global config with encrypted env
    let mut env = std::collections::HashMap::new();
    env.insert("SENTRY_TOKEN".into(), "tok-123".into());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();

    let config = McpConfig {
        id: "cfg-global".into(), server_id: "srv1".into(), label: "Sentry Global".into(),
        env_keys: vec!["SENTRY_TOKEN".into()], env_encrypted: encrypted.clone(),
        args_override: None, is_global: true, include_general: true,
        config_hash: "h".into(), project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Both projects should see it
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert_eq!(for_p1.len(), 1, "P1 should see global config");
    assert_eq!(for_p2.len(), 1, "P2 should see global config");

    // Now update the global config's env
    let mut new_env = std::collections::HashMap::new();
    new_env.insert("SENTRY_TOKEN".into(), "tok-456-updated".into());
    let new_encrypted = crate::db::mcps::encrypt_env(&new_env, &secret).unwrap();
    crate::db::mcps::update_config(
        &conn, "cfg-global", None, Some(&new_encrypted), None, None, None, None, None, None,
    ).unwrap();

    // Both projects should see the UPDATED value
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    let decrypted = crate::db::mcps::decrypt_env(&for_p1[0].env_encrypted, &secret).unwrap();
    assert_eq!(decrypted.get("SENTRY_TOKEN").unwrap(), "tok-456-updated",
        "Global config update must be visible to all projects immediately");
}

#[test]
fn mcp_set_config_projects_relinks() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "P2")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p3", "P3")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into(), "p2".into()], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Verify initial state
    let loaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    assert_eq!(loaded.project_ids.len(), 2);

    // Re-link to p2 and p3 (remove p1, add p3)
    crate::db::mcps::set_config_projects(&conn, "cfg1", &["p2".into(), "p3".into()]).unwrap();

    let reloaded = crate::db::mcps::get_config(&conn, "cfg1").unwrap().unwrap();
    assert!(!reloaded.project_ids.contains(&"p1".to_string()), "p1 should be unlinked");
    assert!(reloaded.project_ids.contains(&"p2".to_string()), "p2 should remain");
    assert!(reloaded.project_ids.contains(&"p3".to_string()), "p3 should be added");

    // p1 should no longer see this config
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert!(for_p1.is_empty(), "p1 should have no configs after re-link");
}

#[test]
fn mcp_delete_config_removes_project_links() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();

    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into()], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Delete the config
    let deleted = crate::db::mcps::delete_config(&conn, "cfg1").unwrap();
    assert!(deleted);

    // Project should have no configs
    let for_p1 = crate::db::mcps::configs_for_project(&conn, "p1").unwrap();
    assert!(for_p1.is_empty(), "Deleted config should not appear in project configs");
}

#[test]
fn mcp_config_update_global_flag_changes_visibility() {
    let conn = test_db();
    let server = McpServer {
        id: "srv1".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "test".into(), args: vec![] },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "P1")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "P2")).unwrap();

    // Create non-global config linked to p1 only
    let config = McpConfig {
        id: "cfg1".into(), server_id: "srv1".into(), label: "Test".into(),
        env_keys: vec![], env_encrypted: "".into(),
        args_override: None, is_global: false, include_general: true,
        config_hash: "h".into(), project_ids: vec!["p1".into()], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // p2 should NOT see it
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert!(for_p2.is_empty(), "Non-global config should not be visible to unlinked project");

    // Promote to global
    crate::db::mcps::update_config(
        &conn, "cfg1", None, None, None, None, Some(true), None, None, None,
    ).unwrap();

    // Now p2 should see it
    let for_p2 = crate::db::mcps::configs_for_project(&conn, "p2").unwrap();
    assert_eq!(for_p2.len(), 1, "Global config must be visible to all projects");
}

#[test]
fn mcp_config_hash_changes_on_env_update() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
        api_spec: None,
    };

    let mut env_old = std::collections::HashMap::new();
    env_old.insert("TOKEN".into(), "old-value".into());
    let mut env_new = std::collections::HashMap::new();
    env_new.insert("TOKEN".into(), "new-value".into());

    let hash_old = crate::db::mcps::compute_config_hash(&server, &env_old, None);
    let hash_new = crate::db::mcps::compute_config_hash(&server, &env_new, None);

    assert_ne!(hash_old, hash_new,
        "Config hash must change when env values change (dedup detection)");
}

#[test]
fn mcp_config_hash_changes_on_args_override() {
    let server = McpServer {
        id: "srv".into(), name: "S".into(), description: "".into(),
        transport: McpTransport::Stdio { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] },
        source: McpSource::Registry,
        api_spec: None,
    };
    let env = std::collections::HashMap::new();

    let hash_default = crate::db::mcps::compute_config_hash(&server, &env, None);
    let hash_override = crate::db::mcps::compute_config_hash(
        &server, &env, Some(&vec!["--custom-flag".into()]),
    );

    assert_ne!(hash_default, hash_override,
        "Config hash must differ when args_override is set");
}

// ═══════════════════════════════════════════════════════════════════════════
// Workflows CRUD
// ═══════════════════════════════════════════════════════════════════════════

fn sample_workflow(id: &str) -> Workflow {
    let now = Utc::now();
    Workflow {
        id: id.into(),
        name: "Test Workflow".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![WorkflowStep {
            step_type: StepType::default(),
            output_format: StepOutputFormat::default(),
            description: None,
            name: "step1".into(),
            agent: AgentType::ClaudeCode,
            prompt_template: "Do something".into(),
            mode: StepMode::Normal,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            on_timeout: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        guards: None,
        artifacts: ::std::collections::HashMap::new(),
        on_failure: vec![],
        exec_allowlist: vec![],
        variables: vec![],
        enabled: true,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn workflows_insert_and_list() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    let workflows = crate::db::workflows::list_workflows(&conn).unwrap();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].name, "Test Workflow");
    assert!(workflows[0].enabled);
    assert_eq!(workflows[0].steps.len(), 1);
}

#[test]
fn workflows_get_by_id() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    let found = crate::db::workflows::get_workflow(&conn, "w1").unwrap();
    assert!(found.is_some());

    let missing = crate::db::workflows::get_workflow(&conn, "w999").unwrap();
    assert!(missing.is_none());
}

#[test]
fn workflows_update() {
    let conn = test_db();
    let mut wf = sample_workflow("w1");
    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    wf.name = "Updated".into();
    wf.enabled = false;
    crate::db::workflows::update_workflow(&conn, &wf).unwrap();

    let updated = crate::db::workflows::get_workflow(&conn, "w1").unwrap().unwrap();
    assert_eq!(updated.name, "Updated");
    assert!(!updated.enabled);
}

#[test]
fn workflows_delete() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::delete_workflow(&conn, "w1").unwrap();

    let workflows = crate::db::workflows::list_workflows(&conn).unwrap();
    assert!(workflows.is_empty());
}

// ─── Workflow Runs ───────────────────────────────────────────────────────

fn sample_run(id: &str, workflow_id: &str) -> WorkflowRun {
    let now = Utc::now();
    WorkflowRun {
        id: id.into(),
        workflow_id: workflow_id.into(),
        status: RunStatus::Running,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: ::std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    }
}

#[test]
fn workflow_runs_insert_and_list() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();

    let runs = crate::db::workflows::list_runs(&conn, "w1").unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Running);
}

#[test]
fn purge_runs_older_than_deletes_old_terminal_but_preserves_parents_and_recent() {
    use chrono::Duration;
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let old = || Utc::now() - Duration::days(100);

    // A parent (old, terminal) referenced by a child → must be PRESERVED.
    let mut parent = sample_run("parent", "w1");
    parent.status = RunStatus::Success;
    parent.finished_at = Some(old());
    crate::db::workflows::insert_run(&conn, &parent).unwrap();

    let mut child = sample_run("child", "w1");
    child.status = RunStatus::Success;
    child.parent_run_id = Some("parent".into());
    child.finished_at = Some(old());
    crate::db::workflows::insert_run(&conn, &child).unwrap();

    // Old standalone terminal → DELETED.
    let mut old_standalone = sample_run("old-standalone", "w1");
    old_standalone.status = RunStatus::Failed;
    old_standalone.finished_at = Some(old());
    crate::db::workflows::insert_run(&conn, &old_standalone).unwrap();

    // Recent terminal → kept (within window).
    let mut recent = sample_run("recent", "w1");
    recent.status = RunStatus::Success;
    recent.finished_at = Some(Utc::now());
    crate::db::workflows::insert_run(&conn, &recent).unwrap();

    // Old but still Running (no finished_at) → never purged.
    let mut running = sample_run("running", "w1");
    running.status = RunStatus::Running;
    running.started_at = old();
    crate::db::workflows::insert_run(&conn, &running).unwrap();

    let n = crate::db::workflows::purge_runs_older_than(&conn, 90).unwrap();
    assert_eq!(n, 2, "old standalone terminal + the (unreferenced-after) child");

    let exists = |id: &str| crate::db::workflows::get_run(&conn, id).unwrap().is_some();
    assert!(exists("parent"), "parent referenced by a child is preserved");
    assert!(!exists("old-standalone"), "old standalone terminal purged");
    assert!(!exists("child"), "old terminal child purged");
    assert!(exists("recent"), "recent run kept");
    assert!(exists("running"), "non-terminal run never purged");
}

#[test]
fn reconcile_stale_runs_flips_only_old_running_pending_to_interrupted() {
    use chrono::Duration;
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    // Stale: Running, started 1h ago → should become Interrupted.
    let mut stale = sample_run("stale", "w1");
    stale.status = RunStatus::Running;
    stale.started_at = Utc::now() - Duration::hours(1);
    crate::db::workflows::insert_run(&conn, &stale).unwrap();

    // Stale pending too.
    let mut stale_pending = sample_run("stale-pending", "w1");
    stale_pending.status = RunStatus::Pending;
    stale_pending.started_at = Utc::now() - Duration::hours(1);
    crate::db::workflows::insert_run(&conn, &stale_pending).unwrap();

    // Fresh: Running, started now → must NOT be touched (< cutoff).
    let mut fresh = sample_run("fresh", "w1");
    fresh.status = RunStatus::Running;
    crate::db::workflows::insert_run(&conn, &fresh).unwrap();

    // Already terminal: Success, old → must NOT be touched.
    let mut done = sample_run("done", "w1");
    done.status = RunStatus::Success;
    done.started_at = Utc::now() - Duration::hours(2);
    crate::db::workflows::insert_run(&conn, &done).unwrap();

    let n = crate::db::workflows::reconcile_stale_runs(&conn, 30 * 60).unwrap();
    assert_eq!(n, 2, "the two stale in-flight runs are reconciled");

    let by_id = |id: &str| crate::db::workflows::get_run(&conn, id).unwrap().unwrap();
    assert_eq!(by_id("stale").status, RunStatus::Interrupted);
    assert!(by_id("stale").finished_at.is_some(), "Interrupted run gets a finished_at");
    assert_eq!(by_id("stale-pending").status, RunStatus::Interrupted);
    assert_eq!(by_id("fresh").status, RunStatus::Running, "recent run untouched");
    assert_eq!(by_id("done").status, RunStatus::Success, "terminal run untouched");

    // Cutoff 0 = the BOOT call (Copilot, PR #114): at boot there is no runner,
    // so even a JUST-started zombie must flip — no 30-min lie window.
    let n0 = crate::db::workflows::reconcile_stale_runs(&conn, 0).unwrap();
    assert_eq!(n0, 1, "the fresh zombie is reconciled at cutoff 0");
    assert_eq!(by_id("fresh").status, RunStatus::Interrupted);
}

#[test]
fn list_runs_enriches_subworkflow_parent_provenance() {
    let conn = test_db();
    // A parent workflow with a run, and a child workflow whose run points at it.
    let mut parent_wf = sample_workflow("parent-wf");
    parent_wf.name = "Cron Parent".into();
    crate::db::workflows::insert_workflow(&conn, &parent_wf).unwrap();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("child-wf")).unwrap();

    let parent_run = sample_run("parent-run-1", "parent-wf");
    crate::db::workflows::insert_run(&conn, &parent_run).unwrap();

    let mut child = sample_run("child-run-1", "child-wf");
    child.run_type = "subworkflow".into();
    child.parent_run_id = Some("parent-run-1".into());
    crate::db::workflows::insert_run(&conn, &child).unwrap();

    let runs = crate::db::workflows::list_runs(&conn, "child-wf").unwrap();
    assert_eq!(runs.len(), 1);
    let r = &runs[0];
    assert_eq!(r.parent_workflow_id.as_deref(), Some("parent-wf"), "parent workflow id resolved");
    assert_eq!(r.parent_workflow_name.as_deref(), Some("Cron Parent"), "parent workflow name resolved via JOIN");
    assert_eq!(
        r.parent_run_started_at.map(|d| d.timestamp()),
        Some(parent_run.started_at.timestamp()),
        "parent run tick time carried"
    );

    // get_run enriches too.
    let one = crate::db::workflows::get_run(&conn, "child-run-1").unwrap().unwrap();
    assert_eq!(one.parent_workflow_name.as_deref(), Some("Cron Parent"));
}

#[test]
fn list_runs_provenance_none_for_toplevel_run() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    // Top-level run: no parent_run_id → provenance stays None (the common case).
    crate::db::workflows::insert_run(&conn, &sample_run("top", "w1")).unwrap();

    let runs = crate::db::workflows::list_runs(&conn, "w1").unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].parent_workflow_name.is_none());
    assert!(runs[0].parent_workflow_id.is_none());
    assert!(runs[0].parent_run_started_at.is_none());
}

#[test]
fn list_runs_provenance_cleared_when_parent_deleted() {
    // FK `parent_run_id REFERENCES workflow_runs(id) ON DELETE SET NULL`:
    // deleting the parent run NULLs the child's link (no dangling pointer),
    // so provenance resolves to None rather than a stale name.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("parent-wf")).unwrap();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("child-wf")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("p1", "parent-wf")).unwrap();
    let mut child = sample_run("c1", "child-wf");
    child.run_type = "subworkflow".into();
    child.parent_run_id = Some("p1".into());
    crate::db::workflows::insert_run(&conn, &child).unwrap();

    crate::db::workflows::delete_run(&conn, "p1").unwrap();

    let runs = crate::db::workflows::list_runs(&conn, "child-wf").unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].parent_workflow_name.is_none(), "parent gone → no stale provenance");
}

#[test]
fn workflow_runs_update() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let mut run = sample_run("r1", "w1");
    crate::db::workflows::insert_run(&conn, &run).unwrap();

    run.status = RunStatus::Success;
    run.tokens_used = 500;
    run.finished_at = Some(Utc::now());
    run.step_results = vec![StepResult {
        step_name: "step1".into(),
        status: RunStatus::Success,
        output: "Done".into(),
        tokens_used: 500,
        duration_ms: 1234,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: None,
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
    }];
    crate::db::workflows::update_run(&conn, &run).unwrap();

    let updated = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(updated.status, RunStatus::Success);
    assert_eq!(updated.tokens_used, 500);
    assert!(updated.finished_at.is_some());
    assert_eq!(updated.step_results.len(), 1);
}

#[test]
fn workflow_runs_produced_branches_round_trip() {
    // Regression: 0.7.0 added the `produced_branches` column on
    // workflow_runs. Ensure it serialises on insert + parses back on
    // load — without this test, a JSON typo or column-index drift in
    // row_to_run would silently swallow the data.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let mut run = sample_run("r1", "w1");
    run.produced_branches = vec![
        crate::models::ProducedBranch {
            branch_name: "kronn/Autobot/abcdef12".into(),
            head_sha: "0123456789abcdef0123456789abcdef01234567".into(),
            ahead: 3,
            pushed_upstream: false,
        },
        crate::models::ProducedBranch {
            branch_name: "kronn/Autobot/abcdef12-fix".into(),
            head_sha: "fedcba9876543210fedcba9876543210fedcba98".into(),
            ahead: 1,
            pushed_upstream: true,
        },
    ];
    crate::db::workflows::insert_run(&conn, &run).unwrap();

    let loaded = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(loaded.produced_branches.len(), 2);
    assert_eq!(loaded.produced_branches[0].branch_name, "kronn/Autobot/abcdef12");
    assert_eq!(loaded.produced_branches[0].ahead, 3);
    assert!(!loaded.produced_branches[0].pushed_upstream);
    assert!(loaded.produced_branches[1].pushed_upstream);
}

// ─── claim_waiting_run (TOCTOU gate claim, audit P1) ────────────────────────
//
// The atomic claim is the guard against two concurrent gate decisions (a
// double-click, or a human racing the auto-approve timer) BOTH passing a
// read-then-check and spawning two `resume_run`s on the same run. The
// conditional `UPDATE ... WHERE status='WaitingApproval'` must let exactly one
// caller win. These tests pin that contract at the DB layer — the only place
// the atomicity actually lives.

/// Build a run already parked in `WaitingApproval` (a run sitting at a Gate).
fn waiting_run(id: &str, workflow_id: &str) -> WorkflowRun {
    let mut run = sample_run(id, workflow_id);
    run.status = RunStatus::WaitingApproval;
    run
}

#[test]
fn claim_waiting_run_first_caller_wins_second_loses() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &waiting_run("r1", "w1")).unwrap();

    // First claim (the human who clicked Approve first) flips it to Running.
    let won_first =
        crate::db::workflows::claim_waiting_run(&conn, "r1", &RunStatus::Running).unwrap();
    assert!(won_first, "first claimer must win");

    // Second claim (the racing auto-approve timer, or a double-click) sees the
    // run is no longer WaitingApproval and loses — NO second resume spawned.
    let won_second =
        crate::db::workflows::claim_waiting_run(&conn, "r1", &RunStatus::Cancelled).unwrap();
    assert!(!won_second, "second claimer must lose once the run left WaitingApproval");

    // The winner's status sticks; the loser's Cancelled never applied.
    let run = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running, "first claim's status must persist");
}

#[test]
fn claim_waiting_run_rejects_a_run_that_is_not_waiting() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    // sample_run defaults to Running — never sat at a Gate.
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();

    let claimed =
        crate::db::workflows::claim_waiting_run(&conn, "r1", &RunStatus::Cancelled).unwrap();
    assert!(!claimed, "a Running run is not claimable — only WaitingApproval is");

    let run = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running, "status must be untouched");
}

#[test]
fn claim_waiting_run_supports_the_reject_transition() {
    // The gate-reject path claims into Cancelled. Same atomic guard, different
    // target status — assert it transitions and is then no longer re-claimable.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &waiting_run("r1", "w1")).unwrap();

    let won = crate::db::workflows::claim_waiting_run(&conn, "r1", &RunStatus::Cancelled).unwrap();
    assert!(won);
    assert_eq!(
        crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap().status,
        RunStatus::Cancelled,
    );

    // Re-claiming a cancelled run loses.
    let again =
        crate::db::workflows::claim_waiting_run(&conn, "r1", &RunStatus::Running).unwrap();
    assert!(!again);
}

#[test]
fn claim_waiting_run_on_unknown_id_is_false_not_error() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    // No run inserted — the UPDATE matches zero rows.
    let claimed =
        crate::db::workflows::claim_waiting_run(&conn, "ghost", &RunStatus::Running).unwrap();
    assert!(!claimed, "claiming a non-existent run is a clean false, not an error");
}

#[test]
fn workflow_runs_legacy_row_has_empty_produced_branches() {
    // A run inserted with no preserved branches → loaded back as Vec::new(),
    // not a parse error. Documents the migration-compatibility contract.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let run = sample_run("r1", "w1");
    assert!(run.produced_branches.is_empty(), "sample_run starts with empty");
    crate::db::workflows::insert_run(&conn, &run).unwrap();

    let loaded = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert!(loaded.produced_branches.is_empty());
}

#[test]
fn workflow_runs_produced_branches_survive_update() {
    // After an `update_run` that doesn't touch the field, prior
    // `produced_branches` must still be present on reload — validates the
    // SET clause in update_run_progress includes the column.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let mut run = sample_run("r1", "w1");
    run.produced_branches = vec![crate::models::ProducedBranch {
        branch_name: "kronn/Autobot/persisted".into(),
        head_sha: "deadbeef".into(),
        ahead: 1,
        pushed_upstream: false,
    }];
    crate::db::workflows::insert_run(&conn, &run).unwrap();

    // Mutate something else and persist.
    run.status = RunStatus::Success;
    run.finished_at = Some(Utc::now());
    crate::db::workflows::update_run(&conn, &run).unwrap();

    let loaded = crate::db::workflows::get_run(&conn, "r1").unwrap().unwrap();
    assert_eq!(loaded.produced_branches.len(), 1, "produced_branches must survive update");
    assert_eq!(loaded.produced_branches[0].branch_name, "kronn/Autobot/persisted");
}

#[test]
fn workflow_runs_count_active() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();

    let mut r2 = sample_run("r2", "w1");
    r2.status = RunStatus::Pending;
    crate::db::workflows::insert_run(&conn, &r2).unwrap();

    let mut r3 = sample_run("r3", "w1");
    r3.status = RunStatus::Success;
    crate::db::workflows::insert_run(&conn, &r3).unwrap();

    let count = crate::db::workflows::count_active_runs(&conn, "w1").unwrap();
    assert_eq!(count, 2); // Running + Pending
}

#[test]
fn has_running_run_false_when_no_runs() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    assert!(!crate::db::workflows::has_running_run(&conn).unwrap());
}

#[test]
fn has_running_run_true_when_running() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    // sample_run defaults to Running status.
    assert!(crate::db::workflows::has_running_run(&conn).unwrap());
}

#[test]
fn has_running_run_true_when_pending() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let mut r = sample_run("r1", "w1");
    r.status = RunStatus::Pending;
    crate::db::workflows::insert_run(&conn, &r).unwrap();
    assert!(crate::db::workflows::has_running_run(&conn).unwrap());
}

#[test]
fn has_running_run_false_when_only_terminal() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    for (id, status) in [
        ("r1", RunStatus::Success),
        ("r2", RunStatus::Failed),
        ("r3", RunStatus::Cancelled),
    ] {
        let mut r = sample_run(id, "w1");
        r.status = status;
        crate::db::workflows::insert_run(&conn, &r).unwrap();
    }
    // Even with 3 finished runs, there's nothing to defer host-sync for.
    assert!(!crate::db::workflows::has_running_run(&conn).unwrap());
}

#[test]
fn workflow_runs_delete_all() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();

    crate::db::workflows::delete_all_runs(&conn, "w1").unwrap();
    let runs = crate::db::workflows::list_runs(&conn, "w1").unwrap();
    assert!(runs.is_empty());
}

// ─── Batch runs (Phase 1b) ───────────────────────────────────────────────

/// Build a sample batch run. Caller must insert the placeholder workflow
/// first (or use `insert_batch_run_with_placeholder` below).
fn sample_batch_run(id: &str, qp_id: &str, total: u32) -> WorkflowRun {
    let now = Utc::now();
    WorkflowRun {
        id: id.into(),
        workflow_id: format!("qp:{}", qp_id),
        status: RunStatus::Running,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "batch".into(),
        batch_total: total,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: Some(format!("Cadrage — {}", id)),
        parent_run_id: None,
        state: ::std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    }
}

/// Insert a batch run with its placeholder workflow in one shot.
fn insert_batch_run_with_placeholder(conn: &rusqlite::Connection, id: &str, qp_id: &str, total: u32) {
    crate::db::workflows::ensure_batch_placeholder_workflow(conn, qp_id, "TestQP", None).unwrap();
    let run = sample_batch_run(id, qp_id, total);
    crate::db::workflows::insert_run(conn, &run).unwrap();
}

#[test]
fn batch_run_persists_fields() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br1", "qp-br1", 5);
    let loaded = crate::db::workflows::get_run(&conn, "br1").unwrap().unwrap();
    assert_eq!(loaded.run_type, "batch");
    assert_eq!(loaded.batch_total, 5);
    assert_eq!(loaded.batch_completed, 0);
    assert_eq!(loaded.batch_failed, 0);
    assert_eq!(loaded.batch_name.as_deref(), Some("Cadrage — br1"));
}

#[test]
fn batch_placeholder_workflow_is_hidden_from_list() {
    let conn = test_db();
    // Insert a real workflow AND a batch placeholder
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("real-wf")).unwrap();
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-test", "TestQP", None).unwrap();
    // Placeholder is in the DB but filtered out of list_workflows
    let visible = crate::db::workflows::list_workflows(&conn).unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "real-wf");
    // But get_workflow can still fetch it by id if needed (placeholder row exists)
}

#[test]
fn batch_placeholder_is_idempotent() {
    let conn = test_db();
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-x", "X", None).unwrap();
    // Second call must not error — INSERT OR IGNORE
    crate::db::workflows::ensure_batch_placeholder_workflow(&conn, "qp-x", "X", None).unwrap();
}

#[test]
fn batch_progress_increments_success_counter() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br2", "qp-br2", 3);

    // First child succeeds — not final yet
    let updated = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap();
    let updated = updated.unwrap();
    assert_eq!(updated.batch_completed, 1);
    assert_eq!(updated.batch_failed, 0);
    assert_eq!(updated.status, RunStatus::Running);

    // Second child succeeds
    let updated = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap().unwrap();
    assert_eq!(updated.batch_completed, 2);
    assert_eq!(updated.status, RunStatus::Running);

    // Third (last) child succeeds → run is marked Success and finished
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br2", true).unwrap().unwrap();
    assert_eq!(final_run.batch_completed, 3);
    assert_eq!(final_run.status, RunStatus::Success);
    assert!(final_run.finished_at.is_some());
}

#[test]
fn batch_progress_marks_failed_if_all_children_fail() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br3", "qp-br3", 2);

    crate::db::workflows::increment_batch_progress(&conn, "br3", false).unwrap();
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br3", false).unwrap().unwrap();
    assert_eq!(final_run.batch_failed, 2);
    assert_eq!(final_run.batch_completed, 0);
    // At least one success is needed to mark Success — otherwise Failed
    assert_eq!(final_run.status, RunStatus::Failed);
    assert!(final_run.finished_at.is_some());
}

#[test]
fn batch_progress_marks_success_if_at_least_one_child_succeeds() {
    let conn = test_db();
    insert_batch_run_with_placeholder(&conn, "br4", "qp-br4", 3);

    crate::db::workflows::increment_batch_progress(&conn, "br4", false).unwrap();
    crate::db::workflows::increment_batch_progress(&conn, "br4", true).unwrap();
    let final_run = crate::db::workflows::increment_batch_progress(&conn, "br4", false).unwrap().unwrap();
    assert_eq!(final_run.batch_completed, 1);
    assert_eq!(final_run.batch_failed, 2);
    // Mixed result — one success is enough to count as "the batch did something"
    assert_eq!(final_run.status, RunStatus::Success);
}

#[test]
fn batch_progress_ignores_linear_runs() {
    let conn = test_db();
    // A plain linear run — increment_batch_progress must be a no-op on it
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("lr1", "w1")).unwrap();
    let result = crate::db::workflows::increment_batch_progress(&conn, "lr1", true).unwrap();
    // The UPDATE is guarded on run_type = 'batch' so nothing was written,
    // and the helper returns None early once it loads the run and sees
    // it's not a batch. The caller treats None as "no-op, no broadcast".
    assert!(result.is_none(), "linear runs must be skipped by batch progress helper");
    // Verify the linear run is untouched in DB
    let unchanged = crate::db::workflows::get_run(&conn, "lr1").unwrap().unwrap();
    assert_eq!(unchanged.batch_completed, 0);
    assert_eq!(unchanged.status, RunStatus::Running);
}

#[test]
fn workflow_get_last_run() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();

    let last = crate::db::workflows::get_last_run(&conn, "w1").unwrap();
    assert!(last.is_some());
}

// ─── Tracker processed ──────────────────────────────────────────────────

#[test]
fn tracker_issue_processed() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();

    assert!(!crate::db::workflows::is_issue_processed(&conn, "w1", "issue-1").unwrap());

    crate::db::workflows::mark_issue_processed(&conn, "w1", "issue-1").unwrap();
    assert!(crate::db::workflows::is_issue_processed(&conn, "w1", "issue-1").unwrap());

    // Marking again should not fail (INSERT OR IGNORE)
    crate::db::workflows::mark_issue_processed(&conn, "w1", "issue-1").unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// Batch / Performance functions
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projects_get_names_batch() {
    let conn = test_db();
    crate::db::projects::insert_project(&conn, &sample_project("p1", "Alpha")).unwrap();
    crate::db::projects::insert_project(&conn, &sample_project("p2", "Beta")).unwrap();

    let names = crate::db::projects::get_project_names(&conn).unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names.get("p1").unwrap(), "Alpha");
    assert_eq!(names.get("p2").unwrap(), "Beta");
}

#[test]
fn projects_get_names_empty() {
    let conn = test_db();
    let names = crate::db::projects::get_project_names(&conn).unwrap();
    assert!(names.is_empty());
}

#[test]
fn discussions_list_does_not_load_messages() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();

    let discussions = crate::db::discussions::list_discussions(&conn).unwrap();
    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0].messages.len(), 0, "list_discussions should not load messages");
}

#[test]
fn discussions_list_with_messages_batch_loads() {
    let conn = test_db();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d1", None)).unwrap();
    crate::db::discussions::insert_discussion(&conn, &sample_discussion("d2", None)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m1", MessageRole::User)).unwrap();
    crate::db::discussions::insert_message(&conn, "d1", &sample_message("m2", MessageRole::Agent)).unwrap();
    crate::db::discussions::insert_message(&conn, "d2", &sample_message("m3", MessageRole::User)).unwrap();

    let discussions = crate::db::discussions::list_discussions_with_messages(&conn).unwrap();
    assert_eq!(discussions.len(), 2);

    let d1 = discussions.iter().find(|d| d.id == "d1").unwrap();
    let d2 = discussions.iter().find(|d| d.id == "d2").unwrap();
    assert_eq!(d1.messages.len(), 2);
    assert_eq!(d2.messages.len(), 1);
}

#[test]
fn workflow_get_last_runs_all_batch() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w2")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r1", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r2", "w1")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("r3", "w2")).unwrap();

    let last_runs = crate::db::workflows::get_last_runs_all(&conn).unwrap();
    assert_eq!(last_runs.len(), 2);
    assert!(last_runs.contains_key("w1"));
    assert!(last_runs.contains_key("w2"));
    assert_eq!(last_runs.get("w2").unwrap().id, "r3");
}

#[test]
fn workflow_get_last_runs_all_empty() {
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("w1")).unwrap();
    let last_runs = crate::db::workflows::get_last_runs_all(&conn).unwrap();
    assert!(last_runs.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2 — batch workflows chained from a linear workflow run
// ═══════════════════════════════════════════════════════════════════════════

/// Build a test QuickPrompt with a single `{{ticket}}` variable.
fn sample_qp_for_batch(id: &str) -> QuickPrompt {
    let now = Utc::now();
    QuickPrompt {
        id: id.into(),
        name: format!("BatchQP {}", id),
        icon: "🎯".into(),
        prompt_template: "Analyse le ticket {{ticket}} en profondeur".into(),
        variables: vec![crate::models::PromptVariable {
            name: "ticket".into(),
            label: "Ticket".into(),
            placeholder: "EW-123".into(),
            description: None,
            required: true,
            pattern: None,
        }],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: "Test QP for batch chaining".into(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn create_batch_run_pure_fn_roundtrip_toplevel() {
    let conn = test_db();
    let qp = sample_qp_for_batch("qp-pure-1");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![
                crate::db::workflows::BatchItemInput {

                    title: "EW-100".into(),

                    prompt: "Analyse le ticket EW-100 en profondeur".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-101".into(),

                    prompt: "Analyse le ticket EW-101 en profondeur".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-102".into(),

                    prompt: "Analyse le ticket EW-102 en profondeur".into(),

                    agent_override: None,

                },
            ],
            batch_name: Some("Cadrage hebdo".into()),
            project_id: None,
            parent_run_id: None, // top-level batch, no parent
            author_pseudo: Some("TestUser".into()),
            author_avatar_email: Some("test@example.com".into()),
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    ).unwrap();

    assert_eq!(outcome.batch_total, 3);
    assert_eq!(outcome.discussion_ids.len(), 3);

    // Run is persisted with parent_run_id = None
    let run = crate::db::workflows::get_run(&conn, &outcome.run_id).unwrap().unwrap();
    assert_eq!(run.run_type, "batch");
    assert_eq!(run.batch_total, 3);
    assert_eq!(run.batch_name.as_deref(), Some("Cadrage hebdo"));
    assert_eq!(run.parent_run_id, None);
    assert_eq!(run.status, RunStatus::Running);

    // All three child discussions exist and link back via workflow_run_id
    for (i, disc_id) in outcome.discussion_ids.iter().enumerate() {
        let disc = crate::db::discussions::get_discussion(&conn, disc_id).unwrap().unwrap();
        assert_eq!(disc.workflow_run_id.as_ref(), Some(&outcome.run_id));
        assert_eq!(disc.title, format!("EW-{}", 100 + i));
        assert_eq!(disc.messages.len(), 1);
        assert_eq!(disc.messages[0].content, format!("Analyse le ticket EW-{} en profondeur", 100 + i));
        assert_eq!(disc.messages[0].author_pseudo.as_deref(), Some("TestUser"));
    }
}

#[test]
fn create_batch_run_chained_from_linear_parent() {
    let conn = test_db();

    // 1. Create a real linear workflow and run it (the would-be parent)
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("linear-wf")).unwrap();
    let parent = sample_run("parent-linear-1", "linear-wf");
    crate::db::workflows::insert_run(&conn, &parent).unwrap();

    // 2. Insert a QP and call create_batch_run with parent_run_id = Some(...)
    let qp = sample_qp_for_batch("qp-chained");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![
                crate::db::workflows::BatchItemInput {

                    title: "EW-200".into(),

                    prompt: "rendered prompt A".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-201".into(),

                    prompt: "rendered prompt B".into(),

                    agent_override: None,

                },
            ],
            batch_name: Some("Chained batch".into()),
            project_id: None,
            parent_run_id: Some("parent-linear-1".into()),
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    ).unwrap();

    // 3. The child batch run persists parent_run_id back to the linear parent
    let child = crate::db::workflows::get_run(&conn, &outcome.run_id).unwrap().unwrap();
    assert_eq!(child.parent_run_id.as_deref(), Some("parent-linear-1"));
    assert_eq!(child.run_type, "batch");
    assert_eq!(child.batch_total, 2);

    // 4. The linear parent is untouched (still Running, no batch_total)
    let parent_reloaded = crate::db::workflows::get_run(&conn, "parent-linear-1").unwrap().unwrap();
    assert_eq!(parent_reloaded.run_type, "linear");
    assert_eq!(parent_reloaded.batch_total, 0);
    assert_eq!(parent_reloaded.parent_run_id, None);
}

#[test]
fn partial_response_set_then_recover_inserts_agent_message() {
    // Simulates: agent runs → checkpoints partial → backend dies →
    // boot scan calls recover_partial_responses → user sees a saved Agent
    // message with the "interrupted" footer.
    let conn = test_db();

    // Create a discussion with a user message
    let now = chrono::Utc::now();
    let disc = Discussion {
        id: "disc-pr-1".into(),
        project_id: None,
        title: "Test".into(),
        agent: AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![AgentType::ClaudeCode],
        messages: vec![],
        message_count: 0, non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();

    // Simulate the agent task checkpointing some thinking
    let partial = "I am in the middle of analyzing the issue. Let me look at the code...";
    crate::db::discussions::set_partial_response(&conn, "disc-pr-1", Some(partial)).unwrap();

    // Verify it's there
    let stored: Option<String> = conn.query_row(
        "SELECT partial_response FROM discussions WHERE id = 'disc-pr-1'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(stored.as_deref(), Some(partial));

    // Boot scan recovers it
    let recovered = crate::db::discussions::recover_partial_responses(&conn).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0], "disc-pr-1");

    // partial_response is now cleared
    let after: Option<String> = conn.query_row(
        "SELECT partial_response FROM discussions WHERE id = 'disc-pr-1'", [], |r| r.get(0),
    ).unwrap();
    assert!(after.is_none(), "partial_response must be cleared after recovery");

    // A new Agent message exists with the partial + footer
    let msg_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE discussion_id = 'disc-pr-1'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(msg_count, 1);
    let (role, content): (String, String) = conn.query_row(
        "SELECT role, content FROM messages WHERE discussion_id = 'disc-pr-1'", [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    ).unwrap();
    assert_eq!(role, "Agent");
    assert!(content.starts_with(partial), "Recovered message must start with the partial");
    assert!(content.contains("Réflexion interrompue"), "Footer must be appended");
}

#[test]
fn partial_response_recovery_idempotent_when_nothing_to_recover() {
    let conn = test_db();
    // No discs with partial_response → returns 0, no errors
    let n = crate::db::discussions::recover_partial_responses(&conn).unwrap();
    assert!(n.is_empty());
    // Run again — still empty
    let n2 = crate::db::discussions::recover_partial_responses(&conn).unwrap();
    assert!(n2.is_empty());
}

#[test]
fn partial_response_preserves_started_at_across_checkpoints() {
    // Regression for the 2026-04-13 double-response bug: the recovered
    // Agent message must inherit the start time of the in-flight run, so
    // it falls chronologically BEFORE any user message posted after restart.
    let conn = test_db();
    let now = chrono::Utc::now();
    let disc = Discussion {
        id: "disc-ts".into(), project_id: None, title: "X".into(),
        agent: AgentType::ClaudeCode, language: "fr".into(),
        participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
        skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        archived: false,
        pinned: false, workspace_mode: "Direct".into(),
        workspace_path: None, worktree_branch: None, tier: ModelTier::Default,
        model: None,
        pin_first_message: false, summary_cache: None, summary_up_to_msg_idx: None, summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
        shared_id: None, shared_with: vec![], workflow_run_id: None,
        test_mode_restore_branch: None, test_mode_stash_ref: None,
        created_at: now, updated_at: now,
    };
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();

    // First checkpoint sets the started_at
    crate::db::discussions::set_partial_response(&conn, "disc-ts", Some("draft v1")).unwrap();
    let first_ts: String = conn.query_row(
        "SELECT partial_response_started_at FROM discussions WHERE id = 'disc-ts'", [], |r| r.get(0),
    ).unwrap();
    assert!(!first_ts.is_empty(), "First checkpoint must populate started_at");

    // Wait a tick (in real life: 30s of agent thinking) — second checkpoint
    // updates `partial_response` but MUST NOT shift `started_at`.
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::db::discussions::set_partial_response(&conn, "disc-ts", Some("draft v2 longer")).unwrap();
    let second_ts: String = conn.query_row(
        "SELECT partial_response_started_at FROM discussions WHERE id = 'disc-ts'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(first_ts, second_ts, "started_at must be preserved across updates");

    // Recovery uses the started_at, not now()
    let ids = crate::db::discussions::recover_partial_responses(&conn).unwrap();
    assert_eq!(ids, vec!["disc-ts"]);
    let msg_ts_str: String = conn.query_row(
        "SELECT timestamp FROM messages WHERE discussion_id = 'disc-ts'", [], |r| r.get(0),
    ).unwrap();
    let msg_ts = chrono::DateTime::parse_from_rfc3339(&msg_ts_str).unwrap();
    let started = chrono::DateTime::parse_from_rfc3339(&first_ts).unwrap();
    // Tolerate sub-second drift but not seconds — the recovered message
    // must use the checkpoint time, not Utc::now() at recovery moment.
    let drift = (msg_ts - started).num_milliseconds().abs();
    assert!(drift < 100, "Recovered message timestamp must match started_at within 100ms (got {}ms drift)", drift);
}

#[test]
fn has_pending_partial_returns_true_when_set() {
    let conn = test_db();
    let now = chrono::Utc::now();
    let disc = Discussion {
        id: "disc-pending".into(), project_id: None, title: "X".into(),
        agent: AgentType::ClaudeCode, language: "fr".into(),
        participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
        skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        archived: false,
        pinned: false, workspace_mode: "Direct".into(),
        workspace_path: None, worktree_branch: None, tier: ModelTier::Default,
        model: None,
        pin_first_message: false, summary_cache: None, summary_up_to_msg_idx: None, summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
        shared_id: None, shared_with: vec![], workflow_run_id: None,
        test_mode_restore_branch: None, test_mode_stash_ref: None,
        created_at: now, updated_at: now,
    };
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();
    assert!(!crate::db::discussions::has_pending_partial(&conn, "disc-pending").unwrap());
    crate::db::discussions::set_partial_response(&conn, "disc-pending", Some("hi")).unwrap();
    assert!(crate::db::discussions::has_pending_partial(&conn, "disc-pending").unwrap());
    crate::db::discussions::set_partial_response(&conn, "disc-pending", None).unwrap();
    assert!(!crate::db::discussions::has_pending_partial(&conn, "disc-pending").unwrap());
}

#[test]
fn partial_response_clear_with_none_wipes_column() {
    let conn = test_db();
    let now = chrono::Utc::now();
    let disc = Discussion {
        id: "disc-clear".into(), project_id: None, title: "X".into(),
        agent: AgentType::ClaudeCode, language: "fr".into(),
        participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
        skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        archived: false,
        pinned: false, workspace_mode: "Direct".into(),
        workspace_path: None, worktree_branch: None, tier: ModelTier::Default,
        model: None,
        pin_first_message: false, summary_cache: None, summary_up_to_msg_idx: None, summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
        shared_id: None, shared_with: vec![], workflow_run_id: None,
        test_mode_restore_branch: None, test_mode_stash_ref: None,
        created_at: now, updated_at: now,
    };
    crate::db::discussions::insert_discussion(&conn, &disc).unwrap();
    crate::db::discussions::set_partial_response(&conn, "disc-clear", Some("draft")).unwrap();
    crate::db::discussions::set_partial_response(&conn, "disc-clear", None).unwrap();
    let (after, after_ts): (Option<String>, Option<String>) = conn.query_row(
        "SELECT partial_response, partial_response_started_at FROM discussions WHERE id = 'disc-clear'",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert!(after.is_none(), "partial_response must be cleared");
    assert!(after_ts.is_none(), "partial_response_started_at must be cleared too");
}

#[test]
fn delete_batch_run_cascades_discussions_and_messages() {
    // Bulk-deleting a batch group from the sidebar must wipe the batch run row,
    // every child discussion, and (via the FK cascade on messages.discussion_id)
    // every message in those discussions.
    let conn = test_db();
    let qp = sample_qp_for_batch("qp-del");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![
                crate::db::workflows::BatchItemInput {

                    title: "EW-1".into(),

                    prompt: "p1".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-2".into(),

                    prompt: "p2".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-3".into(),

                    prompt: "p3".into(),

                    agent_override: None,

                },
            ],
            batch_name: Some("To-be-deleted".into()),
            project_id: None,
            parent_run_id: None,
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    ).unwrap();

    // Sanity: 3 discs + 3 initial messages exist
    let n_discs_before: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussions WHERE workflow_run_id = ?1",
        rusqlite::params![&outcome.run_id], |r| r.get(0),
    ).unwrap();
    assert_eq!(n_discs_before, 3);
    let n_msgs_before: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE discussion_id IN \
         (SELECT id FROM discussions WHERE workflow_run_id = ?1)",
        rusqlite::params![&outcome.run_id], |r| r.get(0),
    ).unwrap();
    assert_eq!(n_msgs_before, 3);

    // Delete the batch
    let summary = crate::db::workflows::delete_batch_run_with_discussions(
        &conn, &outcome.run_id,
    ).unwrap();
    assert_eq!(summary.discussions_deleted, 3);
    assert_eq!(summary.run_id, outcome.run_id);

    // Run row gone
    assert!(crate::db::workflows::get_run(&conn, &outcome.run_id).unwrap().is_none());
    // Discussions gone
    let n_discs_after: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussions WHERE workflow_run_id = ?1",
        rusqlite::params![&outcome.run_id], |r| r.get(0),
    ).unwrap();
    assert_eq!(n_discs_after, 0);
    // Messages cascaded out (would be 3 if cascade was broken)
    for disc_id in &outcome.discussion_ids {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1",
            rusqlite::params![disc_id], |r| r.get(0),
        ).unwrap();
        assert_eq!(n, 0, "Messages of disc {} should have cascaded", disc_id);
    }
}

#[test]
fn delete_batch_run_refuses_linear_runs() {
    // The endpoint is wired to the sidebar's "delete batch group" action.
    // If a linear run id leaks in (UI bug, manual API call), refuse hard
    // rather than silently nuking the wrong thing.
    let conn = test_db();
    crate::db::workflows::insert_workflow(&conn, &sample_workflow("wf-linear")).unwrap();
    crate::db::workflows::insert_run(&conn, &sample_run("linear-run-1", "wf-linear")).unwrap();

    let result = crate::db::workflows::delete_batch_run_with_discussions(
        &conn, "linear-run-1",
    );
    assert!(result.is_err(), "Should reject linear runs");
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("not 'batch'"), "Error must mention the type mismatch: {}", err);
    // Run still exists (transaction rolled back / never started)
    assert!(crate::db::workflows::get_run(&conn, "linear-run-1").unwrap().is_some());
}

#[test]
fn delete_batch_run_unknown_id_errors() {
    let conn = test_db();
    let result = crate::db::workflows::delete_batch_run_with_discussions(
        &conn, "does-not-exist",
    );
    assert!(result.is_err());
    assert!(format!("{}", result.unwrap_err()).contains("not found"));
}

#[test]
fn create_batch_run_isolated_mode_persists_on_children() {
    // When workspace_mode is Isolated, every child disc should carry that
    // mode — the per-disc worktree is then auto-created by make_agent_stream
    // on the first agent run.
    let conn = test_db();
    // NOTE: project_id is intentionally None here. This test focuses on
    // workspace_mode persistence — the Isolated mode "requires project_id"
    // safety check lives at a higher layer (the BatchQuickPrompt step
    // executor in workflows::batch_step), not inside create_batch_run itself.
    let qp = sample_qp_for_batch("qp-iso");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![
                crate::db::workflows::BatchItemInput {

                    title: "EW-1".into(),

                    prompt: "prompt 1".into(),

                    agent_override: None,

                },
                crate::db::workflows::BatchItemInput {

                    title: "EW-2".into(),

                    prompt: "prompt 2".into(),

                    agent_override: None,

                },
            ],
            batch_name: Some("Isolated batch".into()),
            project_id: None,
            parent_run_id: None,
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Isolated".into(),
        },
    ).unwrap();

    for disc_id in &outcome.discussion_ids {
        let disc = crate::db::discussions::get_discussion(&conn, disc_id).unwrap().unwrap();
        assert_eq!(disc.workspace_mode, "Isolated");
    }
}

#[test]
fn create_batch_run_direct_mode_is_default_when_empty() {
    // Passing an empty workspace_mode string falls back to "Direct" — we
    // don't want to crash later just because a caller forgot to set it.
    let conn = test_db();
    let qp = sample_qp_for_batch("qp-default");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![crate::db::workflows::BatchItemInput { title: "EW-1".into(), prompt: "prompt 1".into(), agent_override: None }],
            batch_name: None,
            project_id: None,
            parent_run_id: None,
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "".into(),
        },
    ).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, &outcome.discussion_ids[0]).unwrap().unwrap();
    assert_eq!(disc.workspace_mode, "Direct");
}

#[test]
fn create_batch_run_sets_workflow_run_id_on_discussions() {
    // Regression: child discussions MUST link back to the batch run via
    // workflow_run_id, otherwise the sidebar grouping + batch progress hooks
    // in api::discussions won't fire.
    let conn = test_db();
    let qp = sample_qp_for_batch("qp-link");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    let outcome = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![crate::db::workflows::BatchItemInput { title: "X-1".into(), prompt: "prompt 1".into(), agent_override: None }],
            batch_name: None,
            project_id: None,
            parent_run_id: None,
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    ).unwrap();

    let disc = crate::db::discussions::get_discussion(&conn, &outcome.discussion_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(disc.workflow_run_id.as_ref(), Some(&outcome.run_id));
}

// ═══════════════════════════════════════════════════════════════════════════
// Workflow multi-step
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn workflow_multi_step_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let wf = Workflow {
        id: "wm1".into(),
        name: "Multi-step".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "analyze".into(),
                agent: AgentType::ClaudeCode,
                prompt_template: "Analyze this".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![],
                on_timeout: None,
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: None,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                batch_quick_prompt_id: None,
                batch_items_from: None,
                batch_wait_for_completion: None,
                batch_max_items: None,
                batch_workspace_mode: None,
                batch_chain_prompt_ids: vec![],
                batch_concurrent_limit: None,
                quick_api_id: None,
                notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
            },
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "fix".into(),
                agent: AgentType::Codex,
                prompt_template: "Fix: {{previous_step.output}}".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![StepConditionRule {
                    contains: "NO_RESULTS".into(),
                    action: ConditionAction::Stop,
                }],
                on_timeout: None,
                stall_timeout_secs: Some(300),
                retry: None,
                delay_after_secs: None,
                skill_ids: vec!["token-saver".into()],
                profile_ids: vec![],
                directive_ids: vec![],
                batch_quick_prompt_id: None,
                batch_items_from: None,
                batch_wait_for_completion: None,
                batch_max_items: None,
                batch_workspace_mode: None,
                batch_chain_prompt_ids: vec![],
                batch_concurrent_limit: None,
                quick_api_id: None,
                notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
            },
            WorkflowStep {
                step_type: StepType::default(),
                output_format: StepOutputFormat::default(),
                description: None,
                name: "review".into(),
                agent: AgentType::GeminiCli,
                prompt_template: "Review the changes".into(),
                mode: StepMode::Normal,
                mcp_config_ids: vec![],
                agent_settings: None,
                on_result: vec![],
                on_timeout: None,
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: Some(5),
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                batch_quick_prompt_id: None,
                batch_items_from: None,
                batch_wait_for_completion: None,
                batch_max_items: None,
                batch_workspace_mode: None,
                batch_chain_prompt_ids: vec![],
                batch_concurrent_limit: None,
                quick_api_id: None,
                notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
            },
        ],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        guards: None,
        artifacts: ::std::collections::HashMap::new(),
        on_failure: vec![],
        exec_allowlist: vec![],
        variables: vec![],
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    let loaded = crate::db::workflows::get_workflow(&conn, "wm1").unwrap().unwrap();
    assert_eq!(loaded.steps.len(), 3);
    assert_eq!(loaded.steps[0].name, "analyze");
    assert_eq!(loaded.steps[0].agent, AgentType::ClaudeCode);
    assert_eq!(loaded.steps[1].name, "fix");
    assert_eq!(loaded.steps[1].agent, AgentType::Codex);
    assert_eq!(loaded.steps[1].on_result.len(), 1);
    assert_eq!(loaded.steps[1].on_result[0].contains, "NO_RESULTS");
    assert_eq!(loaded.steps[1].stall_timeout_secs, Some(300));
    assert_eq!(loaded.steps[1].skill_ids, vec!["token-saver"]);
    assert_eq!(loaded.steps[2].name, "review");
    assert_eq!(loaded.steps[2].agent, AgentType::GeminiCli);
    assert_eq!(loaded.steps[2].delay_after_secs, Some(5));
}

#[test]
fn workflow_update_steps_count() {
    let conn = test_db();
    let mut wf = sample_workflow("wu1");
    assert_eq!(wf.steps.len(), 1);
    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    // Add a second step
    wf.steps.push(WorkflowStep {
        step_type: StepType::default(),
        output_format: StepOutputFormat::default(),
        description: None,
        name: "step2".into(),
        agent: AgentType::ClaudeCode,
        prompt_template: "Second step".into(),
        mode: StepMode::Normal,
        mcp_config_ids: vec![],
        agent_settings: None,
        on_result: vec![],
        on_timeout: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: None,
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: None,
        quick_api_id: None,
        notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
    });
    crate::db::workflows::update_workflow(&conn, &wf).unwrap();

    let loaded = crate::db::workflows::get_workflow(&conn, "wu1").unwrap().unwrap();
    assert_eq!(loaded.steps.len(), 2);
    assert_eq!(loaded.steps[1].name, "step2");
    assert_eq!(loaded.steps[1].prompt_template, "Second step");
}

#[test]
fn workflow_batch_chain_prompt_ids_roundtrip() {
    // Regression: `batch_chain_prompt_ids` (QP Chain Phase 2) must survive a
    // DB save/load cycle. Empty vec should serialize as missing (skip-if-empty)
    // and still deserialize back to an empty vec.
    let conn = test_db();
    let now = Utc::now();
    let mut wf = sample_workflow("wc1");
    wf.steps[0].step_type = StepType::BatchQuickPrompt;
    wf.steps[0].batch_quick_prompt_id = Some("qp-init".into());
    wf.steps[0].batch_chain_prompt_ids = vec![
        "qp-review".into(),
        "qp-summary".into(),
    ];
    wf.created_at = now;
    wf.updated_at = now;
    crate::db::workflows::insert_workflow(&conn, &wf).unwrap();

    let loaded = crate::db::workflows::get_workflow(&conn, "wc1")
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.steps[0].batch_chain_prompt_ids,
        vec!["qp-review".to_string(), "qp-summary".to_string()],
        "chain IDs must survive the roundtrip in order"
    );

    // Empty chain should also roundtrip cleanly (skip_serializing_if).
    let wf_empty = sample_workflow("wc2");
    crate::db::workflows::insert_workflow(&conn, &wf_empty).unwrap();
    let loaded_empty = crate::db::workflows::get_workflow(&conn, "wc2")
        .unwrap()
        .unwrap();
    assert!(
        loaded_empty.steps[0].batch_chain_prompt_ids.is_empty(),
        "empty chain must stay empty after roundtrip"
    );
}

#[test]
fn workflow_step_chain_serde_skip_empty() {
    // `batch_chain_prompt_ids` has `skip_serializing_if = "Vec::is_empty"`.
    // Empty = absent from the JSON payload (keeps existing workflow JSON
    // stable for users who never configured a chain).
    let mut step = sample_workflow("wcs1").steps.into_iter().next().unwrap();
    step.batch_chain_prompt_ids = vec![];
    let json = serde_json::to_string(&step).unwrap();
    assert!(
        !json.contains("batch_chain_prompt_ids"),
        "empty chain vec must be skipped; got: {}",
        json
    );

    step.batch_chain_prompt_ids = vec!["qp-a".into(), "qp-b".into()];
    let json = serde_json::to_string(&step).unwrap();
    assert!(
        json.contains("\"batch_chain_prompt_ids\":[\"qp-a\",\"qp-b\"]"),
        "non-empty chain must serialize in order; got: {}",
        json
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// MCP Config — secrets_broken detection
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn mcp_config_display_secrets_broken_when_decrypt_fails() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();
    let secret_b = crate::core::crypto::generate_secret();

    // Insert a server first
    let server = McpServer {
        id: "srv-broken-test".into(),
        name: "TestServer".into(),
        description: "Test server for secrets_broken".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    // Create env and encrypt with secret_a
    let mut env = std::collections::HashMap::new();
    env.insert("TOKEN".to_string(), "test-value-alpha".to_string());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret_a).unwrap();

    let config = McpConfig {
        id: "cfg-broken-test".into(),
        server_id: "srv-broken-test".into(),
        label: "TestConfig".into(),
        env_keys: vec!["TOKEN".into()],
        env_encrypted: encrypted,
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-broken-test".into(),
        project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Decrypt with wrong secret (secret_b) → secrets_broken should be true
    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_b)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(display[0].secrets_broken, "secrets_broken should be true when decryption fails with wrong key");
}

#[test]
fn mcp_config_display_secrets_ok_when_decrypt_succeeds() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv-ok-test".into(),
        name: "TestServerOk".into(),
        description: "Test".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let mut env = std::collections::HashMap::new();
    env.insert("SECRET_KEY".to_string(), "value-beta".to_string());
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret_a).unwrap();

    let config = McpConfig {
        id: "cfg-ok-test".into(),
        server_id: "srv-ok-test".into(),
        label: "OkConfig".into(),
        env_keys: vec!["SECRET_KEY".into()],
        env_encrypted: encrypted,
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-ok-test".into(),
        project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    // Decrypt with correct secret → secrets_broken should be false
    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_a)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(!display[0].secrets_broken, "secrets_broken should be false when decryption succeeds");
}

#[test]
fn mcp_config_display_secrets_broken_false_when_no_env() {
    let conn = test_db();
    let secret_a = crate::core::crypto::generate_secret();

    let server = McpServer {
        id: "srv-noenv-test".into(),
        name: "TestServerNoEnv".into(),
        description: "Test".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "test-pkg".into()],
        },
        source: McpSource::Registry,
        api_spec: None,
    };
    crate::db::mcps::upsert_server(&conn, &server).unwrap();

    let config = McpConfig {
        id: "cfg-noenv-test".into(),
        server_id: "srv-noenv-test".into(),
        label: "NoEnvConfig".into(),
        env_keys: vec![],        // no env keys
        env_encrypted: String::new(), // no encrypted data
        args_override: None,
        is_global: false,
        include_general: true,
        config_hash: "hash-noenv-test".into(),
        project_ids: vec![], host_sync: HostSyncMode::None,
    };
    crate::db::mcps::insert_config(&conn, &config).unwrap();

    let display = crate::db::mcps::list_configs_display(&conn, Some(&secret_a)).unwrap();
    assert_eq!(display.len(), 1);
    assert!(!display[0].secrets_broken, "secrets_broken should be false when no env keys exist");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Project path remapping
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn update_project_path_success() {
    let conn = test_db();
    let p = sample_project("p-remap", "RemapTest");
    crate::db::projects::insert_project(&conn, &p).unwrap();

    let updated = crate::db::projects::update_project_path(&conn, "p-remap", "/new/path").unwrap();
    assert!(updated, "update_project_path should return true for existing project");

    let project = crate::db::projects::get_project(&conn, "p-remap").unwrap().unwrap();
    assert_eq!(project.path, "/new/path");
}

#[test]
fn update_project_path_nonexistent() {
    let conn = test_db();
    let updated = crate::db::projects::update_project_path(&conn, "nonexistent", "/any").unwrap();
    assert!(!updated, "update_project_path should return false for nonexistent project");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contact export/import round-trip
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn contacts_export_import_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let contact = Contact {
        id: "c1".into(),
        pseudo: "PeerAlpha".into(),
        avatar_email: Some("alpha@test.dev".into()),
        kronn_url: "http://localhost:3140".into(),
        invite_code: "abc123".into(),
        status: "accepted".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::contacts::insert_contact(&conn, &contact).unwrap();

    let contacts = crate::db::contacts::list_contacts(&conn).unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].pseudo, "PeerAlpha");

    // Simulate import: clear and re-insert
    conn.execute_batch("DELETE FROM contacts;").unwrap();
    assert_eq!(crate::db::contacts::list_contacts(&conn).unwrap().len(), 0);

    crate::db::contacts::insert_contact(&conn, &contact).unwrap();
    let reimported = crate::db::contacts::list_contacts(&conn).unwrap();
    assert_eq!(reimported.len(), 1);
    assert_eq!(reimported[0].id, "c1");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts CRUD
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn quick_prompt_crud() {
    let conn = test_db();
    let now = Utc::now();

    // Insert
    let qp = QuickPrompt {
        id: "qp1".into(),
        name: "Analyse ticket".into(),
        icon: "🔍".into(),
        prompt_template: "Analyse le ticket {{ticket}} sur le projet {{project}}".into(),
        variables: vec![
            crate::models::PromptVariable {
                name: "ticket".into(), label: "Ticket".into(), placeholder: "PROJ-123".into(),
                description: Some("Identifiant Jira du ticket à analyser".into()), required: true, pattern: None,
            },
            crate::models::PromptVariable {
                name: "project".into(), label: "Projet".into(), placeholder: "acme-frontend".into(),
                description: None, required: true, pattern: None,
            },
        ],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec!["coder".into()],
        directive_ids: vec!["concise".into()],
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: "Analyse technique d'un ticket Jira pour cadrage".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    // List
    let all = crate::db::quick_prompts::list_quick_prompts(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Analyse ticket");
    assert_eq!(all[0].variables.len(), 2);
    assert_eq!(all[0].variables[0].name, "ticket");

    // Get
    let found = crate::db::quick_prompts::get_quick_prompt(&conn, "qp1").unwrap();
    assert!(found.is_some());
    let found_qp = found.unwrap();
    assert_eq!(found_qp.prompt_template, "Analyse le ticket {{ticket}} sur le projet {{project}}");
    // v2 fields: description on the QP + per-variable description/required
    assert_eq!(found_qp.description, "Analyse technique d'un ticket Jira pour cadrage");
    assert_eq!(found_qp.variables[0].description.as_deref(), Some("Identifiant Jira du ticket à analyser"));
    assert!(found_qp.variables[0].required);
    assert!(found_qp.variables[1].description.is_none());
    // 0.8.5 — profile/directive ids roundtrip.
    assert_eq!(found_qp.profile_ids, vec!["coder".to_string()]);
    assert_eq!(found_qp.directive_ids, vec!["concise".to_string()]);

    // Update
    let mut updated = qp.clone();
    updated.name = "Analyse ticket v2".into();
    updated.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &updated).unwrap();
    let found2 = crate::db::quick_prompts::get_quick_prompt(&conn, "qp1").unwrap().unwrap();
    assert_eq!(found2.name, "Analyse ticket v2");

    // Delete
    crate::db::quick_prompts::delete_quick_prompt(&conn, "qp1").unwrap();
    let all2 = crate::db::quick_prompts::list_quick_prompts(&conn).unwrap();
    assert_eq!(all2.len(), 0);
}

// ─── 0.8.5 — QP version history + metrics ─────────────────────────

#[test]
fn quick_prompt_insert_seeds_version_v1() {
    // After insert_quick_prompt, list_quick_prompt_versions should
    // return exactly one row with version_index = 1.
    let conn = test_db();
    let now = Utc::now();
    let qp = crate::models::QuickPrompt {
        id: "qp-versions-1".into(),
        name: "QP V1".into(),
        icon: "⚡".into(),
        prompt_template: "Initial body".into(),
        variables: vec![],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: "v1".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    let versions = crate::db::quick_prompts::list_quick_prompt_versions(&conn, "qp-versions-1").unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version_index, 1);
    assert_eq!(versions[0].prompt_template, "Initial body");
    assert_eq!(versions[0].description, "v1");
}

#[test]
fn quick_prompt_update_snapshots_v2_v3() {
    // Each update_quick_prompt appends a new snapshot with the NEW
    // body and version_index = max + 1.
    let conn = test_db();
    let now = Utc::now();
    let mut qp = crate::models::QuickPrompt {
        id: "qp-versions-2".into(),
        name: "Init".into(),
        icon: "⚡".into(),
        prompt_template: "v1 body".into(),
        variables: vec![],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: "v1".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    qp.prompt_template = "v2 body".into();
    qp.description = "v2".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();

    qp.prompt_template = "v3 body".into();
    qp.description = "v3".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();

    let versions = crate::db::quick_prompts::list_quick_prompt_versions(&conn, "qp-versions-2").unwrap();
    // Newest first: v3, v2, v1
    assert_eq!(versions.len(), 3);
    assert_eq!(versions[0].version_index, 3);
    assert_eq!(versions[0].prompt_template, "v3 body");
    assert_eq!(versions[1].version_index, 2);
    assert_eq!(versions[1].prompt_template, "v2 body");
    assert_eq!(versions[2].version_index, 1);
    assert_eq!(versions[2].prompt_template, "v1 body");

    // current_version_index() returns the highest
    let cur = crate::db::quick_prompts::current_version_index(&conn, "qp-versions-2").unwrap();
    assert_eq!(cur, Some(3));
}

#[test]
fn quick_prompt_metrics_aggregates_first_agent_reply_per_version() {
    use crate::models::{Discussion, DiscussionMessage, MessageRole, AgentType, ModelTier};
    let conn = test_db();
    let now = Utc::now();
    let mut qp = crate::models::QuickPrompt {
        id: "qp-metrics".into(),
        name: "Metrics QP".into(),
        icon: "⚡".into(),
        prompt_template: "v1".into(),
        variables: vec![],
        agent: AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: ModelTier::Default,
        agent_settings: None,
        description: "".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    // Bump to v2
    qp.prompt_template = "v2".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();

    // Seed 2 launches on v1 (tokens 1000+2000, duration 5000+7000)
    // and 1 launch on v2 (tokens 800, duration 3000).
    let seed_disc = |disc_id: &str, v: u32, agent_tokens: u64, agent_dur: u64| {
        let d = Discussion {
            id: disc_id.into(), project_id: None, title: format!("Disc {}", disc_id),
            agent: AgentType::ClaudeCode, language: "fr".into(),
            participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
            skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
            archived: false, pinned: false,
            workspace_mode: "Direct".into(), workspace_path: None, worktree_branch: None,
            tier: ModelTier::Default, pin_first_message: false,
            model: None,
            summary_cache: None, summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
            shared_id: None, shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None, test_mode_stash_ref: None,
            created_at: Utc::now(), updated_at: Utc::now(),
        };
        crate::db::discussions::insert_discussion(&conn, &d).unwrap();
        // User msg + Agent msg (with tokens + duration).
        let user_msg = DiscussionMessage {
            model: None,
            lint_report: None,
            id: format!("{}-u", disc_id), role: MessageRole::User, content: "ask".into(),
            agent_type: None, timestamp: Utc::now(), tokens_used: 0,
            auth_mode: None, model_tier: None, cost_usd: None,
            author_pseudo: None, author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };
        let agent_msg = DiscussionMessage {
            model: None,
            lint_report: None,
            id: format!("{}-a", disc_id), role: MessageRole::Agent, content: "reply".into(),
            agent_type: Some(AgentType::ClaudeCode), timestamp: Utc::now(),
            tokens_used: agent_tokens, auth_mode: None,
            model_tier: None, cost_usd: None,
            author_pseudo: None, author_avatar_email: None, source_msg_id: None,
            duration_ms: Some(agent_dur),
        };
        crate::db::discussions::insert_message(&conn, disc_id, &user_msg).unwrap();
        crate::db::discussions::insert_message(&conn, disc_id, &agent_msg).unwrap();
        crate::db::discussions::set_originating_qp(&conn, disc_id, "qp-metrics", v).unwrap();
    };
    seed_disc("d-v1-a", 1, 1000, 5000);
    seed_disc("d-v1-b", 1, 2000, 7000);
    seed_disc("d-v2-a", 2, 800,  3000);

    let metrics = crate::db::quick_prompts::list_quick_prompt_version_metrics(&conn, "qp-metrics").unwrap();
    // Newest first: v2 then v1
    assert_eq!(metrics.len(), 2);
    assert_eq!(metrics[0].version_index, 2);
    assert_eq!(metrics[0].launches, 1);
    assert_eq!(metrics[0].avg_tokens, 800);
    assert_eq!(metrics[0].avg_duration_ms, Some(3000));
    assert_eq!(metrics[1].version_index, 1);
    assert_eq!(metrics[1].launches, 2);
    assert_eq!(metrics[1].avg_tokens, 1500);
    assert_eq!(metrics[1].avg_duration_ms, Some(6000));
}

#[test]
fn quick_prompt_metrics_empty_for_qp_without_launches() {
    let conn = test_db();
    let now = Utc::now();
    let qp = crate::models::QuickPrompt {
        id: "qp-no-launches".into(),
        name: "Solo".into(),
        icon: "⚡".into(),
        prompt_template: "...".into(),
        variables: vec![],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: "".into(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    let m = crate::db::quick_prompts::list_quick_prompt_version_metrics(&conn, "qp-no-launches").unwrap();
    assert!(m.is_empty(), "no launches → no metrics rows");
}

#[test]
fn quick_prompt_delete_version_refuses_current_and_succeeds_on_older() {
    use crate::models::{AgentType, ModelTier};
    let conn = test_db();
    let now = Utc::now();
    let mut qp = crate::models::QuickPrompt {
        id: "qp-del".into(), name: "Del".into(), icon: "⚡".into(),
        prompt_template: "v1".into(),
        variables: vec![], agent: AgentType::ClaudeCode,
        project_id: None, skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        tier: ModelTier::Default, description: "".into(),
        agent_settings: None,
        created_at: now, updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    qp.prompt_template = "v2".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();
    qp.prompt_template = "v3".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();

    // current = v3; trying to delete it MUST fail.
    let err = crate::db::quick_prompts::delete_quick_prompt_version(&conn, "qp-del", 3).unwrap_err();
    assert!(err.to_string().contains("current"), "error mentions current: {}", err);

    // Deleting v2 (older) succeeds and returns true.
    let ok = crate::db::quick_prompts::delete_quick_prompt_version(&conn, "qp-del", 2).unwrap();
    assert!(ok);
    let versions = crate::db::quick_prompts::list_quick_prompt_versions(&conn, "qp-del").unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version_index, 3);
    assert_eq!(versions[1].version_index, 1);

    // Deleting a non-existent version returns false (idempotent).
    let none = crate::db::quick_prompts::delete_quick_prompt_version(&conn, "qp-del", 99).unwrap();
    assert!(!none);
}

#[test]
fn quick_prompt_delete_version_clears_discussion_lineage() {
    use crate::models::{Discussion, AgentType, ModelTier};
    let conn = test_db();
    let now = Utc::now();
    let mut qp = crate::models::QuickPrompt {
        id: "qp-cascade".into(), name: "C".into(), icon: "⚡".into(),
        prompt_template: "v1".into(),
        variables: vec![], agent: AgentType::ClaudeCode,
        project_id: None, skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        tier: ModelTier::Default, description: "".into(),
        agent_settings: None,
        created_at: now, updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    qp.prompt_template = "v2".into();
    qp.updated_at = Utc::now();
    crate::db::quick_prompts::update_quick_prompt(&conn, &qp).unwrap();

    // Seed a discussion stamped with v1 (the version we'll delete).
    let d = Discussion {
        id: "d-orphan".into(), project_id: None, title: "T".into(),
        agent: AgentType::ClaudeCode, language: "fr".into(),
        participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
        skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        archived: false, pinned: false,
        workspace_mode: "Direct".into(), workspace_path: None, worktree_branch: None,
        tier: ModelTier::Default, pin_first_message: false,
        model: None,
        summary_cache: None, summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: None, shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None, test_mode_stash_ref: None,
        created_at: Utc::now(), updated_at: Utc::now(),
    };
    crate::db::discussions::insert_discussion(&conn, &d).unwrap();
    crate::db::discussions::set_originating_qp(&conn, "d-orphan", "qp-cascade", 1).unwrap();

    // Delete v1 (older — v2 is the current). The discussion's lineage
    // must be cleared so its launch no longer counts under this QP.
    crate::db::quick_prompts::delete_quick_prompt_version(&conn, "qp-cascade", 1).unwrap();
    let (orig_qp, orig_version): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT originating_qp_id, originating_qp_version FROM discussions WHERE id = ?1",
            rusqlite::params!["d-orphan"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(orig_qp.is_none(), "originating_qp_id must be cleared after version deletion");
    assert!(orig_version.is_none(), "originating_qp_version must be cleared too");
}

#[test]
fn quick_prompt_metrics_ignores_non_first_agent_replies() {
    // If a discussion has 3 agent messages, only the FIRST counts toward
    // the QP's metrics — the QP's pertinence is reflected in the
    // initial reply, not subsequent back-and-forth.
    use crate::models::{Discussion, DiscussionMessage, MessageRole, AgentType, ModelTier};
    let conn = test_db();
    let now = Utc::now();
    let qp = crate::models::QuickPrompt {
        id: "qp-first-only".into(),
        name: "F".into(), icon: "⚡".into(), prompt_template: "v1".into(),
        variables: vec![], agent: AgentType::ClaudeCode,
        project_id: None, skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        tier: ModelTier::Default, description: "".into(),
        agent_settings: None,
        created_at: now, updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    let d = Discussion {
        id: "d-multi".into(), project_id: None, title: "T".into(),
        agent: AgentType::ClaudeCode, language: "fr".into(),
        participants: vec![AgentType::ClaudeCode], messages: vec![], message_count: 0, non_system_message_count: 0,
        skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
        archived: false, pinned: false,
        workspace_mode: "Direct".into(), workspace_path: None, worktree_branch: None,
        tier: ModelTier::Default, pin_first_message: false,
        model: None,
        summary_cache: None, summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: None, shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None, test_mode_stash_ref: None,
        created_at: Utc::now(), updated_at: Utc::now(),
    };
    crate::db::discussions::insert_discussion(&conn, &d).unwrap();
    // Three Agent replies — only the first should be counted.
    let mk = |id: &str, role: MessageRole, toks: u64, dur: u64| DiscussionMessage {
        model: None,
        lint_report: None,
        id: id.into(), role, content: "x".into(),
        agent_type: Some(AgentType::ClaudeCode), timestamp: Utc::now(),
        tokens_used: toks, auth_mode: None,
        model_tier: None, cost_usd: None,
        author_pseudo: None, author_avatar_email: None, source_msg_id: None,
        duration_ms: Some(dur),
    };
    crate::db::discussions::insert_message(&conn, "d-multi",
        &DiscussionMessage { agent_type: None, ..mk("u1", MessageRole::User, 0, 0) }).unwrap();
    crate::db::discussions::insert_message(&conn, "d-multi", &mk("a1", MessageRole::Agent, 1000, 5000)).unwrap();
    crate::db::discussions::insert_message(&conn, "d-multi",
        &DiscussionMessage { agent_type: None, ..mk("u2", MessageRole::User, 0, 0) }).unwrap();
    crate::db::discussions::insert_message(&conn, "d-multi", &mk("a2", MessageRole::Agent, 9999, 99999)).unwrap();
    crate::db::discussions::insert_message(&conn, "d-multi", &mk("a3", MessageRole::Agent, 8888, 88888)).unwrap();
    crate::db::discussions::set_originating_qp(&conn, "d-multi", "qp-first-only", 1).unwrap();

    let m = crate::db::quick_prompts::list_quick_prompt_version_metrics(&conn, "qp-first-only").unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].launches, 1);
    // The first agent message's values, not the later ones.
    assert_eq!(m[0].avg_tokens, 1000);
    assert_eq!(m[0].avg_duration_ms, Some(5000));
}

#[test]
fn quick_prompt_not_found() {
    let conn = test_db();
    let found = crate::db::quick_prompts::get_quick_prompt(&conn, "nonexistent").unwrap();
    assert!(found.is_none());
}

#[test]
fn quick_prompt_variables_roundtrip() {
    let conn = test_db();
    let now = Utc::now();
    let qp = QuickPrompt {
        id: "qp-vars".into(),
        name: "Test vars".into(),
        icon: "🔍".into(),
        prompt_template: "{{#jira}}Ticket {{jira}}, {{/jira}}{{#pr}}PR #{{pr}}{{/pr}}".into(),
        variables: vec![
            crate::models::PromptVariable {
                name: "jira".into(), label: "Ticket Jira".into(), placeholder: "PROJ-123".into(),
                description: None, required: false, pattern: None,
            },
            crate::models::PromptVariable {
                name: "pr".into(), label: "PR".into(), placeholder: "42".into(),
                description: None, required: false, pattern: None,
            },
        ],
        agent: crate::models::AgentType::ClaudeCode,
        project_id: None,
        skill_ids: vec!["security".into()],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::Reasoning,
        agent_settings: None,
        description: String::new(),
        created_at: now,
        updated_at: now,
    };
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();
    let loaded = crate::db::quick_prompts::get_quick_prompt(&conn, "qp-vars").unwrap().unwrap();

    // Variables preserved
    assert_eq!(loaded.variables.len(), 2);
    assert_eq!(loaded.variables[0].name, "jira");
    assert_eq!(loaded.variables[0].placeholder, "PROJ-123");
    assert_eq!(loaded.variables[1].name, "pr");

    // Skill IDs preserved
    assert_eq!(loaded.skill_ids, vec!["security"]);

    // Tier preserved
    assert_eq!(loaded.tier, crate::models::ModelTier::Reasoning);

    // Template with conditional sections preserved
    assert!(loaded.prompt_template.contains("{{#jira}}"));
    assert!(loaded.prompt_template.contains("{{/pr}}"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Cross-agent DB round-trip (auto-extends when new agents are added)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cross_agent_db_round_trip_all_types() {
    let conn = test_db();
    let all_agents = [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::Vibe,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
    ];
    for agent_type in &all_agents {
        let disc_id = format!("cross-{:?}", agent_type);
        let now = chrono::Utc::now();
        let disc = Discussion {
            id: disc_id.clone(),
            project_id: None,
            title: format!("Test {:?}", agent_type),
            agent: agent_type.clone(),
            language: "en".into(),
            participants: vec![agent_type.clone()],
            messages: vec![],
            message_count: 0, non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto, introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        };
        crate::db::discussions::insert_discussion(&conn, &disc).unwrap();
        let loaded = crate::db::discussions::get_discussion(&conn, &disc_id).unwrap().unwrap();
        assert_eq!(loaded.agent, *agent_type,
            "DB round-trip failed for {:?} — agent_type mutated after insert+read", agent_type);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 0.8.7 — P0-4 of the QA roadmap.
//
// `create_batch_run` wraps a multi-row write (placeholder workflow + run +
// N child discussions + their initial messages) in an explicit BEGIN /
// COMMIT / ROLLBACK transaction (`db/workflows.rs:478-498`). The previous
// test (`create_batch_run_pure_fn_roundtrip_toplevel`) only exercises the
// happy path ; this one pins the rollback path : when ONE step in the
// loop fails (here : a discussion FK violation because we point at a
// project that doesn't exist), the WHOLE transaction must unwind — no
// orphaned workflow row, no orphaned run, no orphaned discussion or
// message.
//
// Without this guarantee, a crash mid-batch would leave half-committed
// state that corrupts the runs list + the workflow registry forever.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn create_batch_run_rolls_back_on_discussion_fk_violation() {
    let conn = test_db();
    let qp = sample_qp_for_batch("qp-rollback-fk");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    // Sanity : capture row counts BEFORE the call so we can assert
    // "nothing changed" after rollback.
    let workflows_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get(0))
        .unwrap();
    let runs_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflow_runs", [], |r| r.get(0))
        .unwrap();
    let discs_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))
        .unwrap();
    let msgs_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();

    // Inject the failure : route the batch at a parent_run_id that does
    // NOT exist in workflow_runs. `workflow_runs.parent_run_id` carries an
    // FK (migration 030_workflow_run_parent.sql) — `insert_run` raises a
    // SQLITE_CONSTRAINT_FOREIGNKEY when the parent doesn't exist. The
    // failure fires AFTER `ensure_batch_placeholder_workflow` has
    // already inserted the placeholder workflow row, so the rollback
    // has real work to undo.
    let result = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![
                crate::db::workflows::BatchItemInput {
                    title: "EW-rollback-1".into(),
                    prompt: "x".into(),
                    agent_override: None,
                },
                crate::db::workflows::BatchItemInput {
                    title: "EW-rollback-2".into(),
                    prompt: "y".into(),
                    agent_override: None,
                },
                crate::db::workflows::BatchItemInput {
                    title: "EW-rollback-3".into(),
                    prompt: "z".into(),
                    agent_override: None,
                },
            ],
            batch_name: Some("Rollback test".into()),
            project_id: None,
            parent_run_id: Some("run-that-does-not-exist".into()),
            author_pseudo: Some("TestUser".into()),
            author_avatar_email: Some("test@example.com".into()),
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    );

    let err_msg = match result {
        Ok(_) => panic!("create_batch_run must return Err when a child discussion FK violates"),
        Err(e) => format!("{}", e).to_lowercase(),
    };
    assert!(
        err_msg.contains("foreign") || err_msg.contains("constraint"),
        "expected FK / constraint error, got: {err_msg}"
    );

    // The whole transaction must be unwound. Counts after the failed
    // call MUST match counts before — no orphaned rows anywhere.
    let workflows_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflows", [], |r| r.get(0))
        .unwrap();
    let runs_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflow_runs", [], |r| r.get(0))
        .unwrap();
    let discs_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))
        .unwrap();
    let msgs_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        workflows_after, workflows_before,
        "placeholder workflow leaked despite rollback (INSERT OR IGNORE survived)"
    );
    assert_eq!(
        runs_after, runs_before,
        "workflow_run row leaked despite rollback"
    );
    assert_eq!(
        discs_after, discs_before,
        "discussion row leaked despite rollback"
    );
    assert_eq!(
        msgs_after, msgs_before,
        "message row leaked despite rollback"
    );
}

#[test]
fn create_batch_run_subsequent_call_after_rollback_succeeds_cleanly() {
    // Companion to the rollback test : after the failed call rolls back,
    // a SECOND call with a clean (existing) project_id MUST succeed.
    // Regression guard against "rollback leaves the connection in a bad
    // transactional state" (a known SQLite footgun if BEGIN/ROLLBACK
    // pairing is mishandled).
    let conn = test_db();
    let project = sample_project("p-clean", "CleanProject");
    crate::db::projects::insert_project(&conn, &project).unwrap();
    let qp = sample_qp_for_batch("qp-after-rollback");
    crate::db::quick_prompts::insert_quick_prompt(&conn, &qp).unwrap();

    // 1st call — fails on bogus parent_run_id (FK on workflow_runs.id).
    let bad = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![crate::db::workflows::BatchItemInput {
                title: "EW-bad".into(),
                prompt: "x".into(),
                agent_override: None,
            }],
            batch_name: None,
            project_id: None,
            parent_run_id: Some("run-that-does-not-exist".into()),
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    );
    assert!(bad.is_err());

    // 2nd call — points at a real project, must succeed.
    let good = crate::db::workflows::create_batch_run(
        &conn,
        crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items: vec![crate::db::workflows::BatchItemInput {
                title: "EW-good".into(),
                prompt: "y".into(),
                agent_override: None,
            }],
            batch_name: None,
            project_id: Some("p-clean".into()),
            parent_run_id: None,
            author_pseudo: None,
            author_avatar_email: None,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        },
    );
    assert!(good.is_ok(), "subsequent call after rollback must succeed");
    let out = good.unwrap();
    assert_eq!(out.batch_total, 1);
    assert_eq!(out.discussion_ids.len(), 1);
}
