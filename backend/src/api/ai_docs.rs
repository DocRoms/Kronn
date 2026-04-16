//! AI Documentation File Browser — list, search, and read ai/ files.

use axum::{
    extract::{Path, Query, State},
    Json,
};

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

// ═══════════════════════════════════════════════════════════════════════════════
// AI Documentation File Browser
// ═══════════════════════════════════════════════════════════════════════════════

/// GET /api/projects/:id/ai-files
/// Returns the tree of `.md` files under `ai/`.
pub async fn list_ai_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<AiFileNode>>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let ai_dir = project_path.join("ai");
        if !ai_dir.is_dir() {
            return vec![];
        }
        build_ai_file_tree(&ai_dir, "ai")
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

/// Recursively build a tree of `.md` files from the `ai/` directory.
fn build_ai_file_tree(dir: &std::path::Path, rel_prefix: &str) -> Vec<AiFileNode> {
    let mut nodes = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };

    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}/{}", rel_prefix, name);
        let file_type = match entry.file_type().or_else(|_| entry.metadata().map(|m| m.file_type())) {
            Ok(ft) => ft,
            Err(_) => continue, // skip entries with unreadable metadata
        };

        if file_type.is_dir() {
            let children = build_ai_file_tree(&entry.path(), &path);
            if !children.is_empty() {
                nodes.push(AiFileNode { path, name, is_dir: true, children });
            }
        } else if name.ends_with(".md") {
            nodes.push(AiFileNode { path, name, is_dir: false, children: vec![] });
        }
    }
    nodes
}

#[derive(Debug, serde::Deserialize)]
pub struct AiFileQuery {
    pub path: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct AiSearchQuery {
    pub q: String,
}

/// GET /api/projects/:id/ai-search?q=mcp
/// Full-text search across all `.md` files in `ai/`. Returns paths + match counts.
pub async fn search_ai_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiSearchQuery>,
) -> Json<ApiResponse<Vec<AiSearchResult>>> {
    let q = query.q.trim().to_string();
    if q.is_empty() {
        return Json(ApiResponse::ok(vec![]));
    }

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let ai_dir = project_path.join("ai");
        if !ai_dir.is_dir() {
            return vec![];
        }
        let mut results = Vec::new();
        search_ai_dir_recursive(&ai_dir, "ai", &q.to_lowercase(), &mut results);
        // Sort by match_count descending
        results.sort_by_key(|r| std::cmp::Reverse(r.match_count));
        results
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

fn search_ai_dir_recursive(dir: &std::path::Path, rel_prefix: &str, query: &str, results: &mut Vec<AiSearchResult>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}/{}", rel_prefix, name);
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            search_ai_dir_recursive(&entry.path(), &path, query, results);
        } else if name.ends_with(".md") {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let lower = content.to_lowercase();
                let mut count = 0u32;
                let mut start = 0;
                while let Some(idx) = lower[start..].find(query) {
                    count += 1;
                    start += idx + query.len();
                }
                if count > 0 {
                    results.push(AiSearchResult { path, match_count: count });
                }
            }
        }
    }
}

/// GET /api/projects/:id/ai-file?path=ai/index.md
/// Reads a single file from the `ai/` directory with path traversal protection.
pub async fn read_ai_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiFileQuery>,
) -> Json<ApiResponse<AiFileContent>> {
    // Path traversal protection
    if query.path.contains("..") || !query.path.starts_with("ai/") {
        return Json(ApiResponse::err("Invalid path: must start with ai/ and not contain .."));
    }

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();
    let file_path = query.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let full_path = project_path.join(&file_path);
        match std::fs::read_to_string(&full_path) {
            Ok(content) => Ok(AiFileContent { path: file_path, content }),
            Err(e) => Err(format!("Cannot read file: {}", e)),
        }
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(content) => Json(ApiResponse::ok(content)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
