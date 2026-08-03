//! Parser for the onboarding registry (`docs/onboarding.md`).
//!
//! The registry is a doc-IA artifact — a curated catalogue of onboarding
//! courses for the Mode Mentor "onboarding" posture. It is human-curated AND
//! appended to by the onboarding audit agent (O4b), so the parser is tolerant:
//! it reads what it recognises and never fails on free-form prose.
//!
//! Format (one `##` section per topic):
//!
//! ```markdown
//! # Registre d'onboarding
//! > (blockquote intro — ignored)
//!
//! ## Titre du sujet
//! - **Niveau** : intermédiaire
//! - **Périmètre** : une phrase
//! - **Prérequis** : aucun
//! - **Références** : `backend/src/x.rs`, `docs/y.md`
//!
//! Description libre optionnelle.
//! ```
//!
//! Labels are matched case-insensitively in French or English. References are
//! split on commas and stripped of backticks. See docs/design/mentor-mode.md.

use crate::models::{Chapter, OnboardingTopic};
use std::path::Path;

/// Which labelled field a bullet line carries.
enum Field {
    Id,
    Kind,
    Level,
    Scope,
    Prerequisites,
    References,
    Course,
}

/// Map a (lowercased) label to its field, tolerant of FR/EN spellings.
fn classify_label(label: &str) -> Option<Field> {
    let l = label.trim().trim_end_matches(':').trim();
    match l {
        "id" | "identifiant" | "identifier" | "slug" => Some(Field::Id),
        "type" | "catégorie" | "categorie" | "category" | "kind" | "rôle" | "role" => {
            Some(Field::Kind)
        }
        "niveau" | "level" => Some(Field::Level),
        "périmètre" | "perimetre" | "scope" | "portée" | "portee" => Some(Field::Scope),
        "prérequis" | "prerequis" | "prerequisites" | "prerequis(es)" => {
            Some(Field::Prerequisites)
        }
        "références" | "references" | "réf" | "ref" | "fichiers" | "files" => {
            Some(Field::References)
        }
        "cours" | "course" => Some(Field::Course),
        _ => None,
    }
}

/// Normalize a `Type` value to a canonical curriculum role (tronc | branche |
/// capstone | culture). Unknown values are kept lowercased rather than dropped,
/// so a human's free-form label isn't silently lost.
fn normalize_kind(value: &str) -> Option<String> {
    let v = value.trim().to_lowercase();
    if v.is_empty() {
        return None;
    }
    let canon = if v.contains("tronc") || v.contains("trunk") || v.contains("core") {
        "tronc"
    } else if v.contains("branch") {
        "branche"
    } else if v.contains("capstone") {
        "capstone"
    } else if v.contains("cultur") {
        "culture"
    } else {
        return Some(v);
    };
    Some(canon.to_string())
}

/// Parse a `- **Label** : value` (or `- **Label**: value`) bullet.
/// Returns `(field, value)` when the line is a recognised labelled bullet.
fn parse_bullet(line: &str) -> Option<(Field, String)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("- ").or_else(|| t.strip_prefix("* "))?;
    // Expect a bold label: **Label**
    let rest = rest.trim_start();
    let inner = rest.strip_prefix("**")?;
    let end = inner.find("**")?;
    let label = &inner[..end];
    let after = inner[end + 2..].trim_start();
    // Drop the separator between label and value (":" or "-").
    let value = after.trim_start_matches(':').trim_start_matches('-').trim();
    classify_label(&label.to_lowercase()).map(|f| (f, value.to_string()))
}

/// Deterministic slug used as a topic's stable id when the registry doesn't pin
/// an explicit `ID`. Lowercases, folds common French accents to ASCII, and
/// collapses any run of non-alphanumerics to a single '-'. Stable across
/// regenerations for a given title, so it survives a display-title rewrite far
/// better than matching on the title itself.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in s.trim().to_lowercase().chars() {
        let c = match ch {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' => 'y',
            'ñ' => 'n',
            _ => ch,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Split a references value on commas, strip backticks/quotes, drop empties.
fn parse_references(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().trim_matches('`').trim_matches('"').trim())
        .filter(|s| {
            !s.is_empty() && !s.eq_ignore_ascii_case("aucun") && !s.eq_ignore_ascii_case("none")
        })
        .map(|s| s.to_string())
        .collect()
}

/// Parse the full registry markdown into topics. Everything before the first
/// `##` heading (the `#` title + intro blockquote) is ignored. A topic with an
/// empty title is dropped.
pub fn parse_registry(md: &str) -> Vec<OnboardingTopic> {
    let mut topics: Vec<OnboardingTopic> = Vec::new();
    let mut current: Option<OnboardingTopic> = None;
    let mut desc_lines: Vec<String> = Vec::new();

    // Flush the accumulated description into the current topic.
    fn flush_desc(topic: &mut OnboardingTopic, desc_lines: &mut Vec<String>) {
        while desc_lines.last().is_some_and(|l| l.trim().is_empty()) {
            desc_lines.pop();
        }
        while desc_lines.first().is_some_and(|l| l.trim().is_empty()) {
            desc_lines.remove(0);
        }
        if !desc_lines.is_empty() {
            topic.description = Some(desc_lines.join("\n"));
        }
        desc_lines.clear();
    }

    for raw in md.lines() {
        let line = raw.trim_end();
        // Skip HTML comment lines (e.g. the `<!-- proposé par l'audit … -->`
        // provenance marker the onboarding audit prepends to each topic) so
        // they never leak into a topic's description.
        if line.trim_start().starts_with("<!--") {
            continue;
        }
        // A level-2 heading opens a new topic. Deeper headings (### …) are
        // treated as description content, not topic boundaries.
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(mut t) = current.take() {
                flush_desc(&mut t, &mut desc_lines);
                if t.topic_id.is_empty() {
                    t.topic_id = slugify(&t.title);
                }
                if !t.title.trim().is_empty() {
                    topics.push(t);
                }
            }
            current = Some(OnboardingTopic {
                title: title.trim().to_string(),
                topic_id: String::new(),
                kind: None,
                level: None,
                scope: None,
                prerequisites: None,
                references: Vec::new(),
                description: None,
                course_path: None,
            });
            continue;
        }

        let Some(topic) = current.as_mut() else {
            continue; // preamble before the first topic — skip
        };

        if let Some((field, value)) = parse_bullet(line) {
            match field {
                Field::Id => topic.topic_id = slugify(&value),
                Field::Kind => topic.kind = normalize_kind(&value),
                Field::Level => topic.level = non_empty(value),
                Field::Scope => topic.scope = non_empty(value),
                Field::Prerequisites => topic.prerequisites = non_empty(value),
                Field::References => topic.references = parse_references(&value),
                Field::Course => {
                    topic.course_path = non_empty(value.trim_matches('`').trim().to_string())
                }
            }
            continue;
        }

        // Non-bullet line → part of the free-form description.
        desc_lines.push(line.to_string());
    }

    if let Some(mut t) = current.take() {
        flush_desc(&mut t, &mut desc_lines);
        if t.topic_id.is_empty() {
            t.topic_id = slugify(&t.title);
        }
        if !t.title.trim().is_empty() {
            topics.push(t);
        }
    }

    topics
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ─── Course persistence (docs/onboarding/NN-slug.md) ─────────────────────────
// The registry (this file's `parse_registry`) is the catalogue/index. When the
// onboarding posture actually generates a course, we persist its chapters to a
// detail file under `docs/onboarding/` and link it back from the index — the
// same index+folder shape as the tech-debt doc-IA artifact
// (`inconsistencies-tech-debt.md` + `docs/tech-debt/TD-*.md`).

/// Deterministically spread the correct-answer position across a course's
/// quizzes, to kill the LLM's position bias (it tends to park the right answer
/// on the same index — often B). For each quiz checkpoint, rotate the options so
/// the answer lands at position `(chapter_idx + question_idx) % n`, moving the
/// aligned `explanations` in lockstep and updating `answer`. Deterministic (no
/// RNG) so regenerating the same course is stable. A no-op for exercises, single-
/// option quizzes, or a missing/out-of-range `answer`.
pub fn normalize_checkpoint_positions(chapters: &mut [Chapter]) {
    for (ci, ch) in chapters.iter_mut().enumerate() {
        for (qi, cp) in ch.checkpoints.iter_mut().enumerate() {
            let n = cp.options.len();
            if n < 2 {
                continue;
            }
            let Some(ans) = cp.answer else { continue };
            let ans = ans as usize;
            if ans >= n {
                continue;
            }
            let target = (ci + qi) % n;
            if target == ans {
                continue;
            }
            // rotate_left(k): element at old index `ans` moves to (ans - k + n) % n.
            // We want that == target → k = (ans - target + n) % n.
            let k = (ans + n - target) % n;
            cp.options.rotate_left(k);
            if cp.explanations.len() == n {
                cp.explanations.rotate_left(k);
            }
            cp.answer = Some(target as u32);
        }
    }
}

/// A→Z letter for a 0-based option index (0→A, 1→B, …). Stays aligned with the
/// option's POSITION so the rendered letter matches `Checkpoint.answer`.
fn option_letter(idx: usize) -> char {
    (b'A' + (idx as u8)) as char
}

/// Render one chapter's checkpoint. A QUIZ (non-empty `options`) shows lettered
/// options then a folded `<details>` corrigé (correct letter + per-option
/// feedback). An EXERCISE (no options) shows the question then a folded
/// `<details>` with `reveal`. Corrigés are collapsed by default so the file
/// stays a self-test. Out-of-bounds `answer` / mismatched `explanations` never
/// panic (guarded with `.get`).
fn render_checkpoint(out: &mut String, cp: &crate::models::Checkpoint, num: Option<usize>) {
    if cp.question.trim().is_empty() {
        return;
    }
    let is_quiz = !cp.options.is_empty();
    if is_quiz {
        match num {
            Some(i) => out.push_str(&format!("**Question {}**\n\n", i + 1)),
            None => out.push_str("**Checkpoint — Quiz**\n\n"),
        }
        out.push_str(cp.question.trim());
        out.push_str("\n\n");
        for (idx, opt) in cp.options.iter().enumerate() {
            if !opt.trim().is_empty() {
                out.push_str(&format!("{}. {}\n", option_letter(idx), opt.trim()));
            }
        }
        out.push('\n');
        out.push_str("<details>\n<summary>Voir le corrigé</summary>\n\n");
        match cp.answer {
            Some(a) if (a as usize) < cp.options.len() => {
                out.push_str(&format!("**Réponse : {}**\n\n", option_letter(a as usize)));
            }
            _ => out.push_str("**Réponse : —**\n\n"),
        }
        for (idx, opt) in cp.options.iter().enumerate() {
            if opt.trim().is_empty() {
                continue;
            }
            if let Some(fb) = cp.explanations.get(idx) {
                if !fb.trim().is_empty() {
                    out.push_str(&format!("- **{}.** {}\n", option_letter(idx), fb.trim()));
                }
            }
        }
        out.push_str("\n</details>\n\n");
    } else {
        match num {
            Some(i) => out.push_str(&format!("**Question {} — Exercice**\n\n", i + 1)),
            None => out.push_str("**Checkpoint — Exercice**\n\n"),
        }
        out.push_str(cp.question.trim());
        out.push_str("\n\n");
        if let Some(r) = &cp.reveal {
            if !r.trim().is_empty() {
                out.push_str("<details>\n<summary>Voir le corrigé</summary>\n\n");
                out.push_str(r.trim());
                out.push_str("\n\n</details>\n\n");
            }
        }
    }
}

/// Render a generated onboarding course to a SELF-CONTAINED Markdown document —
/// meta header (Niveau / Prérequis / Références) + numbered chapters + checkpoints
/// with their corrigés folded into `<details>`. Pure — the IO wrapper
/// [`persist_course`] writes the result. The persisted file is meant to be read
/// on its own (without the app), hence the full corrigés.
pub fn render_course_md(
    title: &str,
    objective: &str,
    level: Option<&str>,
    prerequisites: Option<&str>,
    references: &[String],
    chapters: &[Chapter],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", title.trim()));
    if !objective.trim().is_empty() {
        out.push_str(&format!("> {}\n\n", objective.trim()));
    }
    // Meta block — always rendered so the file is self-sufficient.
    out.push_str(&format!(
        "- **Niveau** : {}\n",
        level
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("non précisé")
    ));
    out.push_str(&format!(
        "- **Prérequis** : {}\n",
        prerequisites
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Aucun")
    ));
    let refs = references
        .iter()
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .map(|r| format!("`{}`", r))
        .collect::<Vec<_>>();
    out.push_str(&format!(
        "- **Références** : {}\n\n",
        if refs.is_empty() {
            "—".to_string()
        } else {
            refs.join(", ")
        }
    ));
    out.push_str("<!-- Cours généré par le Mode Mentor (posture onboarding). Régénéré à chaque (re)génération du parcours. -->\n\n");
    for (i, ch) in chapters.iter().enumerate() {
        out.push_str(&format!("## {}. {}\n\n", i + 1, ch.title.trim()));
        // Rendered VERBATIM as Markdown — the final "Révision" chapter embeds its
        // cumulative quiz (with its own <details> blocks) directly in explanation.
        out.push_str(ch.explanation.trim());
        out.push_str("\n\n");
        let cps = ch.effective_checkpoints();
        let multi = cps.len() > 1;
        for (qi, cp) in cps.iter().enumerate() {
            render_checkpoint(&mut out, cp, if multi { Some(qi) } else { None });
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

/// Choose the filename for a topic's course under `docs/onboarding/`. Reuses an
/// existing `NN-<slug>.md` when one matches the slug (regeneration overwrites in
/// place); otherwise allocates the next free monotonic number. Reading ORDER is
/// carried by the index, not the number, so filenames stay stable. Pure.
pub fn course_filename(existing: &[String], slug: &str) -> String {
    for f in existing {
        if let Some(stem) = f.strip_suffix(".md") {
            if let Some((num, rest)) = stem.split_once('-') {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && rest == slug {
                    return f.clone();
                }
            }
        }
    }
    let max = existing
        .iter()
        .filter_map(|f| f.strip_suffix(".md"))
        .filter_map(|s| s.split_once('-'))
        .filter_map(|(n, _)| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("{:02}-{}.md", max + 1, slug)
}

/// Insert (or replace) the `- **Cours** : <rel_path>` bullet as the first bullet
/// under the `## <title>` section (matched by slugified title) of the registry
/// markdown, returning the updated document. If no matching section exists the
/// input is returned unchanged — the course file is the source of truth; the
/// index link is a best-effort convenience. Pure/idempotent.
pub fn upsert_course_link(index_md: &str, topic_title: &str, rel_path: &str) -> String {
    let target = slugify(topic_title);
    let bullet = format!("- **Cours** : {}", rel_path);
    let mut out: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut found = false;
    for line in index_md.lines() {
        if let Some(t) = line.strip_prefix("## ") {
            in_target = slugify(t.trim()) == target;
            out.push(line.to_string());
            if in_target {
                found = true;
                out.push(bullet.clone());
            }
            continue;
        }
        // Drop any pre-existing Cours bullet in the target section (replace).
        if in_target && matches!(parse_bullet(line), Some((Field::Course, _))) {
            continue;
        }
        out.push(line.to_string());
    }
    if !found {
        return index_md.to_string();
    }
    let mut joined = out.join("\n");
    if index_md.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Persist a generated onboarding course to `<docs_dir>/onboarding/NN-slug.md`
/// and link it from the `onboarding.md` registry. Returns the repo-relative
/// course path (e.g. `docs/onboarding/01-le-moteur-de-workflow.md`). Best-effort
/// on the index link (course file always written).
pub fn persist_course(
    docs_dir: &Path,
    title: &str,
    objective: &str,
    level: Option<&str>,
    prerequisites: Option<&str>,
    references: &[String],
    chapters: &[Chapter],
) -> std::io::Result<String> {
    let slug = slugify(title);
    let course_dir = docs_dir.join("onboarding");
    std::fs::create_dir_all(&course_dir)?;

    let existing: Vec<String> = std::fs::read_dir(&course_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".md"))
        .collect();
    let filename = course_filename(&existing, &slug);
    std::fs::write(
        course_dir.join(&filename),
        render_course_md(title, objective, level, prerequisites, references, chapters),
    )?;

    let docs_name = docs_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("docs");
    let rel_path = format!("{}/onboarding/{}", docs_name, filename);

    let index_path = docs_dir.join("onboarding.md");
    if let Ok(md) = std::fs::read_to_string(&index_path) {
        let updated = upsert_course_link(&md, title, &rel_path);
        if updated != md {
            let _ = std::fs::write(&index_path, updated);
        }
    }
    Ok(rel_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Registre d'onboarding

> Catalogue des cours (posture Onboarding). Curé + suggéré par l'audit.

## Le moteur de workflow
- **Niveau** : intermédiaire
- **Périmètre** : comprendre comment un workflow s'exécute step par step
- **Prérequis** : bases de Rust
- **Références** : `backend/src/workflows/runner.rs`, `docs/architecture/overview.md`

Le cœur de Kronn : orchestration multi-step, envelope inter-step, guards.

## Les plugins MCP
- **Niveau**: débutant
- **Références**: `backend/src/core/mcp_scanner.rs`

## Sujet vide sans titre utile
"#;

    #[test]
    fn parses_two_topics_and_ignores_preamble() {
        let topics = parse_registry(SAMPLE);
        assert_eq!(topics.len(), 3);
        assert_eq!(topics[0].title, "Le moteur de workflow");
        assert_eq!(topics[0].level.as_deref(), Some("intermédiaire"));
        assert_eq!(
            topics[0].scope.as_deref(),
            Some("comprendre comment un workflow s'exécute step par step")
        );
        assert_eq!(topics[0].prerequisites.as_deref(), Some("bases de Rust"));
        assert_eq!(
            topics[0].references,
            vec![
                "backend/src/workflows/runner.rs".to_string(),
                "docs/architecture/overview.md".to_string()
            ]
        );
        assert!(topics[0]
            .description
            .as_deref()
            .unwrap()
            .contains("cœur de Kronn"));
    }

    #[test]
    fn handles_colon_glued_labels_and_missing_fields() {
        let topics = parse_registry(SAMPLE);
        // Second topic: "- **Niveau**: débutant" (no space before colon).
        assert_eq!(topics[1].title, "Les plugins MCP");
        assert_eq!(topics[1].level.as_deref(), Some("débutant"));
        assert_eq!(topics[1].scope, None);
        assert_eq!(
            topics[1].references,
            vec!["backend/src/core/mcp_scanner.rs".to_string()]
        );
        assert_eq!(topics[1].description, None);
    }

    #[test]
    fn topic_with_only_a_title_is_kept() {
        // "## Sujet vide sans titre utile" has no bullets — still a valid topic.
        let topics = parse_registry(SAMPLE);
        assert_eq!(topics[2].title, "Sujet vide sans titre utile");
        assert!(topics[2].references.is_empty());
    }

    #[test]
    fn empty_or_headingless_input_yields_no_topics() {
        assert!(parse_registry("").is_empty());
        assert!(parse_registry("# Titre\n> intro\nblabla sans section").is_empty());
    }

    #[test]
    fn skips_audit_provenance_comments() {
        // The onboarding audit prepends a `<!-- … -->` marker before each
        // proposed topic; it must not pollute the previous topic's description.
        let md = "## A\n- **Niveau** : débutant\n\ndesc A\n\n<!-- proposé par l'audit onboarding (2026-07-23) -->\n## B\n- **Niveau** : avancé\n";
        let topics = parse_registry(md);
        assert_eq!(topics.len(), 2);
        assert_eq!(topics[0].description.as_deref(), Some("desc A"));
        assert_eq!(topics[1].title, "B");
        assert_eq!(topics[1].level.as_deref(), Some("avancé"));
    }

    #[test]
    fn parses_and_normalizes_the_type_field() {
        let md = "## Setup\n\
- **Type** : Tronc\n\
- **Niveau** : débutant\n\
\n\
## Video\n\
- **Category** : branch\n\
\n\
## Add a page\n\
- **Type** : Capstone\n\
\n\
## Norms\n\
- **Rôle** : Culture\n\
\n\
## No type here\n\
- **Niveau** : avancé\n";
        let t = parse_registry(md);
        assert_eq!(t.len(), 5);
        assert_eq!(t[0].kind.as_deref(), Some("tronc")); // FR label + capitalized value
        assert_eq!(t[1].kind.as_deref(), Some("branche")); // EN label "Category: branch" → branche
        assert_eq!(t[2].kind.as_deref(), Some("capstone"));
        assert_eq!(t[3].kind.as_deref(), Some("culture")); // "Rôle" label
        assert_eq!(t[4].kind, None); // no Type bullet → None (back-compat)
    }

    #[test]
    fn derives_stable_topic_id_from_title_or_explicit_id() {
        let md = "## Le moteur de workflow\n\
- **Niveau** : intermédiaire\n\
\n\
## Les plugins MCP\n\
- **ID** : Registre MCP\n\
- **Niveau** : débutant\n";
        let t = parse_registry(md);
        // No explicit id → deterministic slug of the (accent-folded) title.
        assert_eq!(t[0].topic_id, "le-moteur-de-workflow");
        // Explicit ID bullet wins over the title, and is itself slugified.
        assert_eq!(t[1].topic_id, "registre-mcp");
    }

    #[test]
    fn prerequisites_none_and_english_labels() {
        let md =
            "## X\n- **Level** : advanced\n- **Prerequisites** : none\n- **References** : `a.rs`\n";
        let topics = parse_registry(md);
        assert_eq!(topics[0].level.as_deref(), Some("advanced"));
        assert_eq!(topics[0].prerequisites.as_deref(), Some("none"));
        // "none" as a reference token is filtered, but a real path stays.
        assert_eq!(topics[0].references, vec!["a.rs".to_string()]);
    }

    #[test]
    fn parses_the_course_link_bullet() {
        let md = "## Le moteur\n- **Niveau** : intermédiaire\n- **Cours** : `docs/onboarding/01-le-moteur.md`\n";
        let t = parse_registry(md);
        assert_eq!(
            t[0].course_path.as_deref(),
            Some("docs/onboarding/01-le-moteur.md")
        );
        // Absent bullet → None (back-compat).
        assert_eq!(
            parse_registry("## X\n- **Niveau** : débutant\n")[0].course_path,
            None
        );
    }

    #[test]
    fn course_filename_allocates_and_reuses() {
        let existing = vec!["01-alpha.md".to_string(), "02-beta.md".to_string()];
        // New slug → next monotonic number.
        assert_eq!(course_filename(&existing, "gamma"), "03-gamma.md");
        // Known slug → reuse the existing file (regeneration overwrites in place).
        assert_eq!(course_filename(&existing, "beta"), "02-beta.md");
        // Empty dir → starts at 01.
        assert_eq!(course_filename(&[], "first"), "01-first.md");
    }

    #[test]
    fn upsert_course_link_inserts_replaces_and_skips_missing() {
        let md = "# Registre\n\n## Le moteur de workflow\n- **Niveau** : intermédiaire\n\n## Autre\n- **Niveau** : débutant\n";
        // Insert under the matching section only.
        let one = upsert_course_link(
            md,
            "Le moteur de workflow",
            "docs/onboarding/01-le-moteur-de-workflow.md",
        );
        assert!(one.contains(
            "## Le moteur de workflow\n- **Cours** : docs/onboarding/01-le-moteur-de-workflow.md"
        ));
        assert_eq!(one.matches("**Cours**").count(), 1);
        // Idempotent: a second upsert replaces rather than duplicates.
        let two = upsert_course_link(
            &one,
            "Le moteur de workflow",
            "docs/onboarding/01-le-moteur-de-workflow.md",
        );
        assert_eq!(two.matches("**Cours**").count(), 1);
        // Unknown section → unchanged.
        assert_eq!(upsert_course_link(md, "Inexistant", "x.md"), md);
    }

    fn chap(
        title: &str,
        explanation: &str,
        checkpoint: Option<crate::models::Checkpoint>,
    ) -> Chapter {
        Chapter {
            title: title.into(),
            explanation: explanation.into(),
            checkpoint,
            checkpoints: vec![],
            done: false,
            learner_answer: None,
            needs_review: false,
        }
    }

    #[test]
    fn render_course_md_has_title_objective_meta_and_numbered_chapters() {
        let chapters = vec![
            chap("Intro", "Le point de départ.", None),
            chap("Aller plus loin", "Les détails.", None),
        ];
        let md = render_course_md(
            "Prise en main",
            "Comprendre l'architecture.",
            Some("débutant"),
            Some("bases de Rust"),
            &["backend/src/lib.rs".into(), "docs/AGENTS.md".into()],
            &chapters,
        );
        assert!(md.starts_with("# Prise en main\n"));
        assert!(md.contains("> Comprendre l'architecture."));
        // Meta block, self-sufficient header.
        assert!(md.contains("- **Niveau** : débutant"));
        assert!(md.contains("- **Prérequis** : bases de Rust"));
        assert!(md.contains("- **Références** : `backend/src/lib.rs`, `docs/AGENTS.md`"));
        assert!(md.contains("## 1. Intro"));
        assert!(md.contains("## 2. Aller plus loin"));
    }

    #[test]
    fn render_course_md_meta_falls_back_when_absent() {
        let md = render_course_md("T", "", None, None, &[], &[chap("C", "x", None)]);
        assert!(md.contains("- **Niveau** : non précisé"));
        assert!(md.contains("- **Prérequis** : Aucun"));
        assert!(md.contains("- **Références** : —"));
    }

    #[test]
    fn render_course_md_quiz_shows_lettered_options_and_folded_corrige() {
        let cp = crate::models::Checkpoint {
            question: "Pourquoi X ?".into(),
            options: vec![
                "Parce que A".into(),
                "Parce que B".into(),
                "Parce que C".into(),
            ],
            answer: Some(1),
            explanations: vec![
                "A : non, méprise.".into(),
                "B : oui, car…".into(),
                "C : non plus.".into(),
            ],
            reveal: None,
        };
        let md = render_course_md(
            "T",
            "o",
            Some("débutant"),
            None,
            &[],
            &[chap("Ch", "corps", Some(cp))],
        );
        assert!(md.contains("**Checkpoint — Quiz**"));
        assert!(md.contains("A. Parce que A"));
        assert!(md.contains("B. Parce que B"));
        // Corrigé folded by default + correct letter + per-option feedback.
        assert!(md.contains("<details>\n<summary>Voir le corrigé</summary>"));
        assert!(md.contains("**Réponse : B**"));
        assert!(md.contains("- **B.** B : oui, car…"));
    }

    #[test]
    fn render_course_md_exercise_reveals_corrige() {
        let cp = crate::models::Checkpoint {
            question: "Écris la migration.".into(),
            options: vec![],
            answer: None,
            explanations: vec![],
            reveal: Some("ALTER TABLE … ADD COLUMN archived_at".into()),
        };
        let md = render_course_md("T", "o", None, None, &[], &[chap("Ch", "corps", Some(cp))]);
        assert!(md.contains("**Checkpoint — Exercice**"));
        assert!(md.contains("<details>\n<summary>Voir le corrigé</summary>"));
        assert!(md.contains("ALTER TABLE … ADD COLUMN archived_at"));
    }

    fn quiz(q: &str, opts: &[&str], answer: u32, expl: &[&str]) -> crate::models::Checkpoint {
        crate::models::Checkpoint {
            question: q.into(),
            options: opts.iter().map(|s| s.to_string()).collect(),
            answer: Some(answer),
            explanations: expl.iter().map(|s| s.to_string()).collect(),
            reveal: None,
        }
    }
    fn chap_cps(title: &str, expl: &str, checkpoints: Vec<crate::models::Checkpoint>) -> Chapter {
        Chapter {
            title: title.into(),
            explanation: expl.into(),
            checkpoint: None,
            checkpoints,
            done: false,
            learner_answer: None,
            needs_review: false,
        }
    }

    #[test]
    fn normalize_spreads_answer_positions_and_keeps_alignment() {
        // Two chapters, one quiz each, both with the answer parked on index 1 (B).
        let mut chapters = vec![
            chap_cps(
                "A",
                "x",
                vec![quiz("q", &["a", "b", "c"], 1, &["fa", "fb", "fc"])],
            ),
            chap_cps(
                "B",
                "x",
                vec![quiz("q", &["a", "b", "c"], 1, &["fa", "fb", "fc"])],
            ),
        ];
        normalize_checkpoint_positions(&mut chapters);
        // Targets: ch0 → (0+0)%3=0, ch1 → (1+0)%3=1. So ch0's answer moves to 0.
        let cp0 = &chapters[0].checkpoints[0];
        assert_eq!(cp0.answer, Some(0));
        // The option that WAS correct ("b") is now at index 0, and its feedback
        // ("fb") rode along — alignment preserved.
        assert_eq!(cp0.options[0], "b");
        assert_eq!(cp0.explanations[0], "fb");
        // ch1 target == original index → unchanged.
        assert_eq!(chapters[1].checkpoints[0].answer, Some(1));
        assert_eq!(chapters[1].checkpoints[0].options[1], "b");
    }

    #[test]
    fn render_numbers_multiple_checkpoints() {
        let ch = chap_cps(
            "Révision",
            "Récapitulons.",
            vec![
                quiz("Q un ?", &["a", "b"], 0, &["oui", "non"]),
                quiz("Q deux ?", &["c", "d"], 1, &["non", "oui"]),
            ],
        );
        let md = render_course_md("T", "o", None, None, &[], &[ch]);
        assert!(md.contains("**Question 1**"));
        assert!(md.contains("**Question 2**"));
        assert!(!md.contains("**Checkpoint — Quiz**")); // multi → numbered, not the singular header
    }

    #[test]
    fn effective_checkpoints_folds_legacy_singular() {
        let legacy = Chapter {
            title: "L".into(),
            explanation: "x".into(),
            checkpoint: Some(quiz("q", &["a", "b"], 1, &["fa", "fb"])),
            checkpoints: vec![],
            done: false,
            learner_answer: None,
            needs_review: false,
        };
        assert_eq!(legacy.effective_checkpoints().len(), 1);
        // A legacy single checkpoint still renders (folded) — as the singular header.
        let md = render_course_md("T", "o", None, None, &[], &[legacy]);
        assert!(md.contains("**Checkpoint — Quiz**"));
    }

    #[test]
    fn render_course_md_quiz_out_of_bounds_answer_is_safe() {
        // answer past the options + explanations shorter than options: no panic,
        // "Réponse : —", only the feedback that exists is emitted.
        let cp = crate::models::Checkpoint {
            question: "Q ?".into(),
            options: vec!["a".into(), "b".into()],
            answer: Some(9),
            explanations: vec!["seul feedback".into()],
            reveal: None,
        };
        let md = render_course_md("T", "o", None, None, &[], &[chap("Ch", "corps", Some(cp))]);
        assert!(md.contains("**Réponse : —**"));
        assert!(md.contains("- **A.** seul feedback"));
        assert!(!md.contains("- **B.**")); // no feedback at idx 1 → not emitted
    }
}
