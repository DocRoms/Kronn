//! AI Documentation File Browser — list, search, and read files from
//! the project's docs folder (post-pivot `docs/`, alt `doc/`, or
//! legacy `ai/` — picked by `detect_docs_dir`).

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
        let docs_dir = scanner::detect_docs_dir(&project_path);
        if !docs_dir.is_dir() {
            return vec![];
        }
        // Use the actual folder name (`docs`, `doc` or `ai`) as the tree
        // root so the frontend's display matches the on-disk reality.
        let prefix = docs_dir.file_name().and_then(|n| n.to_str()).unwrap_or("docs");
        build_ai_file_tree(&docs_dir, prefix)
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

/// Recursively build a tree of `.md` files from the project's docs folder.
fn build_ai_file_tree(dir: &std::path::Path, rel_prefix: &str) -> Vec<AiFileNode> {
    let mut nodes = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };

    // 0.8.3 UX — sort directories FIRST (A-Z), then files (A-Z).
    // The previous flat alphabetic sort intermixed dirs and files
    // (`architecture/`, `briefing.md`, `coding-rules.md`, `operations/`)
    // which doesn't match the common file-explorer convention users
    // expect (folders grouped at the top). Two-tier key: (is_file, name).
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by(|a, b| {
        let a_is_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        let b_is_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            // Same kind → case-insensitive name compare so `architecture/`
            // and `Architecture/` cohabit predictably regardless of FS
            // case sensitivity.
            _ => a.file_name().to_ascii_lowercase().cmp(&b.file_name().to_ascii_lowercase()),
        }
    });

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
        let docs_dir = scanner::detect_docs_dir(&project_path);
        if !docs_dir.is_dir() {
            return vec![];
        }
        let prefix = docs_dir.file_name().and_then(|n| n.to_str()).unwrap_or("docs");
        let mut results = Vec::new();
        search_ai_dir_recursive(&docs_dir, prefix, &q.to_lowercase(), &mut results);
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
/// Reads a single file from the project's docs folder (post-pivot
/// `docs/`, alt `doc/`, or legacy `ai/`) with path traversal protection.
pub async fn read_ai_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiFileQuery>,
) -> Json<ApiResponse<AiFileContent>> {
    // Path traversal protection — must be confined to one of the
    // recognised docs roots and never contain `..`.
    let allowed_prefix = query.path.starts_with("docs/")
        || query.path.starts_with("doc/")
        || query.path.starts_with("ai/");
    if query.path.contains("..") || !allowed_prefix {
        return Json(ApiResponse::err("Invalid path: must start with docs/, doc/ or ai/ and not contain .."));
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

#[cfg(test)]
mod tests {
    use super::*;

    // 0.8.3 UX regression — verrouille l'ordre dirs-first puis files.
    // Avant : tri alphabétique pur mélangeait `architecture/`,
    // `briefing.md`, `coding-rules.md`, `operations/`. Convention
    // file-explorer attendue : dossiers groupés en haut.
    fn touch(p: &std::path::Path) {
        if let Some(parent) = p.parent() { std::fs::create_dir_all(parent).unwrap(); }
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn tree_lists_dirs_first_then_files_each_alphabetic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Mix: file, dir, file, dir — pre-fix would interleave them.
        touch(&root.join("briefing.md"));
        touch(&root.join("architecture/overview.md"));
        touch(&root.join("coding-rules.md"));
        touch(&root.join("operations/debug.md"));
        touch(&root.join("AGENTS.md"));

        let tree = build_ai_file_tree(root, "docs");
        let names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();

        // Expect: dirs A-Z first, then files A-Z (case-insensitive).
        assert_eq!(
            names,
            vec!["architecture", "operations", "AGENTS.md", "briefing.md", "coding-rules.md"],
            "dirs must come before files; within each group sort case-insensitive A-Z"
        );
    }

    #[test]
    fn tree_recursion_keeps_same_ordering_in_subdirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("architecture/overview.md"));
        touch(&root.join("architecture/sequences/auth.md"));
        touch(&root.join("architecture/README.md"));

        let tree = build_ai_file_tree(root, "docs");
        assert_eq!(tree.len(), 1);
        let arch_children: Vec<&str> = tree[0].children.iter().map(|n| n.name.as_str()).collect();
        // sequences/ (dir) first, then files A-Z.
        assert_eq!(arch_children, vec!["sequences", "overview.md", "README.md"]);
    }

    #[test]
    fn tree_case_insensitive_sort_groups_letters() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Mix uppercase + lowercase — must sort as if lowercase.
        touch(&root.join("Zebra.md"));
        touch(&root.join("apple.md"));
        touch(&root.join("Banana.md"));

        let tree = build_ai_file_tree(root, "docs");
        let names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["apple.md", "Banana.md", "Zebra.md"]);
    }
}
