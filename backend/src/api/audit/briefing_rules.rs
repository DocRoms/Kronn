// Project rules and bans in the briefing (KT-1193): a few lines the team
// states about the code, each a rule (always) or a ban (never), a ban
// optionally carrying a pattern Kronn checks. Recorded in `docs/briefing.md`
// like the project facts, mirrored into the coding-rules doc, and handed to
// the audit steps that judge the code.

use std::path::Path;

use crate::core::project_rules::pattern_kind_label;
use crate::models::{
    BriefingProjectRules, ProjectRule, ProjectRuleKind, ProjectRulePattern, ProjectRulePatternKind,
};

use super::project_doc::{cell, check_text};

pub(crate) const RULES_HEADING: &str = "## Project rules";
pub(crate) const RULES_ANSWERS_PREFIX: &str = "<!-- kronn:project-rules v1 ";
const RULES_ANSWERS_SUFFIX: &str = " -->";
pub(crate) const MAX_RULES: usize = 20;
pub(crate) const TEXT_MAX_CHARS: usize = 200;
pub(crate) const PATTERN_MAX_CHARS: usize = 100;
/// Keeps a file glob readable; the matcher is linear whatever the pattern.
const MAX_GLOB_STARS: usize = 3;

/// Opening line of the Kronn-generated section of the coding-rules doc.
pub(crate) const DOC_SECTION_OPEN: &str = "<!-- kronn:section name=\"project-rules\"";
const DOC_SECTION_END: &str = "<!-- kronn:section:end -->";
const CODING_RULES_FILE: &str = "coding-rules.md";

// ─── Validation ─────────────────────────────────────────────────────────────

fn check_pattern(pattern: &ProjectRulePattern) -> Result<ProjectRulePattern, String> {
    let value = pattern.value.trim().to_string();
    let field = "project_rules.pattern";
    if value.is_empty() {
        return Err(format!("{field}: empty"));
    }
    check_text(field, &value, PATTERN_MAX_CHARS)?;
    let allowed = |c: char| match pattern.kind {
        ProjectRulePatternKind::Dependency => {
            c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '_' | '-')
        }
        ProjectRulePatternKind::Import => {
            c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '_' | '-' | ':' | '\\' | '$')
        }
        ProjectRulePatternKind::FileGlob => {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '*' | '?' | '@' | '+')
        }
    };
    if let Some(bad) = value.chars().find(|c| !allowed(*c)) {
        return Err(format!(
            "{field}: `{bad}` is not allowed in a {} pattern",
            pattern_kind_label(pattern.kind)
        ));
    }
    if pattern.kind == ProjectRulePatternKind::FileGlob {
        if value.starts_with('/') || value.split('/').any(|s| s == "..") {
            return Err(format!(
                "{field}: a file glob stays relative to the repository"
            ));
        }
        // Each `*` or `**` is one wildcard; three stars or more mean nothing.
        let runs: Vec<usize> = value
            .split(|c| c != '*')
            .map(str::len)
            .filter(|n| *n > 0)
            .collect();
        if runs.iter().any(|n| *n > 2) {
            return Err(format!("{field}: `***` is not a wildcard; use `*` or `**`"));
        }
        if runs.len() > MAX_GLOB_STARS {
            return Err(format!(
                "{field}: at most {MAX_GLOB_STARS} wildcards in a file glob"
            ));
        }
    }
    Ok(ProjectRulePattern {
        kind: pattern.kind,
        value,
    })
}

/// Trim and validate the lines: at most [`MAX_RULES`], each a single bounded
/// line; empty lines and exact duplicates are dropped; only a ban may carry a
/// pattern.
pub(crate) fn normalize(rules: &BriefingProjectRules) -> Result<BriefingProjectRules, String> {
    let mut out = BriefingProjectRules::default();
    for rule in &rules.rules {
        let text = rule.text.trim().to_string();
        check_text("project_rules.text", &text, TEXT_MAX_CHARS)?;
        let pattern = match &rule.pattern {
            None => None,
            Some(_) if rule.kind == ProjectRuleKind::Rule => {
                return Err("project_rules: a verifiable pattern applies to a ban only".to_string())
            }
            Some(p) => Some(check_pattern(p)?),
        };
        if text.is_empty() {
            if pattern.is_some() {
                return Err("project_rules: a ban with a pattern needs its text".to_string());
            }
            continue;
        }
        let entry = ProjectRule {
            kind: rule.kind,
            text,
            pattern,
        };
        if out.rules.contains(&entry) {
            continue;
        }
        out.rules.push(entry);
    }
    if out.rules.len() > MAX_RULES {
        return Err(format!("project_rules: at most {MAX_RULES} lines"));
    }
    Ok(out)
}

// ─── Briefing section (docs/briefing.md) ───────────────────────────────────

fn rule_line(rule: &ProjectRule) -> String {
    let lead = match rule.kind {
        ProjectRuleKind::Rule => "Always",
        ProjectRuleKind::Ban => "Never",
    };
    let mut line = format!("{lead}: {}", cell(&rule.text));
    if let Some(p) = &rule.pattern {
        line.push_str(&format!(
            " — checked by Kronn: {} `{}`",
            pattern_kind_label(p.kind),
            p.value
        ));
    }
    line
}

/// The `## Project rules` section, or `None` without any line (the briefing
/// then keeps its earlier shape byte for byte).
pub(crate) fn render_section(rules: &BriefingProjectRules) -> Option<String> {
    if rules.rules.is_empty() {
        return None;
    }
    let mut out = format!(
        "{RULES_HEADING}\n> Stated in the briefing form. A ban with a pattern is checked by Kronn at each audit.\n"
    );
    for rule in &rules.rules {
        out.push_str(&format!("- {}\n", rule_line(rule)));
    }
    // `<` and `>` are escaped so a value can never close the HTML comment.
    let json = serde_json::to_string(rules)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    out.push_str(&format!(
        "{RULES_ANSWERS_PREFIX}{json}{RULES_ANSWERS_SUFFIX}\n"
    ));
    Some(out)
}

/// The rules recorded in a briefing, if any.
pub(crate) fn parse(notes: &str) -> Option<BriefingProjectRules> {
    notes.lines().find_map(|line| {
        let json = line
            .trim()
            .strip_prefix(RULES_ANSWERS_PREFIX)?
            .strip_suffix(RULES_ANSWERS_SUFFIX)?;
        serde_json::from_str(json).ok()
    })
}

/// `notes` without the rules section: the steps that judge the code get
/// their own short block instead, the other steps read the coding-rules doc.
pub(crate) fn strip_section(notes: &str) -> String {
    super::project_doc::strip_sections(notes, &[RULES_HEADING], &[RULES_ANSWERS_PREFIX])
}

// ─── Audit prompt ───────────────────────────────────────────────────────────

/// The steps that judge the code: every step writing a TD index.
pub(crate) fn judges_code(target_file: &str) -> bool {
    target_file.starts_with("docs/inconsistencies-")
}

/// The rules block for a step prompt; empty for a step that does not judge
/// the code and when the briefing states no rule.
pub(crate) fn prompt_block(target_file: &str, rules: Option<&BriefingProjectRules>) -> String {
    let Some(rules) = rules.filter(|r| !r.rules.is_empty()) else {
        return String::new();
    };
    if !judges_code(target_file) {
        return String::new();
    }
    let mut out = String::from(
        "\n\n## Project rules and bans (from the briefing)\nThe team stated these rules for this code base:\n",
    );
    for rule in &rules.rules {
        out.push_str(&format!("- {}\n", rule_line(rule)));
    }
    out.push_str(
        "A ban violated or a rule unmet that you find in the code is a TD with its proof (`file:line`). \
Never propose a fix that breaks a ban.\n",
    );
    out
}

// ─── Coding-rules doc ───────────────────────────────────────────────────────

fn doc_section(rules: &BriefingProjectRules) -> String {
    let mut out = format!(
        "{DOC_SECTION_OPEN} owner=\"kronn\" generated=\"briefing\" -->\n\
## Project rules and bans\n\n\
> Written by Kronn from the project briefing at each save and each audit: change them in the briefing, not here.\n\n"
    );
    for rule in &rules.rules {
        let lead = match rule.kind {
            ProjectRuleKind::Rule => "**Always:**",
            ProjectRuleKind::Ban => "**Never:**",
        };
        let mut line = format!("- {lead} {}", cell(&rule.text));
        if let Some(p) = &rule.pattern {
            line.push_str(&format!(
                " (checked by Kronn: {} `{}`)",
                pattern_kind_label(p.kind),
                p.value
            ));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(DOC_SECTION_END);
    out.push('\n');
    out
}

/// A line Kronn writes inside the section; nothing else can be removed when
/// the end marker is missing.
fn is_generated_line(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t == "## Project rules and bans"
        || t.starts_with("> Written by Kronn from the project briefing")
        || t.starts_with("- **Always:** ")
        || t.starts_with("- **Never:** ")
}

/// Byte range of the generated section, end marker line included. Without
/// an end marker (an agent dropped it), the range stops at the first line
/// Kronn could not have written, so human text is never swallowed.
fn find_doc_section(content: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut start = None;
    let mut generated_end = 0;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        match start {
            None if trimmed.starts_with(DOC_SECTION_OPEN) => {
                start = Some(offset);
                generated_end = offset + line.len();
            }
            Some(s) if trimmed == DOC_SECTION_END => return Some((s, offset + line.len())),
            Some(_) if generated_end == offset && is_generated_line(line) => {
                generated_end = offset + line.len();
            }
            _ => {}
        }
        offset += line.len();
    }
    let start = start?;
    tracing::warn!(
        "coding-rules doc: the project-rules section lost its end marker; only Kronn's own lines are replaced"
    );
    // Keep the blank line before the next human text.
    let tail = &content[start..generated_end];
    let trimmed = tail.trim_end_matches(['\n', ' ']).len();
    let end = (start + trimmed + 1).min(generated_end);
    Some((start, end))
}

/// `content` with the generated section set to `rules`: replaced in place,
/// inserted before the first `## ` heading, or removed when there is no rule.
/// The rest of the doc is left byte for byte.
pub(crate) fn upsert_doc_section(content: &str, rules: Option<&BriefingProjectRules>) -> String {
    let section = rules.filter(|r| !r.rules.is_empty()).map(doc_section);
    if let Some((start, end)) = find_doc_section(content) {
        let (head, tail) = (&content[..start], &content[end..]);
        return match section {
            Some(section) => format!("{head}{section}{tail}"),
            None => {
                let tail = tail.strip_prefix('\n').unwrap_or(tail);
                format!("{head}{tail}")
            }
        };
    }
    let Some(section) = section else {
        return content.to_string();
    };
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.starts_with("## ") {
            return format!("{}{section}\n{}", &content[..offset], &content[offset..]);
        }
        offset += line.len();
    }
    let mut out = content.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&section);
    out
}

/// Bring the coding-rules doc in line with the briefing's rules. No-op when
/// the doc does not exist yet; `Ok(true)` when the file was rewritten.
pub(crate) fn sync_coding_rules_doc(
    project_root: &Path,
    rules: Option<&BriefingProjectRules>,
) -> Result<bool, String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_root);
    let target = docs_dir.join(CODING_RULES_FILE);
    let current = match std::fs::read_to_string(&target) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("read {}: {e}", target.display())),
    };
    let next = upsert_doc_section(&current, rules);
    if next == current {
        return Ok(false);
    }
    crate::core::fs_guard::assert_contained_no_symlink(project_root, &target)?;
    crate::core::mcp_scanner::atomic_write(&target, &next)?;
    Ok(true)
}

/// [`sync_coding_rules_doc`] from the briefing notes, for the audit hooks:
/// never fails the audit, a failure is logged.
pub(crate) fn sync_from_notes(project_root: &Path, notes: Option<&str>) {
    let rules = notes.and_then(parse);
    if let Err(e) = sync_coding_rules_doc(project_root, rules.as_ref()) {
        tracing::warn!("project rules not written to the coding-rules doc: {e}");
    }
}

#[cfg(test)]
#[path = "briefing_rules_tests.rs"]
mod tests;
