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
/// Source browser limits. The viewer is for human-readable code, not vendored
/// dependencies, build artefacts or multi-megabyte generated blobs.
const MAX_SOURCE_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SOURCE_FILES: usize = 10_000;
const MAX_SOURCE_SEARCH_RESULTS: usize = 500;
const MAX_SOURCE_SEARCH_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SOURCE_EXCLUSIONS: usize = 100;

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
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let project_path = scanner::resolve_host_path(&project_path_str);
        assemble_doc_nodes(&project_path)
    })
    .await
    .unwrap_or_default();

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
        let prefix = docs_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("docs")
            .to_string();
        let children = build_ai_file_tree(&docs_dir, &prefix);
        // Only surface the folder node when it actually has docs — an empty
        // `docs/` node would suppress the "run the audit" empty state that
        // un-audited projects rely on.
        if !children.is_empty() {
            nodes.push(AiFileNode {
                path: prefix.clone(),
                name: prefix,
                is_dir: true,
                children,
            });
        }
    }

    if let Some(readme) = find_root_readme(project_path) {
        nodes.push(AiFileNode {
            path: readme.clone(),
            name: readme,
            is_dir: false,
            children: vec![],
        });
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
    let in_docs_root =
        path.starts_with("docs/") || path.starts_with("doc/") || path.starts_with("ai/");
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
            _ => a
                .file_name()
                .to_ascii_lowercase()
                .cmp(&b.file_name().to_ascii_lowercase()),
        }
    });

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}/{}", rel_prefix, name);
        let file_type = match entry
            .file_type()
            .or_else(|_| entry.metadata().map(|m| m.file_type()))
        {
            Ok(ft) => ft,
            Err(_) => continue, // skip entries with unreadable metadata
        };

        if file_type.is_dir() {
            let children = build_ai_file_tree(&entry.path(), &path);
            if !children.is_empty() {
                nodes.push(AiFileNode {
                    path,
                    name,
                    is_dir: true,
                    children,
                });
            }
        } else if name.ends_with(".md") {
            nodes.push(AiFileNode {
                path,
                name,
                is_dir: false,
                children: vec![],
            });
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

    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
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
        let prefix = docs_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("docs");
        let mut results = Vec::new();
        search_ai_dir_recursive(&docs_dir, prefix, &q.to_lowercase(), &mut results);
        // Sort by match_count descending
        results.sort_by_key(|r| std::cmp::Reverse(r.match_count));
        results
    })
    .await
    .unwrap_or_default();

    Json(ApiResponse::ok(result))
}

fn search_ai_dir_recursive(
    dir: &std::path::Path,
    rel_prefix: &str,
    query: &str,
    results: &mut Vec<AiSearchResult>,
) {
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
                    results.push(AiSearchResult {
                        path,
                        match_count: count,
                    });
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
        return Json(ApiResponse::err(
            "Invalid path: must be under docs/, doc/, ai/ or the root README, and not contain ..",
        ));
    }

    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
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
            Ok(content) => Ok(AiFileContent {
                path: file_path,
                content,
            }),
            Err(e) => Err(format!("Cannot read file: {}", e)),
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    match result {
        Ok(content) => Json(ApiResponse::ok(content)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Project source browser
// ═══════════════════════════════════════════════════════════════════════════════

/// Directories that are never useful in a source browser. Besides keeping the
/// tree fast on large projects, this prevents accidental traversal into VCS
/// internals, dependency caches and generated output.
fn is_skipped_source_dir(name: &str, is_root_child: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".git"
            | ".kronn"
            | ".idea"
            | ".cache"
            | ".next"
            | ".nuxt"
            | ".output"
            | ".pnpm-store"
            | ".turbo"
            | ".venv"
            | "cache"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "bundle"
            | "bundles"
            | "target"
            | "dist"
            | "build"
            | "coverage"
            | "tmp"
            | "temp"
            | "logs"
            | "__pycache__"
    ) || (is_root_child && matches!(lower.as_str(), "docs" | "doc" | "ai"))
}

fn is_sensitive_source_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.starts_with("secret.")
        || lower.starts_with("secrets.")
        || lower.starts_with("credential.")
        || lower.starts_with("credentials.")
        || matches!(
            lower.as_str(),
            ".npmrc"
                | ".pypirc"
                | ".netrc"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || matches!(
            std::path::Path::new(&lower)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some("pem" | "key" | "p12" | "pfx" | "jks" | "keystore")
        )
}

/// Extensions and bare names whose verdict is settled without opening the file.
///
/// KT-594 — deciding by content meant reading 8 KiB of every candidate: on this
/// repository that was 4.10 s of the 4.19 s a full listing took, against 0.05 s
/// for the directory walk itself. Almost everything in a source tree carries an
/// extension that answers the question outright, so the read is now a fallback
/// for the remainder rather than the rule.
///
/// `None` means "the name does not say", and only those files are opened.
fn source_text_by_name(name: &str) -> Option<bool> {
    let lower = name.to_ascii_lowercase();

    // Names that carry no extension, or whose whole name IS the extension.
    // `Path::extension` reads `.gitignore` as a stem, not a suffix.
    if matches!(
        lower.as_str(),
        "makefile"
            | "dockerfile"
            | "containerfile"
            | "rakefile"
            | "gemfile"
            | "brewfile"
            | "justfile"
            | "procfile"
            | "vagrantfile"
            | "jenkinsfile"
            | "cname"
            | "license"
            | "licence"
            | "readme"
            | "changelog"
            | "notice"
            | "authors"
            | "contributors"
            | "copying"
            | "codeowners"
            | ".gitignore"
            | ".gitattributes"
            | ".gitmodules"
            | ".mailmap"
            | ".editorconfig"
            | ".dockerignore"
            | ".npmignore"
            | ".eslintignore"
            | ".prettierignore"
            | ".nvmrc"
            | ".node-version"
            | ".python-version"
            | ".ruby-version"
            | ".tool-versions"
            | ".browserslistrc"
            | ".gitkeep"
    ) {
        return Some(true);
    }

    // macOS drops one of these in every directory it has ever displayed.
    if lower == ".ds_store" {
        return Some(false);
    }

    let extension = std::path::Path::new(&lower)
        .extension()
        .and_then(|ext| ext.to_str())?;

    if matches!(
        extension,
        "rs" | "toml" | "lock"
            | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
            | "json" | "jsonc" | "json5" | "map"
            | "css" | "scss" | "sass" | "less" | "styl"
            | "html" | "htm" | "xml" | "xhtml" | "xsl" | "xslt"
            | "svg" | "vue" | "svelte" | "astro"
            | "md" | "mdx" | "markdown" | "rst" | "adoc" | "txt" | "text"
            | "yml" | "yaml" | "ini" | "cfg" | "conf" | "properties" | "editorconfig"
            | "py" | "pyi" | "pyx" | "rb" | "go" | "java" | "kt" | "kts" | "scala"
            | "swift" | "m" | "mm" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh"
            | "cs" | "php" | "pl" | "pm" | "lua" | "r" | "jl" | "ex" | "exs"
            | "erl" | "hrl" | "clj" | "cljs" | "cljc" | "dart" | "hs" | "elm" | "nim"
            | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "bats"
            | "sql" | "graphql" | "gql" | "proto" | "thrift"
            | "tf" | "tfvars" | "hcl" | "nix" | "gradle" | "groovy" | "cmake" | "mk"
            | "patch" | "diff" | "csv" | "tsv" | "log" | "srt" | "vtt" | "po" | "pot"
    ) {
        return Some(true);
    }

    if matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "avif"
            | "ico" | "icns" | "heic" | "psd" | "pdf"
            | "zip" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tar" | "tgz"
            | "jar" | "war" | "ear" | "class" | "pyc" | "pyo"
            | "o" | "a" | "so" | "dylib" | "dll" | "exe" | "bin" | "wasm" | "node"
            | "db" | "sqlite" | "sqlite3" | "mdb" | "pack"
            | "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac"
            | "mp4" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv"
            | "woff" | "woff2" | "ttf" | "otf" | "eot"
    ) {
        return Some(false);
    }

    None
}

/// Whether a file belongs in the source tree, by name where the name suffices
/// and by content otherwise.
fn is_source_text_file(name: &str, path: &std::path::Path) -> bool {
    source_text_by_name(name).unwrap_or_else(|| is_probably_text_file(path))
}

fn is_probably_text_file(path: &std::path::Path) -> bool {
    use std::io::Read;

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut sample = Vec::with_capacity(8 * 1024);
    if file.take(8 * 1024).read_to_end(&mut sample).is_err() {
        return false;
    }
    !sample.contains(&0) && std::str::from_utf8(&sample).is_ok()
}

/// KT-605 — a directory the caller asked to expand. Stricter than the file
/// check below: there the last component is a file name, here it is a folder
/// and has to answer the same question as its ancestors. Without this, a
/// caller could ask for `node_modules` by name — the tree never offers it, so
/// a request for it is out of band.
fn safe_source_directory_path(path: &str) -> bool {
    if !safe_source_relative_path(path) {
        return false;
    }
    std::path::Path::new(path)
        .components()
        .enumerate()
        .all(|(index, component)| match component {
            std::path::Component::Normal(value) => value
                .to_str()
                .map(|name| !is_skipped_source_dir(name, index == 0))
                .unwrap_or(false),
            _ => false,
        })
}

fn safe_source_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let rel = std::path::Path::new(path);
    if rel
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }
    let parts: Vec<_> = rel.components().collect();
    if parts.len() > 1
        && parts[..parts.len() - 1]
            .iter()
            .enumerate()
            .any(|(index, component)| match component {
                std::path::Component::Normal(value) => value
                    .to_str()
                    .map(|name| is_skipped_source_dir(name, index == 0))
                    .unwrap_or(true),
                _ => true,
            })
    {
        return false;
    }
    rel.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| !is_sensitive_source_name(name))
}

/// Every directory the walk would enter, so git can be asked about them all at
/// once. Descendants of an already-excluded directory are not collected.
fn collect_source_dirs(
    dir: &std::path::Path,
    rel_prefix: &str,
    root: &std::path::Path,
    excluded_paths: &std::collections::HashSet<String>,
    remaining_depth: Option<usize>,
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.path().symlink_metadata() else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if is_skipped_source_dir(&name, dir == root) {
            continue;
        }
        let path = if rel_prefix.is_empty() {
            name
        } else {
            format!("{rel_prefix}/{name}")
        };
        if excluded_paths.contains(&path) {
            continue;
        }
        if remaining_depth != Some(0) {
            collect_source_dirs(
                &entry.path(),
                &path,
                root,
                excluded_paths,
                remaining_depth.map(|depth| depth.saturating_sub(1)),
                out,
            );
        }
        out.push(path);
    }
}

/// What the walk must not enter: the folders the human excluded, plus the ones
/// git already ignores.
///
/// KT-594 — `is_skipped_source_dir` is a hand-maintained guess at "machine
/// output", and it will always be one name behind. Here the name it lacked was
/// `frontend/.pnpm-store`: that one content-addressable store filled the
/// 10 000-file budget on its own, so the walk stopped before reaching `site/`
/// and those files were simply absent, with nothing in the answer to say why.
///
/// Git already knows what is generated. Asking it costs one process and one
/// pass over the DIRECTORIES — 0.19 s on this repository — and takes the tree
/// from over 10 000 files down to 2 088.
///
/// Ignored FILES inside a kept directory are untouched: they still appear,
/// still badged. It is entering an ignored directory that has no upside.
fn walk_exclusions(
    root: &std::path::Path,
    user_exclusions: Vec<String>,
) -> std::collections::HashSet<String> {
    walk_exclusions_under(root, root, "", None, user_exclusions)
}

/// The same, bounded to the subtree actually about to be listed.
///
/// KT-605 — listing one directory must not pay for the whole repository. With
/// `remaining_depth: Some(0)` this asks git about that directory's own children
/// and nothing else, which is exactly the set the answer can hide.
fn walk_exclusions_under(
    root: &std::path::Path,
    dir: &std::path::Path,
    rel_prefix: &str,
    remaining_depth: Option<usize>,
    user_exclusions: Vec<String>,
) -> std::collections::HashSet<String> {
    let mut excluded: std::collections::HashSet<String> = user_exclusions.into_iter().collect();
    let mut candidates = Vec::new();
    collect_source_dirs(dir, rel_prefix, root, &excluded, remaining_depth, &mut candidates);
    excluded.extend(git_ignored_paths(root, &candidates));
    excluded
}

fn git_ignored_paths(
    root: &std::path::Path,
    candidates: &[String],
) -> std::collections::HashSet<String> {
    use std::io::Write;
    use std::process::Stdio;

    if candidates.is_empty() {
        return std::collections::HashSet::new();
    }

    // `git ls-files --ignored` enumerates every ignored cache entry in the
    // repository (hundreds of thousands in some Symfony projects). Ask Git
    // only about the bounded set already admitted to the source tree instead.
    let child = crate::core::cmd::sync_cmd("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return std::collections::HashSet::new();
    };
    let Some(mut stdin) = child.stdin.take() else {
        return std::collections::HashSet::new();
    };
    let candidates = candidates.to_vec();
    let writer = std::thread::spawn(move || {
        candidates
            .into_iter()
            .all(|path| stdin.write_all(path.as_bytes()).is_ok() && stdin.write_all(&[0]).is_ok())
    });
    let Ok(output) = child.wait_with_output() else {
        return std::collections::HashSet::new();
    };
    if !writer.join().unwrap_or(false) {
        return std::collections::HashSet::new();
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect()
}

fn build_source_tree(
    dir: &std::path::Path,
    rel_prefix: &str,
    root: &std::path::Path,
    file_count: &mut usize,
    excluded_paths: &std::collections::HashSet<String>,
) -> Vec<SourceFileNode> {
    build_source_tree_with_depth(dir, rel_prefix, root, file_count, excluded_paths, None)
}

fn build_source_tree_with_depth(
    dir: &std::path::Path,
    rel_prefix: &str,
    root: &std::path::Path,
    file_count: &mut usize,
    excluded_paths: &std::collections::HashSet<String>,
    remaining_depth: Option<usize>,
) -> Vec<SourceFileNode> {
    if *file_count >= MAX_SOURCE_FILES {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by(|a, b| {
        let a_dir = a.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let b_dir = b.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a
                .file_name()
                .to_ascii_lowercase()
                .cmp(&b.file_name().to_ascii_lowercase()),
        }
    });

    let mut nodes = Vec::new();
    for entry in entries {
        if *file_count >= MAX_SOURCE_FILES {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let path = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let metadata = match std::fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        // Never follow symlinks: canonical containment protects reads, while
        // skipping links here keeps tree enumeration deterministic and cheap.
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if is_skipped_source_dir(&name, dir == root) || excluded_paths.contains(&path) {
                continue;
            }
            let children = if remaining_depth == Some(0) {
                Vec::new()
            } else {
                build_source_tree_with_depth(
                    &entry.path(),
                    &path,
                    root,
                    file_count,
                    excluded_paths,
                    remaining_depth.map(|depth| depth.saturating_sub(1)),
                )
            };
            if remaining_depth == Some(0) || !children.is_empty() {
                nodes.push(SourceFileNode {
                    path,
                    name,
                    is_dir: true,
                    children,
                    git_ignored: false,
                });
            }
        } else if metadata.is_file()
            && metadata.len() <= MAX_SOURCE_FILE_BYTES
            && !is_sensitive_source_name(&name)
            && is_source_text_file(&name, &entry.path())
        {
            *file_count += 1;
            nodes.push(SourceFileNode {
                path,
                name,
                is_dir: false,
                children: Vec::new(),
                git_ignored: false,
            });
        }
    }
    nodes
}

fn mark_git_ignored(
    nodes: &mut [SourceFileNode],
    ignored_paths: &std::collections::HashSet<String>,
) {
    for node in nodes {
        if node.is_dir {
            mark_git_ignored(&mut node.children, ignored_paths);
        } else {
            node.git_ignored = ignored_paths.contains(&node.path);
        }
    }
}

fn normalize_source_exclusion(path: &str) -> Option<String> {
    let cleaned = path.trim().trim_matches('/').replace('\\', "/");
    if cleaned.is_empty() || cleaned.len() > 500 {
        return None;
    }
    let mut parts = Vec::new();
    for component in std::path::Path::new(&cleaned).components() {
        let std::path::Component::Normal(value) = component else {
            return None;
        };
        parts.push(value.to_str()?.to_string());
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn flatten_source_paths(nodes: &[SourceFileNode], paths: &mut Vec<String>) {
    for node in nodes {
        if node.is_dir {
            flatten_source_paths(&node.children, paths);
        } else {
            paths.push(node.path.clone());
        }
    }
}

fn source_language(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if name == "dockerfile" || name == "makefile" {
        return Some("Shell");
    }
    let extension = std::path::Path::new(name).extension()?.to_str()?;
    match extension {
        "rs" => Some("Rust"),
        "ts" | "tsx" => Some("TypeScript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("JavaScript"),
        "php" => Some("PHP"),
        "py" | "pyw" => Some("Python"),
        "go" => Some("Go"),
        "java" => Some("Java"),
        "kt" | "kts" => Some("Kotlin"),
        "swift" => Some("Swift"),
        "c" | "h" => Some("C"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("C++"),
        "cs" => Some("C#"),
        "rb" => Some("Ruby"),
        "dart" => Some("Dart"),
        "scala" | "sc" => Some("Scala"),
        "vue" => Some("Vue"),
        "svelte" => Some("Svelte"),
        "css" | "scss" | "sass" | "less" => Some("CSS"),
        "html" | "htm" => Some("HTML"),
        "sh" | "bash" | "zsh" | "fish" => Some("Shell"),
        "sql" => Some("SQL"),
        "lua" => Some("Lua"),
        "ex" | "exs" => Some("Elixir"),
        "erl" | "hrl" => Some("Erlang"),
        "fs" | "fsx" | "fsi" => Some("F#"),
        "clj" | "cljs" | "cljc" => Some("Clojure"),
        "groovy" | "gradle" => Some("Groovy"),
        "r" => Some("R"),
        _ => None,
    }
}

/// Compute a bounded, GitHub-style language breakdown from the same source
/// universe as the Code tab. Dependency/cache folders, docs, sensitive files,
/// project exclusions and Git-ignored files do not influence the result.
pub(crate) fn compute_source_language_stats(
    root: &std::path::Path,
    exclusions: &[String],
) -> Vec<ProjectLanguageStat> {
    let mut file_count = 0;
    let excluded_paths = walk_exclusions(root, exclusions.to_vec());
    let tree = build_source_tree(root, "", root, &mut file_count, &excluded_paths);
    let mut paths = Vec::with_capacity(file_count);
    flatten_source_paths(&tree, &mut paths);
    let ignored_paths = git_ignored_paths(root, &paths);
    let mut totals = std::collections::HashMap::<&'static str, u64>::new();

    for path in paths {
        if ignored_paths.contains(&path) {
            continue;
        }
        let Some(language) = source_language(&path) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(root.join(&path)) else {
            continue;
        };
        *totals.entry(language).or_default() += metadata.len().max(1);
    }

    let mut stats: Vec<_> = totals
        .into_iter()
        .map(|(language, bytes)| ProjectLanguageStat {
            language: language.to_string(),
            bytes,
        })
        .collect();
    stats.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.language.cmp(&b.language))
    });
    stats
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct SourceFilesQuery {
    /// Return only the entries of one level, without their children.
    #[serde(default)]
    pub shallow: bool,
    /// KT-605 — the directory to list, relative to the project root. With it,
    /// opening a folder costs that folder rather than the whole repository,
    /// and `MAX_SOURCE_FILES` stops being a ceiling the answer can hit without
    /// being able to say so. Absent means the root.
    #[serde(default)]
    pub path: Option<String>,
}

/// GET /api/projects/:id/source-files
pub async fn list_source_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SourceFilesQuery>,
) -> Json<ApiResponse<Vec<SourceFileNode>>> {
    let project_and_exclusions = match state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &id)?;
            let exclusions = crate::db::projects::get_source_exclusions(conn, &id)?;
            Ok((project, exclusions))
        })
        .await
    {
        Ok((Some(project), exclusions)) => (project, exclusions),
        Ok((None, _)) => return Json(ApiResponse::err("Project not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let (project, exclusions) = project_and_exclusions;
    let project_path = project.path;
    // KT-605 — an empty `path` is the root, not a broken request: the frontend
    // sends the same shape for both.
    let requested = query.path.filter(|path| !path.is_empty());
    if let Some(path) = &requested {
        if !safe_source_directory_path(path) {
            return Json(ApiResponse::err("Invalid or unsupported source path"));
        }
    }
    let depth = if query.shallow { Some(0) } else { None };
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<SourceFileNode>, String> {
        let root = scanner::resolve_host_path(&project_path);
        let (dir, rel_prefix) = match &requested {
            Some(path) => (root.join(path), path.as_str()),
            None => (root.clone(), ""),
        };
        if !dir.is_dir() {
            return Err("Directory not found".to_string());
        }
        let mut file_count = 0;
        let excluded_paths = walk_exclusions_under(&root, &dir, rel_prefix, depth, exclusions);
        let mut tree = build_source_tree_with_depth(
            &dir,
            rel_prefix,
            &root,
            &mut file_count,
            &excluded_paths,
            depth,
        );
        let mut paths = Vec::with_capacity(file_count);
        flatten_source_paths(&tree, &mut paths);
        let ignored_paths = git_ignored_paths(&root, &paths);
        mark_git_ignored(&mut tree, &ignored_paths);
        Ok(tree)
    })
    .await
    .unwrap_or_else(|error| Err(format!("Source listing failed: {error}")));
    match result {
        Ok(tree) => Json(ApiResponse::ok(tree)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// GET /api/projects/:id/source-exclusions
pub async fn get_source_exclusions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<String>>> {
    match state
        .db
        .with_read_conn(move |conn| {
            if crate::db::projects::get_project(conn, &id)?.is_none() {
                return Ok(None);
            }
            Ok(Some(crate::db::projects::get_source_exclusions(conn, &id)?))
        })
        .await
    {
        Ok(Some(paths)) => Json(ApiResponse::ok(paths)),
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

/// PUT /api/projects/:id/source-exclusions
pub async fn set_source_exclusions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(paths): Json<Vec<String>>,
) -> Json<ApiResponse<Vec<String>>> {
    if paths.len() > MAX_SOURCE_EXCLUSIONS {
        return Json(ApiResponse::err(format!(
            "At most {MAX_SOURCE_EXCLUSIONS} source exclusions are allowed"
        )));
    }
    let mut normalized = std::collections::BTreeSet::new();
    for path in paths {
        let Some(path) = normalize_source_exclusion(&path) else {
            return Json(ApiResponse::err("Invalid source exclusion path"));
        };
        normalized.insert(path);
    }
    let normalized: Vec<_> = normalized.into_iter().collect();
    let id_for_write = id.clone();
    let values = normalized.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::replace_source_exclusions(conn, &id_for_write, &values)
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(normalized)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

/// GET /api/projects/:id/source-file?path=src/main.rs
pub async fn read_source_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiFileQuery>,
) -> Json<ApiResponse<AiFileContent>> {
    if !safe_source_relative_path(&query.path) {
        return Json(ApiResponse::err("Invalid or unsupported source path"));
    }
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let project_path = project.path;
    let relative = query.path;
    let result = tokio::task::spawn_blocking(move || -> Result<AiFileContent, String> {
        let root = scanner::resolve_host_path(&project_path);
        let canonical_root = root
            .canonicalize()
            .map_err(|error| format!("Cannot resolve project: {error}"))?;
        let full_path = root.join(&relative);
        if std::fs::symlink_metadata(&full_path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(true)
        {
            return Err("Source symlinks are not readable".into());
        }
        let canonical_file = full_path
            .canonicalize()
            .map_err(|error| format!("Cannot resolve file: {error}"))?;
        if !canonical_file.starts_with(&canonical_root) {
            return Err("Source path escapes the project".into());
        }
        let metadata = canonical_file
            .metadata()
            .map_err(|error| format!("Cannot inspect file: {error}"))?;
        if !metadata.is_file() || metadata.len() > MAX_SOURCE_FILE_BYTES {
            return Err("Source file is unavailable or too large".into());
        }
        let content = std::fs::read_to_string(canonical_file)
            .map_err(|error| format!("Cannot read source file: {error}"))?;
        Ok(AiFileContent {
            path: relative,
            content,
        })
    })
    .await
    .unwrap_or_else(|error| Err(format!("Task failed: {error}")));

    match result {
        Ok(content) => Json(ApiResponse::ok(content)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// GET /api/projects/:id/source-search?q=needle
pub async fn search_source_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AiSearchQuery>,
) -> Json<ApiResponse<Vec<AiSearchResult>>> {
    let needle = query.q.trim().to_lowercase();
    if needle.is_empty() {
        return Json(ApiResponse::ok(Vec::new()));
    }
    if needle.len() > 200 {
        return Json(ApiResponse::err("Search query is too long"));
    }
    let project_and_exclusions = match state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &id)?;
            let exclusions = crate::db::projects::get_source_exclusions(conn, &id)?;
            Ok((project, exclusions))
        })
        .await
    {
        Ok((Some(project), exclusions)) => (project, exclusions),
        Ok((None, _)) => return Json(ApiResponse::err("Project not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let (project, exclusions) = project_and_exclusions;
    let project_path = project.path;
    let results = tokio::task::spawn_blocking(move || {
        let root = scanner::resolve_host_path(&project_path);
        let mut file_count = 0;
        let excluded_paths = walk_exclusions(&root, exclusions);
        let tree = build_source_tree(&root, "", &root, &mut file_count, &excluded_paths);
        let mut paths = Vec::with_capacity(file_count);
        flatten_source_paths(&tree, &mut paths);
        let mut results = Vec::new();
        let mut scanned_bytes = 0u64;
        for path in paths {
            if results.len() >= MAX_SOURCE_SEARCH_RESULTS
                || scanned_bytes >= MAX_SOURCE_SEARCH_BYTES
            {
                break;
            }
            let Ok(metadata) = std::fs::metadata(root.join(&path)) else {
                continue;
            };
            scanned_bytes = scanned_bytes.saturating_add(metadata.len());
            let Ok(content) = std::fs::read_to_string(root.join(&path)) else {
                continue;
            };
            let lower = content.to_lowercase();
            let mut count = 0u32;
            let mut offset = 0;
            while let Some(index) = lower[offset..].find(&needle) {
                count = count.saturating_add(1);
                offset += index + needle.len();
            }
            if count > 0 {
                results.push(AiSearchResult {
                    path,
                    match_count: count,
                });
            }
        }
        results.sort_by_key(|result| std::cmp::Reverse(result.match_count));
        results
    })
    .await
    .unwrap_or_default();
    Json(ApiResponse::ok(results))
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
        return (
            StatusCode::BAD_REQUEST,
            "Invalid asset path: relative image paths only",
        )
            .into_response();
    }

    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
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

    /// KT-594 — the listing used to open and read 8 KiB of every candidate to
    /// decide whether it was text. The name settles almost all of them, and
    /// only what it cannot settle is worth an `open()`.
    #[test]
    fn the_name_settles_the_common_cases_without_opening_anything() {
        for name in ["main.rs", "Cargo.toml", "index.tsx", "README.md", "styles.scss"] {
            assert_eq!(source_text_by_name(name), Some(true), "{name}");
        }
        for name in ["logo.png", "icon.icns", "bundle.wasm", "archive.tar.gz", "font.woff2"] {
            assert_eq!(source_text_by_name(name), Some(false), "{name}");
        }
    }

    /// `Path::extension` reads `.gitignore` as a stem, so a suffix lookup alone
    /// would send every dotfile and every `Makefile` to the slow path — which is
    /// precisely the set a source tree is full of.
    #[test]
    fn a_name_without_a_suffix_is_still_answerable() {
        for name in ["Makefile", "Dockerfile", ".gitignore", ".editorconfig", "LICENSE"] {
            assert_eq!(source_text_by_name(name), Some(true), "{name}");
        }
        // macOS leaves one of these in every directory it has ever displayed.
        assert_eq!(source_text_by_name(".DS_Store"), Some(false));
    }

    /// Case is not a signal: `README.MD` is the same file as `readme.md`.
    #[test]
    fn the_verdict_ignores_case() {
        assert_eq!(source_text_by_name("README.MD"), Some(true));
        assert_eq!(source_text_by_name("LOGO.PNG"), Some(false));
        assert_eq!(source_text_by_name("MAKEFILE"), Some(true));
    }

    /// An unknown suffix must stay undecided rather than guess. Guessing "text"
    /// would put binaries in the tree; guessing "binary" would hide real files.
    #[test]
    fn an_unknown_name_falls_back_to_reading_the_file() {
        assert_eq!(source_text_by_name("payload.qzx"), None);
        assert_eq!(source_text_by_name("noextension"), None);

        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("payload.qzx");
        std::fs::write(&text, "des octets parfaitement lisibles").unwrap();
        assert!(is_source_text_file("payload.qzx", &text));

        let binary = dir.path().join("payload.qzy");
        std::fs::write(&binary, [0x00, 0x01, 0x02, 0x00]).unwrap();
        assert!(!is_source_text_file("payload.qzy", &binary));
    }

    /// Some suffixes only look binary. EPS is PostScript, which is usually
    /// text, and a VobSub `.idx` is a plain index. Refusing them by name would
    /// have hidden real files; they cost one `open()` each and stay visible.
    #[test]
    fn a_suffix_that_only_looks_binary_is_left_to_the_bytes() {
        assert_eq!(source_text_by_name("figure.eps"), None);
        assert_eq!(source_text_by_name("subtitles.idx"), None);

        let dir = tempfile::tempdir().unwrap();
        let eps = dir.path().join("figure.eps");
        std::fs::write(&eps, "%!PS-Adobe-3.0 EPSF-3.0\n").unwrap();
        assert!(is_source_text_file("figure.eps", &eps));
    }

    /// The fallback must not be consulted when the name already answered: a
    /// `.png` is excluded without the read, even if its bytes happen to be text.
    #[test]
    fn a_known_name_wins_over_what_the_bytes_say() {
        let dir = tempfile::tempdir().unwrap();
        let misnamed = dir.path().join("not-really.png");
        std::fs::write(&misnamed, "je suis du texte").unwrap();
        assert!(!is_source_text_file("not-really.png", &misnamed));
    }

    // 0.8.3 UX regression — verrouille l'ordre dirs-first puis files.
    // Avant : tri alphabétique pur mélangeait `architecture/`,
    // `briefing.md`, `coding-rules.md`, `operations/`. Convention
    // file-explorer attendue : dossiers groupés en haut.
    fn touch(p: &std::path::Path) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
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
            vec![
                "architecture",
                "operations",
                "AGENTS.md",
                "briefing.md",
                "coding-rules.md"
            ],
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
        assert_eq!(
            find_root_readme(tmp.path()),
            Some("ReAdMe.markdown".to_string())
        );
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

    // ── Source browser ───────────────────────────────────────────────────
    #[test]
    fn source_tree_keeps_code_and_excludes_docs_dependencies_and_secrets() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("src/main.rs"));
        touch(&root.join("frontend/App.tsx"));
        touch(&root.join(".github/workflows/ci.yml"));
        touch(&root.join("README.md"));
        touch(&root.join("application/.htaccess"));
        touch(&root.join("application/config/custom.rules"));
        touch(&root.join("docs/AGENTS.md"));
        touch(&root.join("node_modules/pkg/index.js"));
        touch(&root.join("target/debug/generated.rs"));
        touch(&root.join("var/cache/prod/container.php"));
        touch(&root.join("bundles/app.min.js"));
        touch(&root.join(".env"));
        touch(&root.join("server.pem"));
        std::fs::write(root.join("image.png"), [0, 159, 146, 150]).unwrap();

        let mut count = 0;
        let tree = build_source_tree(
            root,
            "",
            root,
            &mut count,
            &std::collections::HashSet::new(),
        );
        let mut paths = Vec::new();
        flatten_source_paths(&tree, &mut paths);

        assert_eq!(count, 6);
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"frontend/App.tsx".to_string()));
        assert!(paths.contains(&".github/workflows/ci.yml".to_string()));
        assert!(paths.contains(&"README.md".to_string()));
        assert!(paths.contains(&"application/.htaccess".to_string()));
        assert!(paths.contains(&"application/config/custom.rules".to_string()));
        assert!(!paths.iter().any(|path| path.starts_with("docs/")));
        assert!(!paths.iter().any(|path| path.starts_with("node_modules/")));
        assert!(!paths.iter().any(|path| path.starts_with("target/")));
        assert!(!paths.iter().any(|path| path.starts_with("var/cache/")));
        assert!(!paths.iter().any(|path| path.starts_with("bundles/")));
        assert!(!paths.iter().any(|path| path.contains(".env")));
        assert!(!paths.iter().any(|path| path.ends_with(".pem")));
    }

    #[test]
    fn source_read_guard_rejects_traversal_docs_and_sensitive_files() {
        for allowed in [
            "src/main.rs",
            "frontend/src/App.tsx",
            ".github/workflows/ci.yml",
            "Cargo.toml",
            "package.json",
            "README.md",
            "application/.htaccess",
            "application/config/custom.rules",
        ] {
            assert!(
                safe_source_relative_path(allowed),
                "{allowed} should be readable"
            );
        }
        for rejected in [
            "",
            "../src/main.rs",
            "/etc/passwd",
            "src/../../.env",
            "docs/AGENTS.md",
            "doc/index.md",
            "ai/index.md",
            "node_modules/pkg/index.js",
            "target/generated.rs",
            ".git/hooks/pre-commit.js",
            ".env",
            ".env.local",
            ".npmrc",
            "config/secrets.yml",
            "config/credentials.json",
            "private.key",
        ] {
            assert!(
                !safe_source_relative_path(rejected),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn source_tree_respects_global_file_cap() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("src/a.rs"));
        touch(&tmp.path().join("src/b.rs"));
        let mut count = MAX_SOURCE_FILES - 1;
        let tree = build_source_tree(
            tmp.path(),
            "",
            tmp.path(),
            &mut count,
            &std::collections::HashSet::new(),
        );
        let mut paths = Vec::new();
        flatten_source_paths(&tree, &mut paths);
        assert_eq!(count, MAX_SOURCE_FILES);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn source_tree_skips_nested_pnpm_store_before_applying_file_cap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        touch(&root.join("frontend/.pnpm-store/v11/files/29/cached-file.js"));
        touch(&root.join("site/en.html"));
        let mut count = MAX_SOURCE_FILES - 1;

        let tree = build_source_tree(
            root,
            "",
            root,
            &mut count,
            &std::collections::HashSet::new(),
        );
        let mut paths = Vec::new();
        flatten_source_paths(&tree, &mut paths);

        assert_eq!(count, MAX_SOURCE_FILES);
        assert_eq!(paths, vec!["site/en.html"]);
    }

    #[test]
    fn shallow_source_tree_returns_root_entries_without_descending() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("src/main.rs"));
        touch(&tmp.path().join("README.md"));
        let mut count = 0;

        let tree = build_source_tree_with_depth(
            tmp.path(),
            "",
            tmp.path(),
            &mut count,
            &std::collections::HashSet::new(),
            Some(0),
        );

        let src = tree.iter().find(|node| node.path == "src").unwrap();
        assert!(src.is_dir);
        assert!(src.children.is_empty());
        assert!(tree.iter().any(|node| node.path == "README.md"));
        assert_eq!(count, 1);
    }

    #[test]
    fn git_ignore_lookup_handles_more_output_than_a_pipe_buffer() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "ignored/\n").unwrap();
        let candidates: Vec<_> = (0..5_000)
            .map(|index| format!("ignored/generated-source-file-{index:05}.typescript"))
            .collect();

        let ignored = git_ignored_paths(tmp.path(), &candidates);

        assert_eq!(ignored.len(), candidates.len());
    }

    #[test]
    fn source_tree_respects_project_folder_exclusions() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("src/main.rs"));
        touch(&tmp.path().join("generated/client/api.ts"));
        let excluded = std::collections::HashSet::from(["generated/client".to_string()]);
        let mut count = 0;
        let tree = build_source_tree(tmp.path(), "", tmp.path(), &mut count, &excluded);
        let mut paths = Vec::new();
        flatten_source_paths(&tree, &mut paths);

        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn language_stats_reuse_source_exclusions_and_ignore_gitignored_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("src/App.tsx"), "export default 1;\n").unwrap();
        std::fs::write(root.join("generated/client.ts"), "x".repeat(2_000)).unwrap();
        std::fs::write(root.join("ignored/cache.js"), "x".repeat(3_000)).unwrap();

        let stats = compute_source_language_stats(root, &["generated".into()]);

        assert_eq!(
            stats
                .iter()
                .map(|stat| stat.language.as_str())
                .collect::<Vec<_>>(),
            vec!["TypeScript", "Rust"]
        );
        assert!(stats.iter().all(|stat| stat.bytes > 0));
    }

    #[test]
    fn source_exclusion_paths_are_normalized_and_traversal_safe() {
        assert_eq!(
            normalize_source_exclusion(" /generated/client/ "),
            Some("generated/client".into())
        );
        assert_eq!(
            normalize_source_exclusion(r"var\cache"),
            Some("var/cache".into())
        );
        assert_eq!(
            normalize_source_exclusion("generated//client"),
            Some("generated/client".into())
        );
        assert!(normalize_source_exclusion("../outside").is_none());
        assert!(normalize_source_exclusion("/").is_none());
    }

    #[test]
    fn source_tree_marks_gitignored_text_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "application/local.rules\n").unwrap();
        touch(&tmp.path().join("application/local.rules"));
        touch(&tmp.path().join("application/tracked.rules"));

        let mut count = 0;
        let mut tree = build_source_tree(
            tmp.path(),
            "",
            tmp.path(),
            &mut count,
            &std::collections::HashSet::new(),
        );
        let mut paths = Vec::new();
        flatten_source_paths(&tree, &mut paths);
        let ignored = git_ignored_paths(tmp.path(), &paths);
        mark_git_ignored(&mut tree, &ignored);
        let application = tree.iter().find(|node| node.path == "application").unwrap();
        let local = application
            .children
            .iter()
            .find(|node| node.name == "local.rules")
            .unwrap();
        let tracked = application
            .children
            .iter()
            .find(|node| node.name == "tracked.rules")
            .unwrap();

        assert!(local.git_ignored);
        assert!(!tracked.git_ignored);
    }

    /// KT-594 — the bug @user hit: `frontend/.pnpm-store` is not in the name
    /// deny-list, and its content-addressable store filled the 10 000-file
    /// budget on its own. The walk stopped before `site/`, and those files were
    /// absent from the tree with nothing in the answer to say why.
    ///
    /// The deny-list will always be one name behind. Git already knows.
    #[test]
    fn a_git_ignored_folder_is_never_walked_however_it_is_named() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".pnpm-store/\n").unwrap();
        touch(&tmp.path().join(".pnpm-store/v11/deadbeef.rs"));
        // Alphabetically after the store, which is exactly what got lost.
        touch(&tmp.path().join("site/en.html"));

        let excluded = walk_exclusions(tmp.path(), Vec::new());
        let mut count = 0;
        let tree = build_source_tree(tmp.path(), "", tmp.path(), &mut count, &excluded);

        assert!(tree.iter().any(|node| node.path == "site"), "site/ must be reachable");
        assert!(
            !tree.iter().any(|node| node.path == ".pnpm-store"),
            "an ignored folder has no business in a source tree",
        );
    }

    /// Only FOLDERS. An ignored file inside a kept folder stays visible, badged
    /// as before — hiding it would remove something the author can see today.
    #[test]
    fn an_ignored_file_in_a_kept_folder_is_still_listed() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "application/local.rules\n").unwrap();
        touch(&tmp.path().join("application/local.rules"));
        touch(&tmp.path().join("application/tracked.rules"));

        let excluded = walk_exclusions(tmp.path(), Vec::new());
        let mut count = 0;
        let tree = build_source_tree(tmp.path(), "", tmp.path(), &mut count, &excluded);
        let application = tree.iter().find(|node| node.path == "application").unwrap();

        assert!(application.children.iter().any(|node| node.name == "local.rules"));
        assert!(application.children.iter().any(|node| node.name == "tracked.rules"));
    }

    /// A folder the human excluded by hand still wins, and its subtree is not
    /// even offered to git — there is nothing to ask about.
    #[test]
    fn a_hand_excluded_folder_stays_excluded() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("keep/main.rs"));
        touch(&tmp.path().join("drop/main.rs"));

        let excluded = walk_exclusions(tmp.path(), vec!["drop".to_string()]);
        let mut count = 0;
        let tree = build_source_tree(tmp.path(), "", tmp.path(), &mut count, &excluded);

        assert!(tree.iter().any(|node| node.path == "keep"));
        assert!(!tree.iter().any(|node| node.path == "drop"));
    }

    /// KT-605 — listing one directory must cost that directory. The whole point
    /// of the change is that opening a folder no longer walks the repository,
    /// so the assertion is on what it collects, not just on what it returns.
    #[test]
    fn listing_one_directory_returns_its_children_and_looks_no_further() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(&tmp.path().join("site/en.html"));
        touch(&tmp.path().join("site/assets/logo.svg"));
        touch(&tmp.path().join("backend/src/main.rs"));

        let excluded = walk_exclusions_under(tmp.path(), &tmp.path().join("site"), "site", Some(0), Vec::new());
        let mut count = 0;
        let tree = build_source_tree_with_depth(
            &tmp.path().join("site"),
            "site",
            tmp.path(),
            &mut count,
            &excluded,
            Some(0),
        );

        let names: Vec<_> = tree.iter().map(|node| node.path.as_str()).collect();
        assert_eq!(names, vec!["site/assets", "site/en.html"]);
        // The child folder is announced but not expanded — that is the next
        // request, made when someone opens it.
        let assets = tree.iter().find(|node| node.path == "site/assets").unwrap();
        assert!(assets.is_dir);
        assert!(assets.children.is_empty());
        // And nothing from the sibling subtree was even considered.
        assert!(!names.iter().any(|path| path.starts_with("backend")));
    }

    /// A folder path is checked on every component, unlike a file path whose
    /// last component is a name. Without it a caller could ask for a folder the
    /// tree never offers.
    #[test]
    fn a_directory_outside_the_tree_is_refused() {
        assert!(safe_source_directory_path("site"));
        assert!(safe_source_directory_path("site/assets"));

        assert!(!safe_source_directory_path("../etc"));
        assert!(!safe_source_directory_path("/etc"));
        assert!(!safe_source_directory_path("site/../../etc"));
        assert!(!safe_source_directory_path(""));
        // Named directly rather than reached through the tree, which never
        // offers them.
        assert!(!safe_source_directory_path("node_modules"));
        assert!(!safe_source_directory_path("frontend/node_modules"));
        assert!(!safe_source_directory_path("docs"));
        // `docs` is only skipped at the root; deeper it is ordinary source.
        assert!(safe_source_directory_path("backend/docs"));
    }

    /// The exclusions of a subtree are the subtree's own. Asking git about the
    /// whole repository to open one folder would put back the cost the change
    /// exists to remove.
    #[test]
    fn a_subtree_listing_hides_what_git_ignores_inside_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "site/generated/\n").unwrap();
        touch(&tmp.path().join("site/en.html"));
        touch(&tmp.path().join("site/generated/bundle.js"));

        let dir = tmp.path().join("site");
        let excluded = walk_exclusions_under(tmp.path(), &dir, "site", Some(0), Vec::new());
        let mut count = 0;
        let tree = build_source_tree_with_depth(&dir, "site", tmp.path(), &mut count, &excluded, Some(0));

        let names: Vec<_> = tree.iter().map(|node| node.path.as_str()).collect();
        assert_eq!(names, vec!["site/en.html"]);
    }

    // ── 0.8.6 — doc-asset image serving (relative <img> in README/docs) ───
    #[test]
    fn doc_asset_serves_image_extensions_only() {
        for ok in [
            "docs/screenshots/foo.png",
            "logo.svg",
            "a/b/c.jpeg",
            "x.WEBP",
            "i.GIF",
        ] {
            assert!(is_servable_asset_path(ok), "{ok} should be servable");
        }
        for bad in [
            "docs/notes.md",
            ".env",
            "Cargo.toml",
            "src/main.rs",
            "foo",
            "a/b.txt",
        ] {
            assert!(
                !is_servable_asset_path(bad),
                "{bad} must NOT be servable (non-image)"
            );
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
