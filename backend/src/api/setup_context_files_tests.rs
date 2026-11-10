//! KT-1602 — the whole-DB export carries the uploaded context files: rows in
//! data.json, bytes as `context-files/<id>` ZIP entries, counted against the
//! import limit.

use super::*;
use crate::models::db::ContextFileRecord;

const FILE_ID: &str = "11111111-aaaa-bbbb-cccc-000000000001";
const TEXT_ID: &str = "22222222-aaaa-bbbb-cccc-000000000002";
const FRAME_ID: &str = "33333333-aaaa-bbbb-cccc-000000000003";

fn discussion(id: &str) -> crate::models::Discussion {
    crate::models::Discussion {
        connection_id: None,
        id: id.into(),
        project_id: None,
        title: "With files".into(),
        agent: crate::models::AgentType::Codex,
        language: "en".into(),
        participants: vec![crate::models::AgentType::Codex],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: crate::models::ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Off,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        awaiting_agent: false,
        agent_running: false,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn record(id: &str, has_blob: bool) -> ContextFileRecord {
    ContextFileRecord {
        id: id.into(),
        discussion_id: "disc-1".into(),
        filename: "photo.png".into(),
        mime_type: "image/png".into(),
        original_size: 4,
        extracted_text: String::new(),
        extracted_size: 0,
        message_id: None,
        extracted_from_asset_id: None,
        created_at: "2026-05-01 10:00:00".into(),
        has_blob,
        disk_path: None,
    }
}

async fn fresh_state() -> crate::AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let cfg = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    crate::AppState::new_defaults(cfg, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn empty_archive_data() -> DbExport {
    serde_json::from_value(serde_json::json!({
        "version": 8, "exported_at": Utc::now().to_rfc3339(),
        "projects": [], "discussions": [],
    }))
    .unwrap()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Seed `disc-1` with an image whose bytes sit on disk, a text file kept only
/// as extracted text, and a frame taken out of the image.
async fn seed_source(state: &crate::AppState, image_path: &str) {
    let image_path = image_path.to_string();
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &discussion("disc-1"))?;
            crate::db::discussions::insert_context_file(
                conn,
                FILE_ID,
                "disc-1",
                "photo.png",
                "image/png",
                9,
                "",
                Some(&image_path),
            )?;
            crate::db::discussions::insert_context_file(
                conn,
                TEXT_ID,
                "disc-1",
                "notes.txt",
                "text/plain",
                5,
                "hello",
                None,
            )?;
            crate::db::discussions::insert_context_file(
                conn,
                FRAME_ID,
                "disc-1",
                "frame.png",
                "image/png",
                0,
                "",
                None,
            )?;
            conn.execute(
                "UPDATE context_files SET message_id = '__legacy_disc_wide__' WHERE id = ?1",
                [TEXT_ID],
            )?;
            conn.execute(
                "UPDATE context_files SET extracted_from_asset_id = ?2 WHERE id = ?1",
                [FRAME_ID, FILE_ID],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

/// An export that went through the real ZIP comes back with every context file
/// row, the bytes of the one that had a file, and none of the source machine's
/// paths.
#[tokio::test]
#[serial_test::serial]
async fn context_files_round_trip_through_the_zip_with_their_bytes() {
    let _data = crate::core::config::TestDataDir::new();
    let source_dir = tempfile::tempdir().unwrap();
    let image_path = source_dir.path().join("source-photo.png");
    std::fs::write(&image_path, b"PNG-BYTES").unwrap();
    let source = fresh_state().await;
    seed_source(&source, image_path.to_str().unwrap()).await;

    let export = build_export(&source).await.expect("export");
    assert_eq!(export.context_files.len(), 3, "every row is exported");
    assert!(export
        .context_files
        .iter()
        .any(|r| r.id == FILE_ID && r.has_blob));
    assert!(
        export
            .context_files
            .iter()
            .filter(|r| r.id != FILE_ID)
            .all(|r| !r.has_blob),
        "a row with no file on disk carries no blob"
    );
    let json = serde_json::to_string(&export).unwrap();
    assert!(
        !json.contains(image_path.to_str().unwrap()),
        "the source machine's path never reaches data.json"
    );

    let blobs = vec![(
        FILE_ID.to_string(),
        image_path.to_str().unwrap().to_string(),
    )];
    let bytes = build_export_zip(&json, "", None, &blobs).expect("zip");
    let names: Vec<String> = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..]))
        .unwrap()
        .file_names()
        .filter_map(|name| name.ok())
        .map(|name| name.into_owned())
        .collect();
    assert!(
        names.contains(&format!("{CONTEXT_BLOB_PREFIX}{FILE_ID}")),
        "{names:?}"
    );

    // Restore on a fresh instance, from the archive alone.
    std::fs::remove_file(&image_path).unwrap();
    let (data, _, _, context_blobs) = extract_zip(&bytes).expect("extract");
    let target = fresh_state().await;
    let report = do_import_db(&target, &data, context_blobs)
        .await
        .expect("import");
    assert!(
        !report.warnings.iter().any(|w| w.contains("context file")),
        "{:?}",
        report.warnings
    );

    let rows = target
        .db
        .with_conn(crate::db::discussions::list_context_file_records)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    let image = rows.iter().find(|r| r.id == FILE_ID).unwrap();
    let restored_path = image.disk_path.as_deref().expect("bytes restored to disk");
    assert_ne!(Some(restored_path), image_path.to_str(), "a local path");
    assert!(
        std::path::Path::new(restored_path).starts_with(crate::core::config::config_dir().unwrap()),
        "{restored_path} sits in this instance's own store"
    );
    assert_eq!(std::fs::read(restored_path).unwrap(), b"PNG-BYTES");
    assert_eq!(
        (image.filename.as_str(), image.mime_type.as_str()),
        ("photo.png", "image/png")
    );
    let exported = export
        .context_files
        .iter()
        .find(|r| r.id == FILE_ID)
        .unwrap();
    assert_eq!(
        image.created_at, exported.created_at,
        "created_at kept verbatim"
    );

    let text = rows.iter().find(|r| r.id == TEXT_ID).unwrap();
    assert_eq!(text.extracted_text, "hello");
    assert_eq!(text.message_id.as_deref(), Some("__legacy_disc_wide__"));
    assert!(text.disk_path.is_none());
    let frame = rows.iter().find(|r| r.id == FRAME_ID).unwrap();
    assert_eq!(
        frame.extracted_from_asset_id.as_deref(),
        Some(FILE_ID),
        "the frame still points at its clip"
    );
}

/// A row whose bytes the archive lacks still comes back, and the report says so.
#[tokio::test]
#[serial_test::serial]
async fn a_context_file_without_its_bytes_is_restored_as_a_row_and_reported() {
    let _data = crate::core::config::TestDataDir::new();
    let mut data = empty_archive_data();
    data.discussions = vec![discussion("disc-1")];
    data.context_files = vec![record(FILE_ID, true)];
    let target = fresh_state().await;

    let report = do_import_db(&target, &data, ContextBlobs::default())
        .await
        .expect("import");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("without their bytes")),
        "{:?}",
        report.warnings
    );
    let rows = target
        .db
        .with_conn(crate::db::discussions::list_context_file_records)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].disk_path.is_none());
}

/// An archive without the section (any v7 backup) leaves the local files alone
/// when it does not replace their discussions.
#[tokio::test]
async fn an_archive_without_context_files_keeps_the_local_ones() {
    let target = fresh_state().await;
    target
        .db
        .with_conn(|conn| {
            crate::db::discussions::insert_discussion(conn, &discussion("disc-1"))?;
            crate::db::discussions::insert_context_file(
                conn,
                FILE_ID,
                "disc-1",
                "n.txt",
                "text/plain",
                1,
                "x",
                None,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let older: DbExport = serde_json::from_value(serde_json::json!({
        "version": 7, "exported_at": Utc::now().to_rfc3339(),
        "projects": [], "discussions": [],
    }))
    .unwrap();
    assert!(
        older.context_files.is_empty(),
        "the field defaults to empty"
    );

    do_import_db(&target, &older, ContextBlobs::default())
        .await
        .expect("import");
    let rows = target
        .db
        .with_conn(crate::db::discussions::list_context_file_records)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "nothing wiped");
}

/// The blob bytes are counted against the import budget, per file and all
/// together; an entry no row names is neither read nor counted.
#[test]
fn context_blobs_are_counted_against_the_import_budget() {
    let mut data = empty_archive_data();
    data.context_files = vec![record(FILE_ID, true), record(TEXT_ID, true)];
    let read = |entries: &[(&str, &[u8])], per_file: u64, total: u64| {
        let bytes = zip_of(entries);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
        read_context_blobs(&mut archive, &data, per_file, total)
    };
    let first = format!("{CONTEXT_BLOB_PREFIX}{FILE_ID}");
    let second = format!("{CONTEXT_BLOB_PREFIX}{TEXT_ID}");
    let stray = format!("{CONTEXT_BLOB_PREFIX}99999999-not-a-row");

    let blobs = read(&[(&first, &[1; 10]), (&second, &[2; 10])], 10, 20).expect("at the limit");
    assert_eq!(blobs.0.len(), 2);

    let err = read(&[(&first, &[1; 11])], 10, 100).expect_err("per-file cap");
    assert!(err.contains("zip bomb"), "{err}");
    let err = read(&[(&first, &[1; 10]), (&second, &[2; 11])], 10, 20).expect_err("total budget");
    assert!(err.contains("zip bomb"), "{err}");

    let blobs = read(&[(&stray, &[9; 1000]), (&first, &[1; 5])], 10, 10).expect("stray ignored");
    assert_eq!(blobs.0.len(), 1);
    assert!(blobs.0.contains_key(FILE_ID));
}

/// The exporter stops at the same budget the importer enforces, so an archive
/// this instance wrote is never refused for its blobs; the rows stay.
#[test]
fn the_exporter_stops_at_the_budget_the_importer_enforces() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, len: usize| {
        let path = dir.path().join(name);
        std::fs::write(&path, vec![7u8; len]).unwrap();
        path.to_string_lossy().into_owned()
    };
    let mut rows = vec![
        record(FILE_ID, false),
        record(TEXT_ID, false),
        record(FRAME_ID, false),
        record("44444444-aaaa-bbbb-cccc-000000000004", false),
    ];
    rows[0].disk_path = Some(write("a", 6));
    rows[1].disk_path = Some(write("b", 6)); // over the total (10) once `a` is in
    rows[2].disk_path = Some(write("c", 20)); // over the per-file cap (8)
    rows[3].disk_path = Some(dir.path().join("gone").to_string_lossy().into_owned());

    let omitted = mark_exportable_blobs(&mut rows, 8, 10);
    assert_eq!(omitted, 3);
    assert_eq!(
        rows.iter().map(|r| r.has_blob).collect::<Vec<_>>(),
        vec![true, false, false, false]
    );
}

/// An id that would not be a safe file name never becomes a blob entry.
#[test]
fn only_safe_ids_name_blob_entries() {
    assert!(is_blob_id(FILE_ID));
    for bad in [
        "short",
        "../../etc/passwd-aaaa",
        "a b c d e f g h",
        "ééééééééé",
        "",
    ] {
        assert!(!is_blob_id(bad), "{bad:?}");
    }
}
