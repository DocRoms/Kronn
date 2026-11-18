//! KT-1751 fourth review — the snapshot's registry follows the project table:
//! rebuilt whole from it (both boots, an import, a reset), a re-registered path
//! never keeps a stale list, and a removed folder is still forgotten or moved.
//! Fifth review: a live project's list survives a second registration at its
//! path, and an unarmed snapshot gives nothing outside the managed layout.

use super::*;
use crate::models::{Project, ProjectWorkspace};

fn listing(entries: &[&str]) -> LaneWarmConfig {
    LaneWarmConfig {
        validation_executables: Some(entries.iter().map(|e| e.to_string()).collect()),
        ..LaneWarmConfig::default()
    }
}

pub(crate) fn project(id: &str, path: &Path, lane_warm: Option<LaneWarmConfig>) -> Project {
    let now = chrono::Utc::now();
    Project {
        id: id.into(),
        name: id.into(),
        path: path.display().to_string(),
        repo_url: None,
        token_override: None,
        ai_config: crate::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: Default::default(),
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        write_access: None,
        mcp_sync_report: None,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        workspace: lane_warm.map(|lane_warm| ProjectWorkspace {
            lane_warm,
            ..ProjectWorkspace::default()
        }),
        created_at: now,
        updated_at: now,
    }
}

fn current_global() -> LaneWarmConfig {
    SNAPSHOT
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .global
        .clone()
}

/// A `mono` project with a list, a `mono/sub` project (a plain sub-folder, no
/// repository of its own) with a lane entry but no executable, and a stale
/// entry left by a project that is no longer in the table.
struct Tree {
    _root: tempfile::TempDir,
    mono: PathBuf,
    sub: PathBuf,
    stale: PathBuf,
}

fn tree() -> Tree {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let mono = base.join("mono");
    let sub = mono.join("sub");
    let stale = base.join("stale");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    Tree {
        _root: root,
        mono,
        sub,
        stale,
    }
}

fn sub_entry() -> LaneWarmConfig {
    LaneWarmConfig {
        dependency_dirs: Some(vec!["vendor".into()]),
        ..LaneWarmConfig::default()
    }
}

#[test]
fn a_rebuild_replaces_the_whole_snapshot_from_the_project_list() {
    let _owner = test_guard::exclusive();
    let t = tree();
    set_project(&t.stale, Some(&listing(&["stale-tool"])));
    set_project(&t.mono, Some(&listing(&["old-outer"])));

    rebuild(
        &current_global(),
        &[
            project("mono", &t.mono, Some(listing(&["outer-tool"]))),
            project("sub", &t.sub, Some(sub_entry())),
        ],
    );
    let in_sub = validation_executables_for(&t.sub.join("src"));
    let sub_deps = effective_for(&t.sub, None).dependency_dirs;
    let outer = validation_executables_for(&t.mono);
    let stale = validation_executables_for(&t.stale);

    assert!(in_sub.is_empty(), "{in_sub:?}");
    assert_eq!(sub_deps, Some(vec!["vendor".to_string()]));
    assert_eq!(outer, vec!["outer-tool".to_string()]);
    assert!(stale.is_empty(), "a project gone from the table: {stale:?}");
}

#[tokio::test]
async fn arming_from_the_database_registers_every_project_and_loads_its_lane_entry() {
    let _owner = test_guard::exclusive_async().await;
    let t = tree();
    set_project(&t.stale, Some(&listing(&["stale-tool"])));
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let rows = vec![
        project("mono", &t.mono, Some(listing(&["outer-tool"]))),
        project("sub", &t.sub, None),
        project("deps", &t.stale.join("deps"), Some(sub_entry())),
    ];
    std::fs::create_dir_all(t.stale.join("deps")).unwrap();
    db.with_conn(move |conn| {
        for row in &rows {
            crate::db::projects::insert_project(conn, row)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .expect("insert projects");

    crate::arm_lane_settings(&db, &current_global())
        .await
        .expect("armed");
    let in_sub = validation_executables_for(&t.sub.join("src"));
    let outer = validation_executables_for(&t.mono);
    let deps = effective_for(&t.stale.join("deps"), None).dependency_dirs;
    let stale = validation_executables_for(&t.stale);

    assert!(in_sub.is_empty(), "the unlisted sub-project: {in_sub:?}");
    assert_eq!(outer, vec!["outer-tool".to_string()]);
    assert_eq!(deps, Some(vec!["vendor".to_string()]));
    assert!(stale.is_empty(), "a project not in the database: {stale:?}");
}

/// A leftover is a lane entry no registered project owns any more.
#[test]
fn registering_a_project_drops_a_leftover_lane_entry_for_its_path() {
    let _shared = test_guard::shared();
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    SNAPSHOT
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .projects
        .insert(dir.clone(), listing(&["left-over"]));
    register_project(&dir);
    let after = validation_executables_for(&dir);
    forget_project(&dir);

    assert!(after.is_empty(), "a new project inherited {after:?}");
}

/// KT-1751 fifth review — `create` has no duplicate-path check and `add_folder`
/// compares strings: a second project at a registered path (or the same path
/// with a trailing `/`) must not take the first one's list out of memory.
#[test]
fn registering_a_second_project_at_a_registered_path_keeps_the_first_ones_list() {
    let _shared = test_guard::shared();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().canonicalize().unwrap().join("app");
    std::fs::create_dir_all(&dir).unwrap();
    set_project(&dir, Some(&listing(&["first-tool"])));

    register_project(&dir);
    let same_path = validation_executables_for(&dir);
    register_project(&PathBuf::from(format!("{}/", dir.display())));
    let trailing_slash = validation_executables_for(&dir);
    forget_project(&dir);

    assert_eq!(same_path, vec!["first-tool".to_string()]);
    assert_eq!(trailing_slash, vec!["first-tool".to_string()]);
}

/// KT-1751 fifth review — the snapshot could not be armed (the project listing
/// failed at boot or after an import): a workspace save of the outer project
/// registers it alone, and that partial registry must not hand its list to an
/// unregistered nested project. Outside the managed layout nothing runs.
#[test]
fn an_unarmed_snapshot_gives_nothing_after_a_workspace_save_of_an_outer_project() {
    let _owner = test_guard::exclusive();
    let t = tree();
    disarm(&current_global());

    // What `set_project_workspace` publishes for `mono`.
    set_project(&t.mono, Some(&listing(&["outer-tool"])));
    let in_sub = validation_executables_for(&t.sub.join("src"));
    let outer = validation_executables_for(&t.mono);
    rebuild(&current_global(), &[]);

    assert!(
        in_sub.is_empty(),
        "the unregistered sub-project got {in_sub:?}"
    );
    assert!(outer.is_empty(), "an unarmed lookup gave {outer:?}");
}

/// The stored key went through a link (`/var` and `/private/var` on macOS), and
/// the folder is gone by the time the project is forgotten or moved: the entry
/// is still found.
#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_removed_folder_reached_through_a_link_is_still_forgotten_and_moved() {
    let _shared = test_guard::shared();
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let real = base.join("real");
    let link = base.join("link");
    std::fs::create_dir_all(real.join("gone")).unwrap();
    std::fs::create_dir_all(real.join("old")).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    set_project(&link.join("gone"), Some(&listing(&["gone-tool"])));
    std::fs::remove_dir(real.join("gone")).unwrap();
    forget_project(&link.join("gone"));
    let (gone_listed, gone_registered) = {
        let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner());
        (
            snapshot.projects.contains_key(&real.join("gone")),
            snapshot.registered.contains(&real.join("gone")),
        )
    };

    set_project(&link.join("old"), Some(&listing(&["moved-tool"])));
    std::fs::remove_dir(real.join("old")).unwrap();
    std::fs::create_dir_all(real.join("new")).unwrap();
    rekey_project(&link.join("old"), &link.join("new"));
    let moved = validation_executables_for(&real.join("new"));
    let old_left = {
        let snapshot = SNAPSHOT.read().unwrap_or_else(|e| e.into_inner());
        snapshot.projects.contains_key(&real.join("old"))
            || snapshot.registered.contains(&real.join("old"))
    };
    forget_project(&real.join("new"));

    assert!(!gone_listed, "the removed folder's list stayed");
    assert!(!gone_registered, "the removed folder stayed registered");
    assert_eq!(moved, vec!["moved-tool".to_string()]);
    assert!(!old_left, "the old path stayed");
}
