//! REST API for the user-scoped agent context directory
//! (`~/.kronn/user-context/`).
//!
//! UI surface : a small inline editor that lets the operator manage their
//! cross-project agent prompts without ever opening a terminal. Backed by
//! the same `core::user_context` reader the agent runner uses, so what
//! the UI shows IS what gets injected.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct UserContextFile {
    pub name: String,
    pub size: u64,
    /// File body. None on the LIST endpoint (only filled in GET /:name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WriteUserContextRequest {
    pub content: String,
}

/// GET /api/user-context — list files in `~/.kronn/user-context/`.
/// Excludes README.md (seed file) and dot-prefixed files (editor swaps).
pub async fn list(State(_state): State<AppState>) -> Json<ApiResponse<Vec<UserContextFile>>> {
    let dir = crate::core::user_context::user_context_dir();
    if !dir.exists() {
        return Json(ApiResponse::ok(vec![]));
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return Json(ApiResponse::err(format!("read_dir: {}", e))),
    };
    let mut files: Vec<UserContextFile> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".md") || name.starts_with('.') {
                return None;
            }
            // README.md is the seed file — visible in `ls` but not user-content.
            // Surface it so the user can edit it but mark distinctly via name.
            let metadata = e.metadata().ok()?;
            Some(UserContextFile {
                name,
                size: metadata.len(),
                content: None,
            })
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Json(ApiResponse::ok(files))
}

/// GET /api/user-context/:name — read a single file's content.
pub async fn get(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> Json<ApiResponse<UserContextFile>> {
    if !is_safe_name(&name) {
        return Json(ApiResponse::err("Invalid filename"));
    }
    let path = crate::core::user_context::user_context_dir().join(&name);
    if !path.is_file() {
        return Json(ApiResponse::err("File not found"));
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return Json(ApiResponse::err(format!("read: {}", e))),
    };
    let size = content.len() as u64;
    Json(ApiResponse::ok(UserContextFile {
        name,
        size,
        content: Some(content),
    }))
}

/// PUT /api/user-context/:name — write/replace a file's content.
/// Creates the user-context directory on first write if needed.
pub async fn put(
    State(_state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<WriteUserContextRequest>,
) -> Json<ApiResponse<UserContextFile>> {
    if !is_safe_name(&name) {
        return Json(ApiResponse::err("Invalid filename"));
    }
    // Hard cap at 64 KB. User-context isn't supposed to be a novel.
    if req.content.len() > 64 * 1024 {
        return Json(ApiResponse::err("Content exceeds 64 KB limit"));
    }
    let dir = crate::core::user_context::user_context_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Json(ApiResponse::err(format!("mkdir: {}", e)));
    }
    let path = dir.join(&name);
    if let Err(e) = std::fs::write(&path, &req.content) {
        return Json(ApiResponse::err(format!("write: {}", e)));
    }
    let size = req.content.len() as u64;
    Json(ApiResponse::ok(UserContextFile {
        name,
        size,
        content: Some(req.content),
    }))
}

/// DELETE /api/user-context/:name — delete a file.
pub async fn delete(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> Json<ApiResponse<()>> {
    if !is_safe_name(&name) {
        return Json(ApiResponse::err("Invalid filename"));
    }
    let path = crate::core::user_context::user_context_dir().join(&name);
    if !path.exists() {
        // Already gone. Idempotent.
        return Json(ApiResponse::ok(()));
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("delete: {}", e))),
    }
}

/// Guard against path traversal (`../etc/passwd`) and dot-prefixed names
/// (the LIST endpoint filters them too — keep the door closed at WRITE
/// time as well). Only allows `.md` extension.
fn is_safe_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 100 {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    if name.starts_with('.') {
        return false;
    }
    if !name.ends_with(".md") {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_name_accepts_normal_md_files() {
        assert!(is_safe_name("about-me.md"));
        assert!(is_safe_name("01-conventions.md"));
        assert!(is_safe_name("README.md"));
    }

    #[test]
    fn safe_name_rejects_traversal() {
        assert!(!is_safe_name("../escape.md"));
        assert!(!is_safe_name("foo/bar.md"));
        assert!(!is_safe_name("foo\\bar.md"));
        // Names containing `..` (path traversal attempt) must be rejected.
        assert!(!is_safe_name("foo..bar.md"));
    }

    #[test]
    fn safe_name_rejects_dot_prefixed() {
        assert!(!is_safe_name(".swp.md"));
        assert!(!is_safe_name(".about-me.md"));
    }

    #[test]
    fn safe_name_rejects_non_md_extension() {
        assert!(!is_safe_name("script.sh"));
        assert!(!is_safe_name("data.txt"));
        assert!(!is_safe_name("noext"));
    }

    #[test]
    fn safe_name_rejects_excessive_length() {
        let long = "a".repeat(150) + ".md";
        assert!(!is_safe_name(&long));
    }
}
