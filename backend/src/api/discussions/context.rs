// Context Files: per-discussion uploads (multipart) that feed extra
// background context into agent prompts. Files are extracted to text
// at upload time and stored in the DB; binaries land on disk under
// the discussion's worktree. Suggested skills are auto-derived from
// the file extension to nudge the user toward the right experts.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use crate::models::*;
use crate::AppState;

/// POST /api/discussions/:id/context-files — upload a file (multipart/form-data)
pub async fn upload_context_file(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Json<ApiResponse<crate::models::UploadContextFileResponse>> {
    // The file, plus the optional id of the asset it was taken OUT of. A frame
    // decoded from a clip is not a composer upload: it gets its own transcript
    // message, so it never waits to be pinned to the next thing the user sends
    // — and deleting an unrelated message can no longer take it away.
    let mut upload: Option<(String, axum::body::Bytes)> = None;
    let mut extracted_from: Option<String> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or_default().to_string();
                let file_name = field.file_name().map(str::to_string);
                if name == "extracted_from_asset_id" && file_name.is_none() {
                    match field.text().await {
                        Ok(value) => {
                            let value = value.trim().to_string();
                            if !value.is_empty() {
                                extracted_from = Some(value);
                            }
                        }
                        Err(e) => {
                            return Json(ApiResponse::err(format!("Failed to read upload: {e}")))
                        }
                    }
                    continue;
                }
                match field.bytes().await {
                    Ok(bytes) => {
                        if upload.is_none() {
                            upload = Some((file_name.unwrap_or_else(|| "unknown".into()), bytes));
                        }
                    }
                    Err(e) => return Json(ApiResponse::err(format!("Failed to read upload: {e}"))),
                }
            }
            Ok(None) => break,
            Err(e) => {
                return Json(
                    ApiResponse::<crate::models::UploadContextFileResponse>::err(format!(
                        "Multipart error: {e}"
                    )),
                )
            }
        }
    }
    let Some((filename, data)) = upload else {
        return Json(
            ApiResponse::<crate::models::UploadContextFileResponse>::err(
                "No file provided".to_string(),
            ),
        );
    };

    // Bound the current staging area, not the discussion's attachment history.
    // Once a file is pinned to a durable message it must remain viewable without
    // permanently consuming one of the composer's upload slots.
    let did = discussion_id.clone();
    let count = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::count_pending_context_files(conn, &did)
                .map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .unwrap_or(0);

    if count >= crate::core::context_files::MAX_PENDING_FILES_PER_DISCUSSION {
        return Json(ApiResponse::err(format!(
            "Maximum {} pending context files per discussion reached",
            crate::core::context_files::MAX_PENDING_FILES_PER_DISCUSSION
        )));
    }

    let owner = match extracted_from {
        Some(source_id) => ContextFileOwner::ExtractedFrom(source_id),
        None => ContextFileOwner::Pending,
    };
    match store_context_file(&state, discussion_id, filename, &data, owner).await {
        Ok((file, suggested_skills)) => {
            Json(ApiResponse::ok(crate::models::UploadContextFileResponse {
                file,
                suggested_skills,
            }))
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// Who an attachment belongs to when it is stored.
pub(crate) enum ContextFileOwner {
    /// A composer upload, pinned to the user's next message.
    Pending,
    /// A frame taken out of the clip with this asset id.
    ExtractedFrom(String),
    /// A file an agent linked in this message.
    Message(String),
}

/// Store one attachment: extract its content, write the file, record it.
/// Shared by the upload route and by the files agents link in their messages.
pub(crate) async fn store_context_file(
    state: &AppState,
    discussion_id: String,
    filename: String,
    data: &[u8],
    owner: ContextFileOwner,
) -> Result<(crate::models::ContextFile, Vec<String>), String> {
    // Extract content (text or image)
    let content = match crate::core::context_files::extract_content(&filename, data) {
        Ok(c) => c,
        Err(e) => return Err(e.to_string()),
    };

    // Resolve the work directory for this discussion. With a project, images
    // land in its worktree (agents read them with file tools). WITHOUT a
    // project we must NOT use the system temp dir: under Docker that's the
    // container's /tmp, wiped on every restart/rebuild, so the attachment
    // bytes vanish and the bubble thumbnail 404s. Fall back to the persistent
    // data dir (KRONN_DATA_DIR volume) instead, only dropping to temp if even
    // that can't be resolved.
    let persistent_fallback =
        || crate::core::config::config_dir().unwrap_or_else(|_| std::env::temp_dir());
    let did_for_path = discussion_id.clone();
    let fallback = persistent_fallback();
    let work_dir: std::path::PathBuf = state
        .db
        .with_conn(move |conn| {
            let project_id: Option<String> = conn
                .query_row(
                    "SELECT project_id FROM discussions WHERE id = ?1",
                    rusqlite::params![did_for_path],
                    |row| row.get(0),
                )
                .unwrap_or(None);
            let path = if let Some(pid) = project_id {
                conn.query_row(
                    "SELECT path FROM projects WHERE id = ?1",
                    rusqlite::params![pid],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            } else {
                None
            };
            Ok(match path {
                Some(p) => std::path::PathBuf::from(p),
                None => fallback,
            })
        })
        .await
        .unwrap_or_else(|_: anyhow::Error| persistent_fallback());

    let id = uuid::Uuid::new_v4().to_string();
    let mime = crate::core::context_files::mime_from_extension(&filename).to_string();
    let original_size = data.len() as u64;
    let suggested_skills = crate::core::context_files::suggest_skills(&filename);

    // Handle text vs image vs on-disk file
    let (extracted_text, disk_path) = match content {
        crate::core::context_files::ExtractedContent::Text(text) => {
            let path = if matches!(&owner, ContextFileOwner::Message(_)) {
                Some(
                    crate::core::context_files::save_file_to_dir(&work_dir, &id, &filename, data)
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            (text, path)
        }
        crate::core::context_files::ExtractedContent::DiskFile {
            data: file_data,
            preview,
        } => {
            // Raw file saved to disk (worktree); only the preview lands in
            // context. The agent reads the full file by path. Falls back to the
            // persistent config dir if the project worktree write fails.
            match crate::core::context_files::save_file_to_dir(
                &work_dir, &id, &filename, &file_data,
            ) {
                Ok(path) => (preview, Some(path)),
                Err(e) => {
                    match crate::core::context_files::save_file_to_disk(&id, &filename, &file_data)
                    {
                        Ok(path) => (preview, Some(path)),
                        Err(e2) => {
                            return Err(format!("Failed to save file: {e} / fallback: {e2}"))
                        }
                    }
                }
            }
        }
        crate::core::context_files::ExtractedContent::Image {
            data: img_data,
            ext,
        } => {
            match crate::core::context_files::save_image_to_dir(
                &work_dir, &id, &filename, &ext, &img_data,
            ) {
                Ok(path) => {
                    let label = format!("[Image: {}]", filename);
                    (label, Some(path))
                }
                Err(e) => {
                    // Fallback to config dir if project dir fails
                    match crate::core::context_files::save_image_to_disk(&id, &ext, &img_data) {
                        Ok(path) => {
                            let label = format!("[Image: {}]", filename);
                            (label, Some(path))
                        }
                        Err(e2) => {
                            return Err(format!("Failed to save image: {e} / fallback: {e2}"))
                        }
                    }
                }
            }
        }
    };

    let extracted_size = extracted_text.len() as u64;
    let file_id = id.clone();
    let did = discussion_id.clone();
    let fname = filename.clone();
    let mime_clone = mime.clone();
    let text = extracted_text.clone();
    let dp = disk_path.clone();

    let source_asset = match &owner {
        ContextFileOwner::ExtractedFrom(source_id) => Some(source_id.clone()),
        _ => None,
    };
    let owning_message = match &owner {
        ContextFileOwner::Message(message_id) => Some(message_id.clone()),
        _ => None,
    };
    let owning_message_for_db = owning_message.clone();
    let insert_result = state
        .db
        .with_conn(move |conn| {
            // A frame lands in ONE transaction with the message that explains
            // it: a file pinned to a message that failed to be written would
            // be lost to the reader, and a message with no file would describe
            // a picture nobody can open.
            let anchored = match source_asset {
                Some(source_id) => Some(anchor_extracted_frame(conn, &did, &source_id, &fname)?),
                None => None,
            };
            crate::db::discussions::insert_context_file(
                conn,
                &file_id,
                &did,
                &fname,
                &mime_clone,
                original_size,
                &text,
                dp.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            if let Some((message_id, source_id)) = &anchored {
                conn.execute(
                    "UPDATE context_files
                     SET message_id = ?2, extracted_from_asset_id = ?3
                     WHERE id = ?1",
                    rusqlite::params![file_id, message_id, source_id],
                )?;
            }
            if let Some(message_id) = &owning_message_for_db {
                conn.execute(
                    "UPDATE context_files SET message_id = ?2 WHERE id = ?1",
                    rusqlite::params![file_id, message_id],
                )?;
            }
            Ok(anchored)
        })
        .await;

    let anchored = insert_result.map_err(|e| format!("DB error: {e}"))?;
    let message_id =
        owning_message.or_else(|| anchored.as_ref().map(|(message_id, _)| message_id.clone()));
    let file = crate::models::ContextFile {
        id,
        discussion_id,
        filename,
        mime_type: mime,
        original_size,
        extracted_size,
        disk_path,
        // A composer upload stays pending until the user sends a message;
        // a frame or a file an agent linked belongs to its message already.
        message_id,
        // Neither a user upload nor a linked file has an attested generation job.
        ai_generation: None,
        extracted_from_asset_id: anchored.map(|(_, source_id)| source_id),
        created_at: chrono::Utc::now(),
    };
    Ok((file, suggested_skills))
}

/// Prepare files and destinations before inserting the agent message (KT-954).
/// The caller persists the returned body as the initial message, then publishes
/// its files; this must not silently edit an already-persisted message.
/// Only files under the run's own folder or its project are taken; secrets
/// and Kronn's data dir never are.
pub(crate) async fn attach_files_linked_in_message(
    state: &AppState,
    discussion_id: &str,
    message_id: &str,
    content: &str,
    workspace_path: Option<&str>,
    project_path: &str,
) -> Option<String> {
    use crate::core::message_file_links::{
        local_link_targets, points_to_attached_file, resolve_attachable, rewrite_local_links,
        Refusal, MAX_LINKED_FILE_BYTES,
    };
    use std::io::Read;

    let targets = local_link_targets(content);
    if targets.is_empty() {
        return None;
    }
    // A run with neither its own folder nor a project ran in the shared
    // system temp dir: nothing there is the agent's to publish.
    if workspace_path.is_none() && project_path.is_empty() {
        return None;
    }
    let mut allowed_roots = Vec::new();
    if let Ok(dir) = crate::agents::runner::resolve_agent_work_dir(workspace_path, project_path) {
        allowed_roots.push(dir);
    }
    if !project_path.is_empty() {
        allowed_roots.push(crate::core::scanner::resolve_host_path(project_path));
    }
    let forbidden_roots: Vec<std::path::PathBuf> =
        crate::core::config::config_dir().into_iter().collect();
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());

    let did = discussion_id.to_string();
    let existing = state
        .db
        .with_conn(move |conn| Ok(crate::db::discussions::list_context_files(conn, &did)?))
        .await
        .unwrap_or_default();

    let mut attached = 0usize;
    let mut bytes_read = 0u64;
    let mut rewrites = std::collections::BTreeMap::new();
    let href = |id: &str| format!("/api/discussions/{discussion_id}/context-files/{id}/content");
    let unavailable = |reason: &str| format!("#kronn-local-file-unavailable-{reason}");
    for target in targets {
        // A URL already naming one of this discussion's assets stays usable.
        // A mirrored discussion remaps its ID while retaining the asset UUID.
        if let Some(file) = target
            .strip_prefix("/api/discussions/")
            .and_then(|tail| tail.split_once("/context-files/"))
            .and_then(|(_, tail)| tail.strip_suffix("/content"))
            .and_then(|id| existing.iter().find(|file| file.id == id))
        {
            rewrites.insert(target, href(&file.id));
            continue;
        }
        // A previously attached asset is already available to this discussion.
        // Match its actual stored path, never a filename/size guess.
        if let Some(file) = existing.iter().find(|file| {
            file.disk_path.as_deref().is_some_and(|path| {
                points_to_attached_file(&target, std::path::Path::new(path), home.as_deref())
            })
        }) {
            rewrites.insert(target, href(&file.id));
            continue;
        }
        if attached >= 8 || bytes_read >= MAX_LINKED_FILE_BYTES {
            rewrites.insert(target, unavailable("limit"));
            continue;
        }
        let path =
            match resolve_attachable(&target, &allowed_roots, &forbidden_roots, home.as_deref()) {
                Ok(path) => path,
                Err(refusal) => {
                    tracing::info!(discussion_id, ?refusal, "linked local file not attached");
                    let reason = match refusal {
                        Refusal::Missing | Refusal::NotAFile => "missing",
                        Refusal::Sensitive => "sensitive",
                        Refusal::OutsideAllowedRoots => "outside",
                        Refusal::TooLarge => "limit",
                    };
                    rewrites.insert(target, unavailable(reason));
                    continue;
                }
            };
        let Some(filename) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            rewrites.insert(target, unavailable("missing"));
            continue;
        };
        let mut data = Vec::new();
        let remaining = MAX_LINKED_FILE_BYTES - bytes_read;
        let read_result = std::fs::File::open(&path).and_then(|file| {
            // Do not read a file already known to exceed the remaining budget.
            if file.metadata()?.len() > remaining {
                return Ok(false);
            }
            file.take(remaining + 1).read_to_end(&mut data)?;
            Ok(data.len() as u64 <= remaining)
        });
        // Count partial reads and the growth probe too: repeated failures must
        // not let a message reread a fresh 64 MiB for every linked path.
        bytes_read = bytes_read
            .saturating_add(data.len() as u64)
            .min(MAX_LINKED_FILE_BYTES);
        match read_result {
            Ok(true) => {}
            Ok(false) => {
                rewrites.insert(target, unavailable("limit"));
                continue;
            }
            Err(error) => {
                tracing::warn!(discussion_id, %error, "linked local file unreadable");
                rewrites.insert(target, unavailable("missing"));
                continue;
            }
        }
        match store_context_file(
            state,
            discussion_id.to_string(),
            filename,
            &data,
            ContextFileOwner::Message(message_id.to_string()),
        )
        .await
        {
            Ok((file, _)) => {
                attached += 1;
                rewrites.insert(target, href(&file.id));
            }
            Err(error) => {
                tracing::warn!(discussion_id, %error, "linked local file not stored");
                rewrites.insert(target, unavailable("unavailable"));
            }
        }
    }
    let rewritten = rewrite_local_links(content, &rewrites);
    (rewritten != content).then_some(rewritten)
}

/// Remove only files staged for a message whose insertion failed. Some callers
/// can report an error after committing the message, so check its existence
/// under the database lock before deleting anything.
pub(crate) async fn discard_uncommitted_message_files(
    state: &AppState,
    discussion_id: &str,
    message_id: &str,
) {
    let (did, mid) = (discussion_id.to_owned(), message_id.to_owned());
    let paths = state.db.with_conn(move |conn| {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND discussion_id = ?2)",
            rusqlite::params![mid, did], |row| row.get(0),
        )?;
        if exists { return Ok(Vec::<String>::new()); }
        let paths = conn.prepare(
            "SELECT disk_path FROM context_files WHERE message_id = ?1 AND discussion_id = ?2 AND disk_path IS NOT NULL",
        )?.query_map(rusqlite::params![mid, did], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        conn.execute(
            "DELETE FROM context_files WHERE message_id = ?1 AND discussion_id = ?2",
            rusqlite::params![mid, did],
        )?;
        Ok(paths)
    }).await;
    match paths {
        Ok(paths) => {
            for path in paths {
                crate::core::context_files::delete_image_from_disk(&path);
            }
        }
        Err(error) => {
            tracing::warn!(discussion_id, %error, "uncommitted linked files could not be cleaned up")
        }
    }
}

/// Checks the clip a frame claims to come from, and writes the message that
/// will carry the frame.
///
/// The source is verified here and not only in the browser: an id is
/// guessable, and a picture said to come from a clip of this room must
/// actually come from one. Returns the new message id and the source id.
fn anchor_extracted_frame(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    source_asset_id: &str,
    filename: &str,
) -> anyhow::Result<(String, String)> {
    let source = crate::db::discussions::get_context_file(conn, source_asset_id)?
        .ok_or_else(|| anyhow::anyhow!("the clip this frame comes from no longer exists"))?;
    if source.discussion_id != discussion_id {
        anyhow::bail!("the source clip does not belong to this discussion");
    }
    // The recorded type is not always the truth: every clip stored before
    // `mime_from_extension` knew about video sits in the database as
    // `text/plain`, so a check on the type alone refused the very files this
    // feature exists for. The extension is the same fallback the viewer uses.
    let looks_like_video = source.mime_type.starts_with("video/")
        || matches!(
            source
                .filename
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "mp4" | "m4v" | "webm" | "mov" | "ogv"
        );
    if !looks_like_video {
        anyhow::bail!(
            "a frame can only be taken out of a video ({}, {})",
            source.filename,
            source.mime_type
        );
    }

    let now = chrono::Utc::now();
    let message_id = uuid::Uuid::new_v4().to_string();
    let message = crate::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: message_id.clone(),
        role: crate::models::MessageRole::User,
        channel: crate::models::MessageChannel::Main,
        // Says what happened, in the transcript, at the moment it happened.
        // The provenance itself lives on the file, so every surface can show
        // it — this text is for the reader scrolling by.
        content: format!("Dernière image de « {} »", source.filename),
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: Some(format!("kronn-frame:{source_asset_id}:{filename}")),
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    crate::db::discussions::insert_message(conn, discussion_id, &message)?;
    Ok((message_id, source.id))
}

/// GET /api/discussions/:id/context-files
pub async fn list_context_files(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::ContextFile>>> {
    match state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::list_context_files(conn, &discussion_id)
                .map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        Ok(files) => Json(ApiResponse::ok(files)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

/// Body for POST /api/discussions/:id/context-files/link-pending.
#[derive(serde::Deserialize)]
pub struct LinkPendingRequest {
    pub message_id: String,
    /// When present, pin only these uploads. The composer omits this field and
    /// keeps its historical "all pending" behaviour; MCP agents always send
    /// the exact ids returned by their own uploads so concurrent drafts cannot
    /// steal each other's files.
    #[serde(default)]
    pub file_ids: Option<Vec<String>>,
}

/// POST /api/discussions/:id/context-files/link-pending
///
/// Pin every still-pending (composer-staged) file of a discussion to a given
/// message. The in-disc composer links implicitly at send time, but the
/// initial-creation popup (NewDiscussionForm) uploads files AFTER the first
/// message already exists and runs the agent via `run_agent` (which never
/// links) — so without this, popup attachments stay pending and get vacuumed
/// into message #2 on the next send. The frontend calls this with the first
/// message id right after the popup upload. Returns how many were linked.
pub async fn link_pending_context_files(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    Json(req): Json<LinkPendingRequest>,
) -> Json<ApiResponse<usize>> {
    if req
        .file_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > crate::core::context_files::MAX_PENDING_FILES_PER_DISCUSSION)
    {
        return Json(ApiResponse::err(format!(
            "Cannot link more than {} context files at once",
            crate::core::context_files::MAX_PENDING_FILES_PER_DISCUSSION
        )));
    }
    let did = discussion_id.clone();
    let message_id = req.message_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            match req.file_ids {
                Some(file_ids) => crate::db::discussions::link_selected_context_files_to_message(
                    conn,
                    &did,
                    &req.message_id,
                    &file_ids,
                ),
                None => crate::db::discussions::link_pending_context_files_to_message(
                    conn,
                    &did,
                    &req.message_id,
                ),
            }
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        Ok(n) => {
            if n > 0 {
                let _ = state
                    .ws_broadcast
                    .send(crate::models::WsMessage::ContextFilesChanged {
                        discussion_id: discussion_id.clone(),
                        message_id: message_id.clone(),
                    });
                crate::api::federation::federate_attachments_for_message(
                    &state,
                    &discussion_id,
                    &message_id,
                )
                .await;
            }
            Json(ApiResponse::ok(n))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

/// GET /api/discussions/:id/context-files/:file_id/content
///
/// Streams the raw bytes of an uploaded image so the frontend can render a
/// thumbnail in the message bubble. Security: the on-disk path is resolved
/// from the DB row keyed by BOTH discussion_id AND file_id — a client never
/// supplies a path, so there is no traversal surface. Only image rows have a
/// `disk_path`; text files (disk_path NULL) and unknown ids return 404.
pub async fn get_context_file_content(
    State(state): State<AppState>,
    Path((discussion_id, file_id)): Path<(String, String)>,
) -> Response {
    let row = state.db.with_conn(move |conn| {
        conn.query_row(
            "SELECT disk_path, mime_type, filename FROM context_files WHERE id = ?1 AND discussion_id = ?2",
            rusqlite::params![file_id, discussion_id],
            |r| Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            )),
        ).map_err(|e| anyhow::anyhow!(e))
    }).await;

    let (disk_path, mime_type, filename) = match row {
        Ok((Some(p), mime, name)) => (p, mime, name),
        // No row, or a text file with no stored bytes.
        _ => return (StatusCode::NOT_FOUND, "File not found").into_response(),
    };

    let bytes = match tokio::fs::read(&disk_path).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::NOT_FOUND, "File content missing on disk").into_response(),
    };

    // Derive the Content-Type from the filename extension rather than trusting
    // the stored mime_type: legacy rows (pre-0.8.8) saved images as the default
    // `text/plain`, which would make the browser render the bytes as text when
    // opened. mime_from_extension is the single source of truth and now maps
    // every image extension. Fall back to the stored mime, then octet-stream.
    let derived = crate::core::context_files::mime_from_extension(&filename);
    let content_type = if derived != "text/plain" {
        derived.to_string()
    } else if !mime_type.is_empty() && mime_type != "text/plain" {
        mime_type
    } else {
        derived.to_string()
    };
    // Strip quotes AND control chars (CR/LF) so a crafted filename can't inject
    // headers into the Content-Disposition value.
    let safe_name: String = filename
        .chars()
        .filter(|c| *c != '"' && !c.is_control())
        .collect();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            // Inline so <img>/blob can render it; filename is best-effort.
            (
                header::CONTENT_DISPOSITION,
                format!("inline; filename=\"{}\"", safe_name),
            ),
            (header::CACHE_CONTROL, "private, max-age=3600".to_string()),
        ],
        bytes,
    )
        .into_response()
}

/// DELETE /api/discussions/:id/context-files/:file_id
pub async fn delete_context_file(
    State(state): State<AppState>,
    Path((discussion_id, file_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    // Get disk_path before deleting (to clean up image files)
    let fid = file_id.clone();
    let did = discussion_id.clone();
    let deleted_file_id = file_id.clone();
    let disk_path: Option<String> = state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                "SELECT disk_path FROM context_files WHERE id = ?1 AND discussion_id = ?2",
                rusqlite::params![fid, did],
                |row| row.get(0),
            )
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .ok()
        .flatten();

    match state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::delete_context_file(conn, &discussion_id, &file_id)
                .map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        Ok(true) => {
            if let Some(path) = disk_path {
                crate::core::context_files::delete_image_from_disk(&path);
            }
            // A media job still points at the file it produced. Left alone,
            // its bubble keeps offering "open the media" on bytes that no
            // longer exist — a promise the click cannot keep. Cleared here so
            // the answer survives a reload, rather than patched in the UI.
            forget_deleted_media_asset(&state, &deleted_file_id).await;
            Json(ApiResponse::<()>::ok(()))
        }
        Ok(false) => Json(ApiResponse::<()>::err("Context file not found".to_string())),
        Err(e) => Json(ApiResponse::<()>::err(format!("DB error: {e}"))),
    }
}

/// Detaches a deleted asset from the media job that produced it, and republishes
/// the run so open discussions stop offering it.
///
/// Best effort on purpose: the file IS gone, and failing the deletion because a
/// projection could not be refreshed would be the wrong trade. The next relist
/// still reads the cleared row.
async fn forget_deleted_media_asset(state: &AppState, file_id: &str) {
    let lookup = file_id.to_string();
    let job_id = state
        .db
        .with_conn(move |conn| {
            use rusqlite::OptionalExtension as _;
            let job_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM media_jobs WHERE context_file_id = ?1",
                    rusqlite::params![lookup],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| anyhow::anyhow!(e))?;
            if let Some(job_id) = job_id.as_deref() {
                conn.execute(
                    "UPDATE media_jobs SET context_file_id = NULL WHERE id = ?1",
                    rusqlite::params![job_id],
                )
                .map_err(|e| anyhow::anyhow!(e))?;
            }
            Ok(job_id)
        })
        .await;
    if let Ok(Some(job_id)) = job_id {
        let _ = crate::api::shared_runs::publish_media_job(state, &job_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[tokio::test]
    async fn agent_file_links_persist_exact_assets_and_refuse_unsafe_sources() {
        use http_body_util::BodyExt;
        use std::sync::Arc;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for (name, bytes) in [("a/out.txt", b"first"), ("b/out.txt", b"other")] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        std::fs::write(root.path().join(".env"), b"PRIVATE").unwrap();
        std::fs::write(outside.path().join("private.txt"), b"PRIVATE").unwrap();
        std::fs::File::create(root.path().join("huge.txt"))
            .unwrap()
            .set_len(crate::core::message_file_links::MAX_LINKED_FILE_BYTES + 1)
            .unwrap();
        let content = format!("[first]({}/a/out.txt) [other]({}/b/out.txt) [secret]({}/.env) [outside]({}/private.txt) [large]({}/huge.txt)", root.path().display(), root.path().display(), root.path().display(), outside.path().display(), root.path().display());
        let db = Arc::new(Database::open_in_memory().unwrap());
        let path = root.path().to_string_lossy().into_owned();
        db.with_conn(move |conn| {
            conn.execute("INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p', 'fixture', ?1, datetime('now'), datetime('now'))", [&path])?;
            conn.execute("INSERT INTO discussions (id, project_id, title, created_at, updated_at) VALUES ('d', 'p', 'fixture', datetime('now'), datetime('now'))", [])?;
            Ok(())
        }).await.unwrap();
        let state = AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let rewritten = attach_files_linked_in_message(
            &state,
            "d",
            "m",
            &content,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        let message: crate::models::DiscussionMessage = serde_json::from_value(serde_json::json!({
            "id": "m", "role": "Agent", "content": rewritten,
            "agent_type": "Codex", "timestamp": chrono::Utc::now(),
        }))
        .unwrap();
        state
            .db
            .with_conn(move |conn| {
                assert_eq!(
                    conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                        .get::<_, i64>(0))?,
                    0,
                    "preparing attachments must not insert or revise the message"
                );
                crate::db::discussions::insert_native_agent_message_with_checkpoint(
                    conn,
                    "d",
                    &message,
                    true,
                    None,
                    &crate::models::AgentType::Codex,
                    &[],
                    false,
                    None,
                    None,
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let (files, saved) = state
            .db
            .with_conn(|conn| {
                Ok((
                    crate::db::discussions::list_context_files(conn, "d")?,
                    conn.query_row("SELECT content FROM messages WHERE id = 'm'", [], |row| {
                        row.get::<_, String>(0)
                    })?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(saved, rewritten);
        assert_eq!(
            files.len(),
            2,
            "same name and size must not merge different contents"
        );
        for file in &files {
            assert_eq!(file.filename, "out.txt");
            assert_eq!(file.message_id.as_deref(), Some("m"));
            let response =
                get_context_file_content(State(state.clone()), Path(("d".into(), file.id.clone())))
                    .await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let label = if bytes.as_ref() == b"first" {
                "first"
            } else {
                assert_eq!(bytes.as_ref(), b"other");
                "other"
            };
            assert!(rewritten.contains(&format!(
                "[{label}](/api/discussions/d/context-files/{}/content)",
                file.id
            )));
        }
        for reason in ["sensitive", "outside", "limit"] {
            assert!(rewritten.contains(&format!("#kronn-local-file-unavailable-{reason}")));
        }
        let stored_link = format!("[existing]({})", files[0].disk_path.as_deref().unwrap());
        let existing = attach_files_linked_in_message(
            &state,
            "d",
            "m",
            &stored_link,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        assert!(existing.contains(&files[0].id));
        let count = state
            .db
            .with_conn(|conn| Ok(crate::db::discussions::list_context_files(conn, "d")?.len()))
            .await
            .unwrap();
        assert_eq!(count, 2, "an exact existing asset is reused");
        let mirrored = format!(
            "[mirror](/api/discussions/origin/context-files/{}/content)",
            files[0].id
        );
        let mapped = attach_files_linked_in_message(
            &state,
            "d",
            "m",
            &mirrored,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        assert_eq!(
            mapped,
            format!(
                "[mirror](/api/discussions/d/context-files/{}/content)",
                files[0].id
            )
        );
        assert!(attach_files_linked_in_message(
            &state,
            "d",
            "m",
            &mapped,
            None,
            &root.path().to_string_lossy()
        )
        .await
        .is_none());
        let source = format!("[new]({}/a/out.txt)", root.path().display());
        attach_files_linked_in_message(
            &state,
            "d",
            "failed",
            &source,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        let staged = state
            .db
            .with_conn(|conn| {
                Ok(crate::db::discussions::list_context_files(conn, "d")?
                    .into_iter()
                    .find(|file| file.message_id.as_deref() == Some("failed"))
                    .unwrap())
            })
            .await
            .unwrap();
        assert!(std::path::Path::new(staged.disk_path.as_deref().unwrap()).exists());
        discard_uncommitted_message_files(&state, "d", "failed").await;
        assert!(!std::path::Path::new(staged.disk_path.as_deref().unwrap()).exists());
        discard_uncommitted_message_files(&state, "d", "m").await;
        let retained = state
            .db
            .with_conn(|conn| Ok(crate::db::discussions::list_context_files(conn, "d")?))
            .await
            .unwrap();
        assert_eq!(
            retained.len(),
            2,
            "cleanup must keep committed and reused assets"
        );
        for file in retained {
            assert!(std::path::Path::new(file.disk_path.as_deref().unwrap()).exists());
        }
        let mut many_links = String::new();
        for index in 0..9 {
            let path = root.path().join(format!("small-{index}.txt"));
            std::fs::write(&path, b"x").unwrap();
            many_links.push_str(&format!("[{index}]({}) ", path.display()));
        }
        let bounded = attach_files_linked_in_message(
            &state,
            "d",
            "many",
            &many_links,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        assert_eq!(bounded.matches("/context-files/").count(), 8);
        assert!(bounded.contains("[8](#kronn-local-file-unavailable-limit)"));
        discard_uncommitted_message_files(&state, "d", "many").await;

        for name in ["half-a.bin", "half-b.bin"] {
            std::fs::File::create(root.path().join(name))
                .unwrap()
                .set_len(crate::core::message_file_links::MAX_LINKED_FILE_BYTES / 2 + 1)
                .unwrap();
        }
        let large_links = format!(
            "[a]({}/half-a.bin) [b]({}/half-b.bin) [tiny]({}/small-0.txt)",
            root.path().display(),
            root.path().display(),
            root.path().display()
        );
        let bounded = attach_files_linked_in_message(
            &state,
            "d",
            "budget",
            &large_links,
            None,
            &root.path().to_string_lossy(),
        )
        .await
        .unwrap();
        assert_eq!(bounded.matches("/context-files/").count(), 2);
        assert!(bounded.contains("[b](#kronn-local-file-unavailable-limit)"));
        discard_uncommitted_message_files(&state, "d", "budget").await;
    }

    /// A discussion holding one clip and one plain image.
    async fn seeded() -> Database {
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d-1', 'x', '2026-09-02 10:00:00', '2026-09-02 10:00:00'),
                        ('d-2', 'y', '2026-09-02 10:00:00', '2026-09-02 10:00:00')",
                [],
            )?;
            for (id, disc, name, mime) in [
                ("clip-1", "d-1", "clip.mp4", "video/mp4"),
                ("pic-1", "d-1", "shot.png", "image/png"),
                ("clip-elsewhere", "d-2", "other.mp4", "video/mp4"),
            ] {
                conn.execute(
                    "INSERT INTO context_files
                        (id, discussion_id, filename, mime_type, original_size,
                         extracted_size, extracted_text, disk_path, created_at)
                     VALUES (?1, ?2, ?3, ?4, 10, 0, '', '/tmp/x', '2026-09-02 10:00:00')",
                    rusqlite::params![id, disc, name, mime],
                )?;
            }
            Ok(())
        })
        .await
        .expect("seed");
        db
    }

    #[tokio::test]
    async fn a_frame_gets_its_own_message_and_names_the_clip_it_came_from() {
        let db = seeded().await;
        let (message_id, source_id) = db
            .with_conn(|conn| anchor_extracted_frame(conn, "d-1", "clip-1", "clip-last-frame.png"))
            .await
            .expect("anchored");
        assert_eq!(source_id, "clip-1");

        let (role, content) = db
            .with_read_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT role, content FROM messages WHERE id = ?1",
                    rusqlite::params![message_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?)
            })
            .await
            .expect("the message was written");
        assert_eq!(role, "User");
        // Says what happened where it happened, naming the clip.
        assert!(content.contains("clip.mp4"), "got {content}");
    }

    #[tokio::test]
    async fn a_frame_cannot_claim_a_clip_of_another_discussion() {
        // An id is guessable; a room is not shared. Checking only in the
        // browser would let a picture claim provenance it never had.
        let db = seeded().await;
        let error = db
            .with_conn(|conn| anchor_extracted_frame(conn, "d-1", "clip-elsewhere", "f.png"))
            .await
            .expect_err("refused")
            .to_string();
        assert!(error.contains("does not belong"), "got {error}");
    }

    #[tokio::test]
    async fn a_clip_recorded_before_video_mimes_existed_is_still_a_clip() {
        // Every clip stored before `mime_from_extension` knew about video sits
        // in the database as `text/plain`. Refusing those refused the very
        // files this feature exists for — reported from a real discussion.
        let db = seeded().await;
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO context_files
                    (id, discussion_id, filename, mime_type, original_size,
                     extracted_size, extracted_text, disk_path, created_at)
                 VALUES ('legacy-clip', 'd-1', 'seedance.mp4', 'text/plain', 10, 0, '',
                         '/tmp/legacy.mp4', '2026-09-02 10:00:00')",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("seed");

        let (_, source_id) = db
            .with_conn(|conn| anchor_extracted_frame(conn, "d-1", "legacy-clip", "f.png"))
            .await
            .expect("a legacy clip is still a clip");
        assert_eq!(source_id, "legacy-clip");
    }

    #[tokio::test]
    async fn a_frame_cannot_claim_something_that_is_not_a_video() {
        let db = seeded().await;
        let error = db
            .with_conn(|conn| anchor_extracted_frame(conn, "d-1", "pic-1", "f.png"))
            .await
            .expect_err("refused")
            .to_string();
        assert!(
            error.contains("only be taken out of a video"),
            "got {error}"
        );
    }

    #[tokio::test]
    async fn a_refused_frame_leaves_no_message_behind() {
        let db = seeded().await;
        let _ = db
            .with_conn(|conn| anchor_extracted_frame(conn, "d-1", "pic-1", "f.png"))
            .await;
        let messages: i64 = db
            .with_read_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?)
            })
            .await
            .expect("count");
        assert_eq!(messages, 0, "a refusal must not write a transcript slot");
    }
}
