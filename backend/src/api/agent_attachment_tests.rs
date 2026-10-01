//! KT-946 — an HTTP agent can read the files attached to ITS discussion,
//! read-only, wherever Kronn stored them; and nothing else outside the workspace.
//!
//! The reported case: an attachment saved in Kronn's data directory (the
//! discussion had no project), `read_file` refused it twice — "not attached to any
//! directory", then "outside this discussion's workspace" — and the file was
//! unreachable for the one agent it was attached for. KT-338's bounds are the
//! other half of the contract and must stay exactly where they were.

use super::*;
use crate::api::agent_workspace_tools as ws;

fn attachments(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(name, path)| ((*name).to_string(), (*path).to_string()))
        .collect()
}

#[test]
fn an_attachment_is_found_by_the_path_it_was_announced_under() {
    let rows = attachments(&[
        (
            "review.md",
            "/data/Kronn/.kronn/context-files/76c52a08_review.md",
        ),
        (
            "image.png",
            "/data/Kronn/.kronn/context-files/817c89b9_image.png",
        ),
    ]);
    let review = "/data/Kronn/.kronn/context-files/76c52a08_review.md";

    // Absolute path, bare file name, in-project relative form: the spellings a
    // model actually produces from the announcement.
    for requested in [
        review,
        "76c52a08_review.md",
        ".kronn/context-files/76c52a08_review.md",
        "  76c52a08_review.md  ",
    ] {
        assert_eq!(
            ws::match_attachment(&rows, requested),
            Some(review),
            "{requested}"
        );
    }
    // The friendly spellings do not turn into a search: a different name, a
    // different directory, a traversal and a prefix all match nothing.
    for requested in [
        "",
        "review.md",
        "76c52a08",
        "/etc/passwd",
        "/data/Kronn/.kronn/context-files/other.md",
        "../context-files/76c52a08_review.md",
        ".kronn/context-files/../76c52a08_review.md",
        "src/main.rs",
    ] {
        assert_eq!(
            ws::match_attachment(&rows, requested),
            None,
            "{requested:?}"
        );
    }
}

#[test]
fn an_ordinary_workspace_path_never_costs_a_lookup() {
    for plain in ["src/main.rs", "docs/AGENTS.md", "a/b/c.txt", "..\\x\\y"] {
        assert!(!ws::could_reference_attachment(plain), "{plain}");
    }
    for candidate in [
        "/Users/x/.kronn/context-files/a_b.png",
        "817c89b9_image.png",
        ".kronn/context-files/817c89b9_image.png",
    ] {
        assert!(ws::could_reference_attachment(candidate), "{candidate}");
    }
}

/// A `context-files` directory with a text note, an image and — outside it — a
/// file that is NOT an attachment, the way a discussion's storage really looks.
struct Storage {
    _dir: tempfile::TempDir,
    note: String,
    image: String,
    foreign_note: String,
    secret: String,
    workspace: std::path::PathBuf,
}

fn storage() -> Storage {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let files = root.join("data/context-files");
    std::fs::create_dir_all(&files).unwrap();
    let write = |path: std::path::PathBuf, bytes: &[u8]| {
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().to_string()
    };
    let note = write(
        files.join("76c52a08_note.md"),
        b"line one\nline two\nline three\n",
    );
    let image = write(files.join("817c89b9_image.png"), b"\x89PNG\r\n\x1a\n....");
    let foreign_note = write(
        files.join("11111111_other.md"),
        b"belongs to another room\n",
    );
    let secret = write(root.join("data/secret.txt"), b"not an attachment\n");
    let workspace = root.join("project");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("README.md"), b"# project\n").unwrap();
    Storage {
        _dir: dir,
        note,
        image,
        foreign_note,
        secret,
        workspace,
    }
}

/// Rooms: `general` has no project (the reported case), `ws-room` has a real
/// workspace, `other-room` owns an attachment the others must not reach.
async fn state_with_attachments(storage: &Storage) -> AppState {
    let state = super::quick_prompt_tests::state_with_prompts().await;
    let (note, image, foreign, workspace) = (
        storage.note.clone(),
        storage.image.clone(),
        storage.foreign_note.clone(),
        storage.workspace.to_string_lossy().to_string(),
    );
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id,name,path,created_at,updated_at) \
                 VALUES ('p-ws','ws',?1,'2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')",
                [workspace],
            )?;
            for (id, project) in [("ws-room", Some("p-ws")), ("other-room", None)] {
                conn.execute(
                    "INSERT INTO discussions (id,title,project_id,created_at,updated_at) \
                     VALUES (?1,?1,?2,'2026-09-22T00:00:00Z','2026-09-22T00:00:00Z')",
                    rusqlite::params![id, project],
                )?;
            }
            for (id, room, name, mime, path) in [
                ("f1", "general", "note.md", "text/plain", &note),
                ("f2", "general", "image.png", "image/png", &image),
                ("f3", "ws-room", "note.md", "text/plain", &note),
                ("f4", "other-room", "other.md", "text/plain", &foreign),
            ] {
                crate::db::discussions::insert_context_file(
                    conn,
                    id,
                    room,
                    name,
                    mime,
                    10,
                    "preview",
                    Some(path.as_str()),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    state
}

async fn read(state: &AppState, room: &str, arguments: Value) -> ToolOutcome {
    KronnToolExecutor::new(state.clone(), Some(room.into()))
        .execute(&ToolCall {
            id: "read".into(),
            name: "read_file".into(),
            arguments,
        })
        .await
}

#[tokio::test]
async fn a_room_with_no_workspace_reads_its_own_attachment() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;

    // The reported case: no project, the file in the data directory. Both the
    // announced absolute path and the bare name reach it.
    for requested in [storage.note.as_str(), "76c52a08_note.md"] {
        let result = read(&state, "general", json!({ "path": requested })).await;
        assert!(result.ok, "{requested}: {}", result.content);
        assert_eq!(result.content["text"], "line one\nline two\nline three\n");
        assert_eq!(result.content["attachment"], true);
        assert_eq!(result.content["read_only"], true);
        assert!(
            result.content.get("content_sha256").is_none(),
            "an attachment is never edited, so no revision receipt is offered"
        );
    }
    // Slicing still works on it.
    let slice = read(
        &state,
        "general",
        json!({ "path": storage.note, "offset": 2, "limit": 1 }),
    )
    .await;
    assert!(slice.ok);
    assert_eq!(slice.content["text"], "line two\n");
}

#[tokio::test]
async fn a_room_with_a_workspace_reads_its_attachment_and_its_workspace() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;

    let attachment = read(&state, "ws-room", json!({ "path": storage.note })).await;
    assert!(attachment.ok, "{}", attachment.content);
    assert_eq!(attachment.content["attachment"], true);

    // KT-338's own door is unchanged.
    let workspace_file = read(&state, "ws-room", json!({ "path": "README.md" })).await;
    assert!(workspace_file.ok, "{}", workspace_file.content);
    assert_eq!(workspace_file.content["text"], "# project\n");
    assert!(workspace_file.content.get("attachment").is_none());
}

#[tokio::test]
async fn nothing_else_outside_the_workspace_becomes_readable() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;

    // Next to the attachment, but not one of this room's: refused with KT-338's
    // own wording, not a new one.
    for requested in [storage.secret.as_str(), "/etc/hosts", "../secret.txt"] {
        let result = read(&state, "ws-room", json!({ "path": requested })).await;
        assert!(!result.ok, "{requested} must stay refused");
        assert!(
            result
                .content
                .to_string()
                .contains("outside this discussion's workspace"),
            "{requested}: {}",
            result.content
        );
    }
    // A room with no workspace and no such attachment is still told so.
    let result = read(&state, "general", json!({ "path": storage.secret })).await;
    assert!(!result.ok);
    assert!(
        result
            .content
            .to_string()
            .contains("not attached to any directory"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn another_rooms_attachment_is_not_reachable_by_its_path() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;

    // `foreign_note` is another discussion's attachment, in the very same
    // directory. Guessing or copying its path must not read it.
    for room in ["general", "ws-room"] {
        for requested in [storage.foreign_note.as_str(), "11111111_other.md"] {
            let result = read(&state, room, json!({ "path": requested })).await;
            // Refused outright, or — for a bare name that falls through to the
            // workspace, where KT-338 reports a missing file as a successful
            // "not found" — nothing was read.
            assert!(
                !result.ok || result.content["found"] == false,
                "{room} read {requested}: {}",
                result.content
            );
            assert!(
                !result
                    .content
                    .to_string()
                    .contains("belongs to another room"),
                "{room} / {requested}: the content must never be returned"
            );
        }
    }
    // Its owner reads it without trouble.
    let owner = read(
        &state,
        "other-room",
        json!({ "path": storage.foreign_note }),
    )
    .await;
    assert!(owner.ok, "{}", owner.content);
}

#[tokio::test]
async fn an_attachment_stays_read_only() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;
    let executor = KronnToolExecutor::new(state, Some("ws-room".into()));
    let before = std::fs::read(&storage.note).unwrap();

    for (tool, arguments) in [
        (
            "write_file",
            json!({ "path": storage.note, "content": "overwritten" }),
        ),
        (
            "edit_file",
            json!({ "path": storage.note, "old_string": "line one", "new_string": "x",
                     "expected_sha256": "0".repeat(32) }),
        ),
        ("list_files", json!({ "path": storage.note })),
        (
            "search_text",
            json!({ "pattern": "line", "path": storage.note }),
        ),
    ] {
        let result = executor
            .execute(&ToolCall {
                id: tool.into(),
                name: tool.into(),
                arguments,
            })
            .await;
        assert!(
            !result.ok,
            "{tool} must not reach an attachment: {}",
            result.content
        );
    }
    assert_eq!(
        std::fs::read(&storage.note).unwrap(),
        before,
        "the file is intact"
    );
}

#[tokio::test]
async fn an_attached_image_is_refused_with_an_explanation_not_returned_as_garbage() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;

    let result = read(&state, "general", json!({ "path": storage.image })).await;
    assert!(!result.ok);
    let message = result.content.to_string();
    assert!(message.contains("cannot be read as text"), "{message}");
    assert!(message.contains("do not describe it"), "{message}");
    assert!(!message.contains("PNG"), "no lossy bytes: {message}");
}

#[tokio::test]
async fn attachments_inside_the_project_are_read_only_even_with_a_valid_receipt() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;
    let directory = storage.workspace.join(".kronn/context-files");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("12345678_note.md");
    std::fs::write(&path, "original\n").unwrap();
    let relative = ".kronn/context-files/12345678_note.md";
    let payload = crate::api::agent_workspace_tools::read_file_payload(
        &storage.workspace,
        relative,
        None,
        None,
    )
    .unwrap();
    let receipt = payload["content_sha256"].as_str().unwrap();
    let executor = KronnToolExecutor::new(state, Some("ws-room".into()));
    for name in ["write_file", "edit_file", "edit_lines", "insert_after_line"] {
        for requested in [relative.to_string(), path.to_string_lossy().to_string()] {
            let outcome = executor.execute(&ToolCall {
                id: name.into(), name: name.into(),
                arguments: json!({"path": requested, "content": "changed", "old_string": "original", "new_string": "changed", "expected_sha256": receipt, "start_line": 1, "end_line": 1, "anchor_line": 1})
            }).await;
            assert!(!outcome.ok, "{name}");
            let message = outcome.content.to_string();
            if std::path::Path::new(&requested).is_absolute() {
                assert!(
                    message.contains("outside this discussion's workspace"),
                    "{message}"
                );
            } else {
                assert!(message.contains("read-only"), "{message}");
            }
        }
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");
}

#[tokio::test]
async fn a_forged_row_cannot_turn_the_door_into_an_arbitrary_file_read() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;
    // A row of THIS discussion pointing outside any `context-files` directory.
    let secret = storage.secret.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_context_file(
                conn,
                "forged",
                "general",
                "passwd",
                "text/plain",
                1,
                "",
                Some(&secret),
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let result = read(&state, "general", json!({ "path": storage.secret })).await;
    assert!(!result.ok, "{}", result.content);
    assert!(
        !result.content.to_string().contains("not an attachment"),
        "the forged row's content must not be returned"
    );

    // A symlink inside `context-files` pointing out of it is no better.
    #[cfg(unix)]
    {
        let link = std::path::Path::new(&storage.note)
            .parent()
            .unwrap()
            .join("22222222_link.md");
        std::os::unix::fs::symlink(&storage.secret, &link).unwrap();
        let link_path = link.to_string_lossy().to_string();
        let row_path = link_path.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::insert_context_file(
                    conn,
                    "linked",
                    "general",
                    "link.md",
                    "text/plain",
                    1,
                    "",
                    Some(&row_path),
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let result = read(&state, "general", json!({ "path": link_path })).await;
        assert!(!result.ok, "{}", result.content);
    }
}

#[tokio::test]
async fn a_run_without_a_discussion_has_no_attachments_to_read() {
    let storage = storage();
    let state = state_with_attachments(&storage).await;
    let result = KronnToolExecutor::new(state, None)
        .execute(&ToolCall {
            id: "read".into(),
            name: "read_file".into(),
            arguments: json!({ "path": storage.note }),
        })
        .await;
    assert!(!result.ok, "{}", result.content);
}
