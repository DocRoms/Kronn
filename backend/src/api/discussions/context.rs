// Context Files: per-discussion uploads (multipart) that feed extra
// background context into agent prompts. Files are extracted to text
// at upload time and stored in the DB; binaries land on disk under
// the discussion's worktree. Suggested skills are auto-derived from
// the file extension to nudge the user toward the right experts.

use axum::{
    extract::{Path, State},
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
        Ok(None) => return Json(ApiResponse::<crate::models::UploadContextFileResponse>::err("No file provided".to_string())),
        Err(e) => return Json(ApiResponse::<crate::models::UploadContextFileResponse>::err(format!("Multipart error: {e}"))),
    };

    // Check file count limit
    let did = discussion_id.clone();
    let count = state.db.with_conn(move |conn| {
        crate::db::discussions::count_context_files(conn, &did).map_err(|e| anyhow::anyhow!(e))
    }).await.unwrap_or(0);

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

    // Resolve the work directory for this discussion (project path or temp dir).
    // Images are saved there so agents can read them with their file tools.
    let did_for_path = discussion_id.clone();
    let work_dir: std::path::PathBuf = state.db.with_conn(move |conn| {
        let project_id: Option<String> = conn.query_row(
            "SELECT project_id FROM discussions WHERE id = ?1",
            rusqlite::params![did_for_path],
            |row| row.get(0),
        ).unwrap_or(None);
        let path = if let Some(pid) = project_id {
            conn.query_row(
                "SELECT path FROM projects WHERE id = ?1",
                rusqlite::params![pid],
                |row| row.get::<_, String>(0),
            ).ok()
        } else {
            None
        };
        Ok(std::path::PathBuf::from(path.unwrap_or_else(|| std::env::temp_dir().to_string_lossy().to_string())))
    }).await.unwrap_or_else(|_: anyhow::Error| std::env::temp_dir());

    let id = uuid::Uuid::new_v4().to_string();
    let mime = crate::core::context_files::mime_from_extension(&filename).to_string();
    let original_size = data.len() as u64;
    let suggested_skills = crate::core::context_files::suggest_skills(&filename);

    // Handle text vs image
    let (extracted_text, disk_path) = match content {
        crate::core::context_files::ExtractedContent::Text(text) => (text, None),
        crate::core::context_files::ExtractedContent::Image { data: img_data, ext } => {
            match crate::core::context_files::save_image_to_dir(&work_dir, &id, &filename, &ext, &img_data) {
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
                        Err(e2) => return Json(ApiResponse::err(format!("Failed to save image: {e} / fallback: {e2}"))),
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

    let insert_result = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_context_file(
            conn, &file_id, &did, &fname, &mime_clone, original_size, &text, dp.as_deref(),
        ).map_err(|e| anyhow::anyhow!(e))
    }).await;

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
    match state.db.with_conn(move |conn| {
        crate::db::discussions::list_context_files(conn, &discussion_id).map_err(|e| anyhow::anyhow!(e))
    }).await {
        Ok(files) => Json(ApiResponse::ok(files)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

/// DELETE /api/discussions/:id/context-files/:file_id
pub async fn delete_context_file(
    State(state): State<AppState>,
    Path((discussion_id, file_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    // Get disk_path before deleting (to clean up image files)
    let fid = file_id.clone();
    let did = discussion_id.clone();
    let disk_path: Option<String> = state.db.with_conn(move |conn| {
        conn.query_row(
            "SELECT disk_path FROM context_files WHERE id = ?1 AND discussion_id = ?2",
            rusqlite::params![fid, did],
            |row| row.get(0),
        ).map_err(|e| anyhow::anyhow!(e))
    }).await.ok().flatten();

    match state.db.with_conn(move |conn| {
        crate::db::discussions::delete_context_file(conn, &discussion_id, &file_id).map_err(|e| anyhow::anyhow!(e))
    }).await {
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

