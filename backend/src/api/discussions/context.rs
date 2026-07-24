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
    // Read the first file field
    let (filename, data) = match multipart.next_field().await {
        Ok(Some(field)) => {
            let fname = field.file_name().unwrap_or("unknown").to_string();
            match field.bytes().await {
                Ok(bytes) => (fname, bytes),
                Err(e) => return Json(ApiResponse::err(format!("Failed to read upload: {e}"))),
            }
        }
        Ok(None) => {
            return Json(
                ApiResponse::<crate::models::UploadContextFileResponse>::err(
                    "No file provided".to_string(),
                ),
            )
        }
        Err(e) => {
            return Json(
                ApiResponse::<crate::models::UploadContextFileResponse>::err(format!(
                    "Multipart error: {e}"
                )),
            )
        }
    };

    // Check file count limit
    let did = discussion_id.clone();
    let count = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::count_context_files(conn, &did).map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .unwrap_or(0);

    if count >= crate::core::context_files::MAX_FILES_PER_DISCUSSION {
        return Json(ApiResponse::err(format!(
            "Maximum {} context files per discussion reached",
            crate::core::context_files::MAX_FILES_PER_DISCUSSION
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

    let insert_result = state
        .db
        .with_conn(move |conn| {
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
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await;

    match insert_result {
        Ok(()) => {
            let file = crate::models::ContextFile {
                id,
                discussion_id,
                filename,
                mime_type: mime,
                original_size,
                extracted_size,
                disk_path,
                // Freshly uploaded files are pending until the user sends a
                // message; send_message pins them to that message id.
                message_id: None,
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
    let did = discussion_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::link_pending_context_files_to_message(
                conn,
                &did,
                &req.message_id,
            )
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await
    {
        Ok(n) => Json(ApiResponse::ok(n)),
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
            Json(ApiResponse::<()>::ok(()))
        }
        Ok(false) => Json(ApiResponse::<()>::err("Context file not found".to_string())),
        Err(e) => Json(ApiResponse::<()>::err(format!("DB error: {e}"))),
    }
}
