//! AI Documentation File Browser — list, search, and read files from
//! the project's docs folder (post-pivot `docs/`, alt `doc/`, or
//! legacy `ai/` — picked by `detect_docs_dir`).

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

/// Max size we'll serve for an inline doc image (10 MB). Guards against a
/// README pointing at a huge binary that just happens to have an image ext.
const MAX_DOC_ASSET_BYTES: u64 = 10 * 1024 * 1024;

/// Image extensions we serve via `doc-asset`, with their Content-Type.
/// Image-only is the security boundary: a doc can never pull a project's
/// source, `.env`, etc. through this route.
const DOC_ASSET_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("bmp", "image/bmp"),
];

fn doc_asset_ext(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Whether a requested asset path is safe + servable: relative (no leading
/// slash), no `..`, and an allowed image extension. The on-disk
/// canonicalize-within-root check in the handler is the second layer.
fn is_servable_asset_path(path: &str) -> bool {
    if path.is_empty() || path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let ext = doc_asset_ext(path);
    DOC_ASSET_TYPES.iter().any(|(e, _)| *e == ext)
}

fn doc_asset_content_type(path: &str) -> &'static str {
    let ext = doc_asset_ext(path);
    DOC_ASSET_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, ct)| *ct)
        .unwrap_or("application/octet-stream")
}

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
        assemble_doc_nodes(&project_path)
    }).await.unwrap_or_default();

    Json(ApiResponse::ok(result))
}

/// Assemble the top-level documentation nodes shown in the doc viewer.
///
/// Returns an explicit `docs/` (or `doc/`/`ai/`) folder node wrapping the
/// recursive `.md` tree, so the user sees the folder they're browsing rather
/// than just its loose contents (pre-0.8.6 the children were returned flat,
/// which read as "where's the rest of my repo?"). When a root README exists
/// (`README.md`, `readme.md`, …) it's appended as a sibling file node, so the
/// human entry point is surfaced too and can be previewed without an IDE.
/// Dirs-first: the `docs/` node precedes the README file.
fn assemble_doc_nodes(project_path: &std::path::Path) -> Vec<AiFileNode> {
    let mut nodes = Vec::new();

    let docs_dir = scanner::detect_docs_dir(project_path);
    if docs_dir.is_dir() {
        // Use the actual folder name (`docs`, `doc` or `ai`) so the display
        // matches the on-disk reality.
        let prefix = docs_dir.file_name().and_then(|n| n.to_str()).unwrap_or("docs").to_string();
        let children = build_ai_file_tree(&docs_dir, &prefix);
        // Only surface the folder node when it actually has docs — an empty
        // `docs/` node would suppress the "run the audit" empty state that
        // un-audited projects rely on.
        if !children.is_empty() {
            nodes.push(AiFileNode { path: prefix.clone(), name: prefix, is_dir: true, children });
        }
    }

    if let Some(readme) = find_root_readme(project_path) {
        nodes.push(AiFileNode { path: readme.clone(), name: readme, is_dir: false, children: vec![] });
    }

    nodes
}

/// Find a root-level README markdown file (case-insensitive: `README.md`,
/// `readme.md`, `Readme.markdown`, …). Returns the actual on-disk filename
/// so the read path matches exactly.
fn find_root_readme(project_path: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(project_path).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("readme") && (lower.ends_with(".md") || lower.ends_with(".markdown")) {
            return Some(name);
        }
    }
    None
}

/// Whether a requested doc path is safe to read: confined to a recognised
/// docs root (`docs/`, `doc/`, `ai/`) OR the project's root README, and
/// never containing `..`. The root-README exception is deliberately narrow
/// (no slash + `readme*.md` only) so it can't be used to read arbitrary
/// root files like `.env` or `Cargo.toml`.
fn is_readable_doc_path(path: &str) -> bool {
    if path.contains("..") {
        return false;
    }
    let in_docs_root = path.starts_with("docs/")
        || path.starts_with("doc/")
        || path.starts_with("ai/");
    let is_root_readme = !path.contains('/') && {
        let l = path.to_ascii_lowercase();
        l.starts_with("readme") && (l.ends_with(".md") || l.ends_with(".markdown"))
    };
    in_docs_root || is_root_readme
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
    // Path traversal protection — confined to a recognised docs root or the
    // project's root README, and never containing `..`.
    if !is_readable_doc_path(&query.path) {
        return Json(ApiResponse::err("Invalid path: must be under docs/, doc/, ai/ or the root README, and not contain .."));
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

/// GET /api/projects/:id/doc-asset?path=docs/screenshots/foo.png
/// Serves an IMAGE file from the project repo so relative `<img>` in a
/// README / doc renders in the viewer (the frontend rewrites relative
/// `src` to point here). Same-origin, so `img-src 'self'` covers it.
/// Defense in depth: image-extension allowlist + no `..` + the resolved
/// path must canonicalize INSIDE the project root.
pub async fn read_doc_asset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiFileQuery>,
) -> Response {
    if !is_servable_asset_path(&query.path) {
        return (StatusCode::BAD_REQUEST, "Invalid asset path: relative image paths only").into_response();
    }

    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "Project not found").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "DB error").into_response(),
    };

    let project_path_str = project.path.clone();
    let rel = query.path.clone();

    let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ()> {
        let root = scanner::resolve_host_path(&project_path_str);
        let canon_root = root.canonicalize().map_err(|_| ())?;
        let canon = root.join(&rel).canonicalize().map_err(|_| ())?;
        // Reject symlinks / `..` that escape the project root.
        if !canon.starts_with(&canon_root) {
            return Err(());
        }
        let meta = std::fs::metadata(&canon).map_err(|_| ())?;
        if !meta.is_file() || meta.len() > MAX_DOC_ASSET_BYTES {
            return Err(());
        }
        std::fs::read(&canon).map_err(|_| ())
    })
    .await;

    match bytes {
        Ok(Ok(data)) => (
            [
                (header::CONTENT_TYPE, doc_asset_content_type(&query.path)),
                (header::CACHE_CONTROL, "private, max-age=60"),
            ],
            data,
        )
            .into_response(),
        _ => (StatusCode::NOT_FOUND, "Asset not found").into_response(),
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

    // ── 0.8.6 UX — explicit docs/ root node + project README ──────────────
    #[test]
    fn assemble_wraps_docs_in_an_explicit_root_node() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("docs/AGENTS.md"));
        touch(&root.join("docs/architecture/overview.md"));

        let nodes = assemble_doc_nodes(root);
        // One top-level node = the `docs/` folder itself, not its loose
        // children — so the user sees the folder they're browsing.
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "docs");
        assert_eq!(nodes[0].path, "docs");
        assert!(nodes[0].is_dir);
        let child_names: Vec<&str> = nodes[0].children.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(child_names, vec!["architecture", "AGENTS.md"]);
    }

    #[test]
    fn assemble_appends_root_readme_after_docs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("docs/AGENTS.md"));
        touch(&root.join("README.md"));

        let nodes = assemble_doc_nodes(root);
        // dirs-first: docs/ folder, then the README file.
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "docs");
        assert_eq!(nodes[1].name, "README.md");
        assert!(!nodes[1].is_dir);
        assert_eq!(nodes[1].path, "README.md");
    }

    #[test]
    fn assemble_readme_only_when_no_docs() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("README.md"));
        let nodes = assemble_doc_nodes(tmp.path());
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "README.md");
    }

    #[test]
    fn assemble_empty_when_no_docs_no_readme() {
        // Preserves the "run the audit" empty state for fresh projects.
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(assemble_doc_nodes(tmp.path()).is_empty());
    }

    #[test]
    fn find_root_readme_case_insensitive_and_extensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("ReAdMe.markdown"));
        assert_eq!(find_root_readme(tmp.path()), Some("ReAdMe.markdown".to_string()));
    }

    #[test]
    fn find_root_readme_ignores_non_readme_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("CHANGELOG.md"));
        touch(&tmp.path().join("notes.txt"));
        assert_eq!(find_root_readme(tmp.path()), None);
    }

    #[test]
    fn read_guard_allows_docs_roots_and_root_readme_only() {
        // Allowed.
        assert!(is_readable_doc_path("docs/AGENTS.md"));
        assert!(is_readable_doc_path("doc/index.md"));
        assert!(is_readable_doc_path("ai/index.md"));
        assert!(is_readable_doc_path("README.md"));
        assert!(is_readable_doc_path("readme.markdown"));
        // Rejected — traversal + arbitrary root/nested files.
        assert!(!is_readable_doc_path("../secret"));
        assert!(!is_readable_doc_path("docs/../Cargo.toml"));
        assert!(!is_readable_doc_path("Cargo.toml"));
        assert!(!is_readable_doc_path(".env"));
        assert!(!is_readable_doc_path("src/README.md"));
    }

    // ── 0.8.6 — doc-asset image serving (relative <img> in README/docs) ───
    #[test]
    fn doc_asset_serves_image_extensions_only() {
        for ok in ["docs/screenshots/foo.png", "logo.svg", "a/b/c.jpeg", "x.WEBP", "i.GIF"] {
            assert!(is_servable_asset_path(ok), "{ok} should be servable");
        }
        for bad in ["docs/notes.md", ".env", "Cargo.toml", "src/main.rs", "foo", "a/b.txt"] {
            assert!(!is_servable_asset_path(bad), "{bad} must NOT be servable (non-image)");
        }
    }

    #[test]
    fn doc_asset_rejects_traversal_and_absolute() {
        assert!(!is_servable_asset_path("../secret.png"));
        assert!(!is_servable_asset_path("docs/../../etc/x.png"));
        assert!(!is_servable_asset_path("/etc/passwd.png"));
        assert!(!is_servable_asset_path("\\windows\\x.png"));
        assert!(!is_servable_asset_path(""));
    }

    #[test]
    fn doc_asset_content_type_maps_by_extension() {
        assert_eq!(doc_asset_content_type("a.png"), "image/png");
        assert_eq!(doc_asset_content_type("a.JPG"), "image/jpeg");
        assert_eq!(doc_asset_content_type("a.svg"), "image/svg+xml");
        assert_eq!(doc_asset_content_type("a.webp"), "image/webp");
    }
}
