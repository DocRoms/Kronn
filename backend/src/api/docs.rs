//! Document-generation API — proxies to the kronn-docs Python sidecar.
//!
//! Phase 0 ships only `/pdf`. The other four formats (docx/xlsx/csv/pptx)
//! are wired the same way in phase 1 — each handler is a few lines of
//! JSON passthrough + file path accounting.
//!
//! Security model
//! --------------
//! * Files always land under `~/.kronn/generated/<discussion_id>/` with
//!   a timestamped filename. Callers can't specify the target path —
//!   we pick it so a malicious prompt cannot trick the sidecar into
//!   overwriting arbitrary files.
//! * The sidecar binds to loopback only and receives its port through
//!   an env var, not through caller input. The proxy never forwards
//!   user-controlled URLs.
//! * Output paths are returned to the frontend; the frontend fetches
//!   them via `GET /api/docs/file/<disc_id>/<filename>` which enforces
//!   "file must live under the expected generated dir" (no path
//!   traversal).

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::models::ApiResponse;
use crate::AppState;

/// Shared PDF request body coming from the frontend. The `output_path`
/// we forward to the sidecar is computed on our side — the frontend
/// only tells us the discussion and the content.
#[derive(Debug, Deserialize)]
pub struct PdfRequest {
    /// Discussion the generated file belongs to. Used to decide the
    /// output directory so files stay discoverable per-discussion.
    pub discussion_id: String,
    /// Full HTML document as produced by the agent (including `<style>`).
    pub html: String,
    /// Optional filename hint — sanitized before use. When absent we
    /// generate `kronn-doc-<timestamp>.pdf`.
    #[serde(default)]
    pub filename: Option<String>,
    /// Optional CSS `@page size` override (e.g. "A4", "Letter",
    /// "210mm 297mm"). Passed verbatim to the sidecar.
    #[serde(default)]
    pub page_size: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GeneratedFile {
    /// Absolute path of the generated file on disk. Kept for debug +
    /// future "open in Files" button; the UI uses `download_url`.
    pub path: String,
    /// URL the frontend can GET to stream the file back. Formed as
    /// `/api/docs/file/<discussion_id>/<filename>`.
    pub download_url: String,
    /// File size in bytes — surfaced in the download chip UI.
    pub size_bytes: u64,
}

/// POST /api/docs/pdf — HTML → PDF.
pub async fn generate_pdf(
    State(state): State<AppState>,
    Json(req): Json<PdfRequest>,
) -> Json<ApiResponse<GeneratedFile>> {
    proxy_to_sidecar(
        &state,
        &req.discussion_id,
        req.filename.as_deref(),
        "pdf",
        "pdf",
        serde_json::json!({
            "html": req.html,
            "page_size": req.page_size,
        }),
    ).await
}

// ─── DOCX — HTML input (same shape as PDF) ────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DocxRequest {
    pub discussion_id: String,
    pub html: String,
    #[serde(default)]
    pub filename: Option<String>,
}

/// POST /api/docs/docx — HTML → DOCX. Same HTML input as `/pdf` — the
/// user chooses the output format at export time without the agent
/// having to regenerate the content.
pub async fn generate_docx(
    State(state): State<AppState>,
    Json(req): Json<DocxRequest>,
) -> Json<ApiResponse<GeneratedFile>> {
    proxy_to_sidecar(
        &state,
        &req.discussion_id,
        req.filename.as_deref(),
        "docx",
        "docx",
        serde_json::json!({ "html": req.html }),
    ).await
}

// ─── XLSX — structured JSON (sheets × rows) ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct XlsxRequest {
    pub discussion_id: String,
    /// `[{name, rows: [[cell, cell, ...], ...]}, ...]` — passed through
    /// untouched to the sidecar. Cell values can be any JSON scalar;
    /// XlsxWriter coerces int/float/string/bool to the matching Excel
    /// type. Null → empty cell.
    pub sheets: Value,
    #[serde(default)]
    pub filename: Option<String>,
}

/// POST /api/docs/xlsx — Multi-sheet Excel from structured data.
pub async fn generate_xlsx(
    State(state): State<AppState>,
    Json(req): Json<XlsxRequest>,
) -> Json<ApiResponse<GeneratedFile>> {
    proxy_to_sidecar(
        &state,
        &req.discussion_id,
        req.filename.as_deref(),
        "xlsx",
        "xlsx",
        serde_json::json!({ "sheets": req.sheets }),
    ).await
}

// ─── CSV — flat row dump ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CsvRequest {
    pub discussion_id: String,
    pub rows: Value,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
}

/// POST /api/docs/csv — Flat CSV from a 2-D JSON array.
pub async fn generate_csv(
    State(state): State<AppState>,
    Json(req): Json<CsvRequest>,
) -> Json<ApiResponse<GeneratedFile>> {
    proxy_to_sidecar(
        &state,
        &req.discussion_id,
        req.filename.as_deref(),
        "csv",
        "csv",
        serde_json::json!({
            "rows": req.rows,
            "delimiter": req.delimiter,
        }),
    ).await
}

// ─── PPTX — slide deck ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PptxRequest {
    pub discussion_id: String,
    /// `[{title, bullets: [...], content?: "..."}, ...]` — one entry
    /// per slide. `content` is split on newlines as a fallback when
    /// `bullets` is absent.
    pub slides: Value,
    #[serde(default)]
    pub filename: Option<String>,
}

/// POST /api/docs/pptx — PowerPoint deck from a structured slide list.
pub async fn generate_pptx(
    State(state): State<AppState>,
    Json(req): Json<PptxRequest>,
) -> Json<ApiResponse<GeneratedFile>> {
    proxy_to_sidecar(
        &state,
        &req.discussion_id,
        req.filename.as_deref(),
        "pptx",
        "pptx",
        serde_json::json!({ "slides": req.slides }),
    ).await
}

// ─── Shared proxy helper ──────────────────────────────────────────────

/// Single implementation for all 5 format endpoints. Guards against
/// missing sidecar, builds the output path, proxies the caller-provided
/// payload (merged with `output_path` which we pick server-side), and
/// validates the resulting file's existence + size.
// Parameters:
//   `route`     — sidecar path suffix (e.g. "pdf", "docx"), appended
//                 to the sidecar's base_url.
//   `extension` — file extension written to disk; usually equals
//                 `route` but kept separate so we could, hypothetically,
//                 route different payloads to the same endpoint.
//   `payload`   — format-specific body; we inject `output_path` into
//                 it and forward as JSON to the sidecar.
async fn proxy_to_sidecar(
    state: &AppState,
    discussion_id: &str,
    filename_hint: Option<&str>,
    route: &str,
    extension: &str,
    mut payload: Value,
) -> Json<ApiResponse<GeneratedFile>> {
    let sidecar = match state.docs_sidecar.handle().await {
        Some(h) => h,
        None => {
            return Json(ApiResponse::err(
                "Document sidecar unavailable. Run `make docs-setup` to install it, then restart Kronn.".to_string(),
            ));
        }
    };

    let (output_path, filename) = match build_output_path(discussion_id, filename_hint, extension) {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Inject the server-chosen output_path into the caller's payload —
    // the sidecar writes the file for us.
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "output_path".into(),
            Value::String(output_path.to_string_lossy().into_owned()),
        );
    }

    let client = reqwest::Client::new();
    let url = format!("{}/{}", sidecar.base_url, route);
    let resp = match client.post(&url).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            return Json(ApiResponse::err(format!(
                "Sidecar request failed: {e}. Is the kronn-docs sidecar running? See backend/sidecars/docs/README.md"
            )));
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Json(ApiResponse::err(format!(
            "Sidecar returned {status}: {body}"
        )));
    }

    let size_bytes = match std::fs::metadata(&output_path) {
        Ok(m) => m.len(),
        Err(e) => {
            return Json(ApiResponse::err(format!(
                "Sidecar reported success but output file is missing: {e}"
            )));
        }
    };

    Json(ApiResponse::ok(GeneratedFile {
        path: output_path.to_string_lossy().into_owned(),
        download_url: format!("/api/docs/file/{}/{}", discussion_id, filename),
        size_bytes,
    }))
}

/// GET /api/docs/file/:discussion_id/:filename — streams back a
/// previously-generated file. Enforces path-traversal safety and the
/// "files must live under the discussion's generated dir" contract.
pub async fn download_file(
    AxumPath((discussion_id, filename)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    // Reject obvious traversal attempts early; in-depth validation
    // follows via canonicalize() below.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid filename".to_string()).into_response();
    }
    if discussion_id.contains('/') || discussion_id.contains('\\') || discussion_id.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid discussion id".to_string()).into_response();
    }

    let root = match generated_root() {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let path = root.join(&discussion_id).join(&filename);

    // Resolve symlinks and compare against the root to defeat any path
    // traversal that slipped past the string checks above. If the file
    // doesn't exist canonicalize() fails, which is fine — we turn that
    // into a 404.
    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found".to_string()).into_response(),
    };
    let root_canonical = match root.canonicalize() {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if !resolved.starts_with(&root_canonical) {
        return (StatusCode::FORBIDDEN, "path escapes generated root".to_string()).into_response();
    }

    let bytes = match tokio::fs::read(&resolved).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let mime = guess_mime(&filename);
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response()
}

// ─── helpers ───────────────────────────────────────────────────────────────

/// `~/.kronn/generated` — root dir for every agent-generated file.
pub fn generated_root() -> Result<PathBuf, String> {
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or_else(|| "home dir unknown".to_string())?;
    Ok(home.join(".kronn").join("generated"))
}

/// Build `~/.kronn/generated/<disc_id>/<filename>` — and `mkdir -p` the
/// parent. Filename is derived from the caller hint when valid, else a
/// timestamp-based default.
pub(super) fn build_output_path(
    discussion_id: &str,
    filename_hint: Option<&str>,
    extension: &str,
) -> Result<(PathBuf, String), String> {
    if discussion_id.is_empty() || discussion_id.contains('/') || discussion_id.contains('\\') {
        return Err("invalid discussion_id".into());
    }
    let root = generated_root()?;
    let dir = root.join(discussion_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let filename = match filename_hint {
        Some(h) if !h.is_empty() => sanitize_filename(h, extension),
        _ => default_filename(extension),
    };
    Ok((dir.join(&filename), filename))
}

/// Collapse a user-supplied hint into a safe filename. Alphanumerics,
/// `-`, `_` and space are kept; everything else (including `.`) is
/// replaced with `-`. Dots are dropped on purpose — leaving them in
/// breeds `..` sequences from traversal payloads, which look bad even if
/// they're not functionally exploitable (the path-join + canonicalize
/// check in `download_file` is the real defense). A UUID suffix prevents
/// collisions if the agent picks the same hint twice; the extension is
/// always appended fresh at the end.
fn sanitize_filename(hint: &str, extension: &str) -> String {
    let mut clean = String::with_capacity(hint.len());
    for c in hint.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
            clean.push(c);
        } else {
            clean.push('-');
        }
    }
    // Collapse runs of `-` and trim edges so we don't end up with stuff
    // like `---etc---passwd---` from a traversal-looking hint.
    let collapsed: String = {
        let mut out = String::with_capacity(clean.len());
        let mut last_dash = false;
        for c in clean.chars() {
            if c == '-' {
                if !last_dash {
                    out.push(c);
                }
                last_dash = true;
            } else {
                out.push(c);
                last_dash = false;
            }
        }
        out.trim_matches('-').trim().to_string()
    };
    let base = if collapsed.is_empty() { "kronn-doc".to_string() } else { collapsed };
    let suffix = &Uuid::new_v4().to_string()[..8];
    format!("{base}-{suffix}.{extension}")
}

/// `kronn-doc-2026-04-22T14-03-18.pdf` — used when the caller gave no
/// filename hint.
fn default_filename(extension: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S");
    let suffix = &Uuid::new_v4().to_string()[..8];
    format!("kronn-doc-{now}-{suffix}.{extension}")
}

/// Best-guess content type from the filename extension. Used for the
/// download handler's Content-Type header so browsers know what to do
/// with the file.
fn guess_mime(filename: &str) -> &'static str {
    let ext = Path::new(filename).extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_path_chars_and_forces_extension() {
        let got = sanitize_filename("../../etc/passwd", "pdf");
        // No dots in the base (they get replaced) → no "../" sequences
        // survive. Slashes become `-`, dashes get collapsed/trimmed.
        assert!(!got.contains('/'), "no slashes allowed: {got}");
        assert!(!got.contains(".."), "no dot sequences: {got}");
        assert!(got.ends_with(".pdf"), "extension must be pdf: {got}");
        assert!(got.starts_with("etc-passwd"), "payload stripped to safe core: {got}");
    }

    #[test]
    fn sanitize_filename_handles_empty_and_dot_only() {
        let got = sanitize_filename("", "pdf");
        assert!(got.starts_with("kronn-doc-"), "empty must fall back: {got}");
        let got = sanitize_filename(".hidden.file.pdf", "pdf");
        assert!(got.ends_with(".pdf"));
    }

    #[test]
    fn sanitize_filename_collapses_runs_of_dashes() {
        // "foo///bar" → "foo---bar" after char-by-char, then collapsed → "foo-bar"
        let got = sanitize_filename("foo///bar", "pdf");
        assert!(got.starts_with("foo-bar-"), "collapsed: {got}");
        assert!(!got.contains("---"));
    }

    #[test]
    fn default_filename_has_timestamp_and_extension() {
        let got = default_filename("xlsx");
        assert!(got.starts_with("kronn-doc-2026-") || got.starts_with("kronn-doc-20"), "timestamp prefix: {got}");
        assert!(got.ends_with(".xlsx"));
    }

    #[test]
    fn build_output_path_rejects_traversal_in_disc_id() {
        assert!(build_output_path("../../../etc", None, "pdf").is_err());
        assert!(build_output_path("abc/def", None, "pdf").is_err());
        assert!(build_output_path("", None, "pdf").is_err());
    }

    #[test]
    fn guess_mime_covers_five_formats() {
        assert_eq!(guess_mime("x.pdf"), "application/pdf");
        assert_eq!(guess_mime("x.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        assert_eq!(guess_mime("x.xlsx"), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        assert_eq!(guess_mime("x.pptx"), "application/vnd.openxmlformats-officedocument.presentationml.presentation");
        assert_eq!(guess_mime("x.csv"), "text/csv");
        assert_eq!(guess_mime("x.unknown"), "application/octet-stream");
    }
}
