// Existing documentation in the briefing (KT-999): the locations the user
// keeps, their state and at most one source of truth per kind, recorded in
// `docs/briefing.md` like the project facts (readable lines + machine copy).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::core::briefing_docs as detect;
use crate::models::{
    ApiResponse, BriefingDetectedDoc, BriefingDocKind, BriefingDocState, BriefingDocsAnswers,
    CheckBriefingDocRequest,
};
use crate::AppState;

use super::project_doc::{cell, check_text};

pub(crate) const DOCS_HEADING: &str = "## Existing documentation";
pub(crate) const DOCS_ANSWERS_PREFIX: &str = "<!-- kronn:existing-docs v1 ";
const DOCS_ANSWERS_SUFFIX: &str = " -->";
pub(crate) const NOTE_MAX_CHARS: usize = 200;

fn kind_label(kind: BriefingDocKind) -> &'static str {
    match kind {
        BriefingDocKind::Readme => "README",
        BriefingDocKind::Docs => "documentation folder",
        BriefingDocKind::Wiki => "wiki",
        BriefingDocKind::Adr => "decision records (ADR)",
        BriefingDocKind::Api => "API reference",
        BriefingDocKind::Contributing => "contributing guide",
        BriefingDocKind::Markdown => "Markdown page",
    }
}

fn state_label(state: BriefingDocState) -> &'static str {
    match state {
        BriefingDocState::Unknown => "state not provided",
        BriefingDocState::Recent => "recent",
        BriefingDocState::Old => "old",
        BriefingDocState::Obsolete => "obsolete",
        BriefingDocState::Partial => "partial",
        BriefingDocState::Reference => "reference",
    }
}

/// Validate the answers against the repository: each path is re-normalised
/// and re-classified, duplicates keep their first entry, and a kind has at
/// most one source of truth, which cannot be obsolete. Dates are left empty.
pub(crate) fn normalize(
    root: &std::path::Path,
    answers: &BriefingDocsAnswers,
) -> Result<BriefingDocsAnswers, String> {
    if answers.locations.len() > detect::MAX_LOCATIONS {
        return Err(format!(
            "existing_docs: at most {} locations",
            detect::MAX_LOCATIONS
        ));
    }
    let mut out = BriefingDocsAnswers::default();
    for location in &answers.locations {
        let (path, is_dir) = detect::normalize_path(root, &location.path)?;
        if out.locations.iter().any(|l| l.path == path) {
            continue;
        }
        let note = location.note.trim().to_string();
        check_text("existing_docs.note", &note, NOTE_MAX_CHARS)?;
        let mut entry = location.clone();
        entry.kind = detect::classify(&path, is_dir);
        entry.path = path;
        entry.note = note;
        entry.last_commit = None;
        if entry.source_of_truth && entry.state == BriefingDocState::Obsolete {
            return Err(format!(
                "existing_docs: `{}` cannot be both obsolete and the source of truth",
                entry.path
            ));
        }
        if entry.source_of_truth {
            if let Some(other) = out
                .locations
                .iter()
                .find(|l| l.source_of_truth && l.kind == entry.kind)
            {
                return Err(format!(
                    "existing_docs: one source of truth per kind — `{}` and `{}` are both a {}",
                    other.path,
                    entry.path,
                    kind_label(entry.kind)
                ));
            }
        }
        out.locations.push(entry);
    }
    Ok(out)
}

/// [`normalize`], then the last commit date of every location.
pub(crate) async fn normalize_with_dates(
    root: &std::path::Path,
    answers: &BriefingDocsAnswers,
) -> Result<BriefingDocsAnswers, String> {
    let mut out = normalize(root, answers)?;
    let paths: Vec<String> = out.locations.iter().map(|l| l.path.clone()).collect();
    let dates = detect::last_commit_dates(root, &paths).await;
    for (location, date) in out.locations.iter_mut().zip(dates) {
        location.last_commit = date;
    }
    Ok(out)
}

/// The `## Existing documentation` section, or `None` without any location.
pub(crate) fn render_section(answers: &BriefingDocsAnswers) -> Option<String> {
    if answers.locations.is_empty() {
        return None;
    }
    let mut out = format!(
        "{DOCS_HEADING}\n> Stated in the briefing form; last-commit dates read from git by Kronn when the briefing was saved.\n"
    );
    for l in &answers.locations {
        let date = match &l.last_commit {
            Some(d) => format!("last commit {d}"),
            None => "last commit unknown".to_string(),
        };
        let mut line = format!(
            "- `{}` — {}; {}; {date}",
            l.path,
            kind_label(l.kind),
            state_label(l.state)
        );
        if l.source_of_truth {
            line.push_str(&format!(
                "; **source of truth** for the {}",
                kind_label(l.kind)
            ));
        }
        if !l.note.is_empty() {
            line.push_str(&format!(" — note: {}", cell(&l.note)));
        }
        out.push_str(&line);
        out.push('\n');
    }
    // `<` and `>` are escaped so a value can never close the HTML comment.
    let json = serde_json::to_string(answers)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    out.push_str(&format!(
        "{DOCS_ANSWERS_PREFIX}{json}{DOCS_ANSWERS_SUFFIX}\n"
    ));
    Some(out)
}

/// The documentation answers recorded in a briefing, if any.
pub(crate) fn parse(notes: &str) -> Option<BriefingDocsAnswers> {
    notes.lines().find_map(|line| {
        let json = line
            .trim()
            .strip_prefix(DOCS_ANSWERS_PREFIX)?
            .strip_suffix(DOCS_ANSWERS_SUFFIX)?;
        serde_json::from_str(json).ok()
    })
}

/// `notes` without the documentation section: kept out of every prompt until
/// a bench run shows the audit gains from reading it.
pub(crate) fn strip_section(notes: &str) -> String {
    super::project_doc::strip_sections(notes, &[DOCS_HEADING], &[DOCS_ANSWERS_PREFIX])
}

fn bad_request(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<BriefingDetectedDoc>::err(message)),
    )
        .into_response()
}

async fn project_root(state: &AppState, id: String) -> Option<std::path::PathBuf> {
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
        .ok()
        .flatten()?;
    Some(crate::core::scanner::resolve_host_path(&project.path))
}

/// GET /api/projects/:id/briefing/docs — locations found in the repository,
/// with the date of their last commit.
pub async fn detect_briefing_docs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<BriefingDetectedDoc>>> {
    let Some(root) = project_root(&state, id).await else {
        return Json(ApiResponse::err("Project not found"));
    };
    if !root.is_dir() {
        return Json(ApiResponse::ok(Vec::new()));
    }
    Json(ApiResponse::ok(detect::detect_with_dates(&root).await))
}

/// POST /api/projects/:id/briefing/docs/check — a location typed by hand,
/// normalised and dated; 400 when it leaves the repository or does not exist.
pub async fn check_briefing_doc(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CheckBriefingDocRequest>,
) -> Response {
    let Some(root) = project_root(&state, id).await else {
        return Json(ApiResponse::<BriefingDetectedDoc>::err("Project not found")).into_response();
    };
    let (path, is_dir) = match detect::normalize_path(&root, &req.path) {
        Ok(found) => found,
        Err(e) => return bad_request(e),
    };
    let last_commit = detect::last_commit_date(&root, &path).await;
    Json(ApiResponse::ok(BriefingDetectedDoc {
        kind: detect::classify(&path, is_dir),
        path,
        is_dir,
        last_commit,
    }))
    .into_response()
}

#[cfg(test)]
#[path = "briefing_docs_tests.rs"]
mod tests;
