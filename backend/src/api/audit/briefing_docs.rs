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
use crate::core::docs_location::DocsRoot;
use crate::models::{
    ApiResponse, BriefingDetectedDoc, BriefingDocKind, BriefingDocLocation, BriefingDocState,
    BriefingDocsAnswers, CheckBriefingDocRequest,
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

/// One location as a readable line: path, kind, state and last commit, then
/// the source-of-truth flag and the note when set.
fn location_line(l: &BriefingDocLocation) -> String {
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
    line
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
        out.push_str(&location_line(l));
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

/// `notes` without the documentation section: the briefing block of a step
/// prompt never carries it. [`prompt_block`] gives the steps that use it their
/// own, when the setting is on (KT-999).
pub(crate) fn strip_section(notes: &str) -> String {
    super::project_doc::strip_sections(notes, &[DOCS_HEADING], &[DOCS_ANSWERS_PREFIX])
}

// ─── Audit prompt (KT-999) ──────────────────────────────────────────────────

/// Set to `1` to give the audit steps the documentation section for one run
/// (the bench switch); the `audit_briefing_docs` setting does it for good.
pub(crate) const ENV_SWITCH: &str = "KRONN_EXP_BRIEFING_DOCS";

/// Longest documentation block, in characters, in a step prompt. It counts
/// inside the briefing cap: the briefing text gives up the room it takes.
pub(crate) const PROMPT_MAX_CHARS: usize = 3_000;

/// Whether the audit prompts carry the documentation section: the setting, or
/// the env switch of a bench run. Off by default.
pub(crate) fn prompt_enabled(setting: bool) -> bool {
    setting
        || crate::core::child_env::var(ENV_SWITCH)
            .is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// The audit steps that use the documentation section.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DocsStep {
    /// Step 1, the project index (`AGENTS.md`).
    Index,
    /// Step 8, the tech-debt step.
    TechDebt,
    /// The API design audit.
    Api,
}

/// By file name: a step target lives under the project's docs root (KT-1192).
fn docs_step(target_file: &str) -> Option<DocsStep> {
    match target_file.rsplit('/').next()? {
        "AGENTS.md" => Some(DocsStep::Index),
        "inconsistencies-tech-debt.md" => Some(DocsStep::TechDebt),
        "inconsistencies-api.md" => Some(DocsStep::Api),
        _ => None,
    }
}

/// How the audit treats a location, in reading order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Reference,
    Obsolete,
    /// Old or partial: the team says it lags behind the code.
    Verify,
    Recent,
    Unknown,
}

impl Group {
    fn of(state: BriefingDocState) -> Self {
        match state {
            BriefingDocState::Reference => Self::Reference,
            BriefingDocState::Obsolete => Self::Obsolete,
            BriefingDocState::Old | BriefingDocState::Partial => Self::Verify,
            BriefingDocState::Recent => Self::Recent,
            BriefingDocState::Unknown => Self::Unknown,
        }
    }

    /// Which locations are dropped last when the block does not fit.
    fn keep_rank(self) -> u8 {
        match self {
            Self::Obsolete => 0,
            Self::Reference => 1,
            Self::Verify => 2,
            Self::Recent => 3,
            Self::Unknown => 4,
        }
    }

    /// The heading and the rule the audit follows for these locations.
    fn rule(self, step: DocsStep, docs_root: &DocsRoot) -> String {
        match (self, step) {
            (Self::Reference, _) => "### Reference\n\
Read each one first, reuse its facts and cite it (its path) instead of re-deriving them.\n"
                .to_string(),
            (Self::Obsolete, DocsStep::TechDebt) => format!(
                "### Obsolete — this IS tech debt\n\
Each one is tech debt, whatever the scope or the dimension of this audit. Record one TD per location: \
a row in the `Current list` table AND a detail file `{}` (Area `Docs`, Status `Inferred`) whose Problem names \
the location and its last commit date as listed below, and whose Where points to the location. \
Copy no fact from it. Never write that it is out of scope, not part of this audit or not tech debt, \
and never skip it because another document is the source of truth. \
On a later audit, never list it under `## Reconciliation candidates` as `out of scope this audit`: \
re-emit it, reusing its TD ID.\n",
                docs_root.path("tech-debt/TD-<date>-<slug>.md")
            ),
            (Self::Obsolete, _) => "### Obsolete — this IS tech debt\n\
Copy no fact from it and cite it for nothing. It is tech debt: the tech-debt step records it. \
Never call it out of scope.\n"
                .to_string(),
            (Self::Verify, _) => "### Old or partial\n\
Verify each fact against the code before you reuse it.\n"
                .to_string(),
            (Self::Recent, _) => "### Recent\n\
The team calls it up to date, but it is not a reference: check a fact against the code \
before you rely on it for anything important.\n"
                .to_string(),
            (Self::Unknown, _) => "### State not provided\n\
Nobody said how reliable it is: verify each fact against the code before you reuse it.\n"
                .to_string(),
        }
    }
}

/// Whether a recorded location can go into a prompt line as it is: single-line
/// text within the form's limits, a path that cannot close its code span, and a
/// date shaped like `YYYY-MM-DD`.
fn prompt_safe(l: &BriefingDocLocation) -> bool {
    let date_ok = l.last_commit.as_deref().is_none_or(|d| {
        d.len() == 10
            && d.char_indices().all(|(i, c)| match i {
                4 | 7 => c == '-',
                _ => c.is_ascii_digit(),
            })
    });
    !l.path.is_empty()
        && !l.path.contains('`')
        && check_text("path", &l.path, 300).is_ok()
        && check_text("note", &l.note, NOTE_MAX_CHARS).is_ok()
        && date_ok
}

/// The `## Existing documentation` block of a step prompt, from the answers
/// recorded in the briefing. Empty for a step that does not use it and for a
/// briefing without a location; the caller decides whether the setting is on
/// ([`prompt_enabled`]). At most [`PROMPT_MAX_CHARS`]: the obsolete locations
/// are the last to be dropped.
pub(crate) fn prompt_block(target_file: &str, notes: &str, docs_root: &DocsRoot) -> String {
    let Some(step) = docs_step(target_file) else {
        return String::new();
    };
    let Some(mut answers) = parse(notes) else {
        return String::new();
    };
    // `briefing.md` can be hand-edited: what reaches a prompt is checked again.
    answers.locations.retain(prompt_safe);
    if answers.locations.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n## Existing documentation (from the briefing)\n\
The team listed the documentation that already exists here. The state is the team's word; \
the date is the last git commit that touched the location.\n",
    );
    let mut groups: Vec<Group> = answers
        .locations
        .iter()
        .map(|l| Group::of(l.state))
        .collect();
    groups.sort();
    groups.dedup();
    let rules: Vec<String> = groups.iter().map(|g| g.rule(step, docs_root)).collect();
    let fixed = out.chars().count() + rules.iter().map(|r| r.chars().count() + 1).sum::<usize>();
    // Room left for the lines, minus the "not shown" marker.
    let mut room = PROMPT_MAX_CHARS.saturating_sub(fixed + 120);
    let mut ranked: Vec<(Group, String)> = answers
        .locations
        .iter()
        .map(|l| (Group::of(l.state), location_line(l)))
        .collect();
    ranked.sort_by_key(|(g, _)| g.keep_rank());
    let mut kept: Vec<(Group, String)> = Vec::new();
    let mut dropped = 0usize;
    for (group, line) in ranked {
        let cost = line.chars().count() + 1;
        if cost <= room {
            room -= cost;
            kept.push((group, line));
        } else {
            dropped += 1;
        }
    }
    for (group, rule) in groups.iter().zip(&rules) {
        out.push('\n');
        out.push_str(rule);
        for (_, line) in kept.iter().filter(|(g, _)| g == group) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n[{dropped} more location(s) not shown: read {}.]\n",
            docs_root.path("briefing.md")
        ));
    }
    out
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
