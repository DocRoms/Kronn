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
                    Err(e) => {
                        return Json(ApiResponse::err(format!("Failed to read upload: {e}")))
                    }
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

    // Extract content (text or image)
    let content = match crate::core::context_files::extract_content(&filename, &data) {
        Ok(c) => c,
        Err(e) => return Json(ApiResponse::err(e.to_string())),
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
        crate::core::context_files::ExtractedContent::Text(text) => (text, None),
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
                            return Json(ApiResponse::err(format!(
                                "Failed to save file: {e} / fallback: {e2}"
                            )))
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
                            return Json(ApiResponse::err(format!(
                                "Failed to save image: {e} / fallback: {e2}"
                            )))
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

    let source_asset = extracted_from.clone();
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
            Ok(anchored)
        })
        .await;

    match insert_result {
        Ok(anchored) => {
            let file = crate::models::ContextFile {
                id,
                discussion_id,
                filename,
                mime_type: mime,
                original_size,
                extracted_size,
                disk_path,
                // Freshly uploaded files are pending until the user sends a
                // message; send_message pins them to that message id. A frame
                // taken out of a clip owns its own message from the start.
                message_id: anchored.as_ref().map(|(message_id, _)| message_id.clone()),
                // A user upload has no attested generation job.
                ai_generation: None,
                extracted_from_asset_id: anchored.map(|(_, source_id)| source_id),
                created_at: chrono::Utc::now(),
            };
            Json(ApiResponse::ok(crate::models::UploadContextFileResponse {
                file,
                suggested_skills,
            }))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
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
        assert!(error.contains("only be taken out of a video"), "got {error}");
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
