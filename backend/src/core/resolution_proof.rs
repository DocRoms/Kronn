//! KT-1388 — a debt declared settled must cite the code that settles it.
//!
//! docroms' audit wrote "fully mitigated / verified correct" over an SSH
//! host-key fallback it had not read, and dismissed the detector signals as
//! false alarms: a conclusion that closes a finding is the one claim a
//! reader cannot afford to take on trust. One vocabulary and one proof rule
//! serve the detector disposition gate and the documentary gate, so the two
//! can never disagree on what "resolved" needs.

/// What a settled finding is called. Lower-case, matched on word boundaries.
const SETTLED_PHRASES: &[&str] = &[
    "resolved",
    "fixed",
    "mitigated",
    "false alarm",
    "false positive",
    "verified correct",
    "not applicable",
    "no longer applicable",
    "no longer applies",
    "no longer an issue",
    "not an issue",
];

/// A word before the phrase that turns it into something still to do
/// ("not resolved", "to be fixed", "once mitigated").
const NEGATING_WORDS: &[&str] = &[
    "not",
    "never",
    "no",
    "partially",
    "partly",
    "be",
    "if",
    "once",
    "until",
    "when",
    "unless",
    "needs",
    "to",
];

/// Whether `text` declares a finding resolved, mitigated, a false alarm or
/// not applicable. A line that merely mentions the word in a negation ("not
/// resolved", "to be fixed") or inside a compound ("fixed-width") does not.
pub fn declares_settled(text: &str) -> bool {
    let lower = text.to_lowercase();
    SETTLED_PHRASES.iter().any(|phrase| {
        lower.match_indices(phrase).any(|(at, _)| {
            let before = &lower[..at];
            let after = &lower[at + phrase.len()..];
            let bounded = !before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let compound = after.starts_with('-')
                && after[1..].chars().next().is_some_and(char::is_alphabetic);
            let negated = before
                .split(|c: char| !c.is_alphanumeric() && c != '\'')
                .rfind(|word| !word.is_empty())
                .is_some_and(|word| NEGATING_WORDS.contains(&word));
            bounded && !compound && !negated
        })
    })
}

/// Whether a TD sheet settles its own finding: its `Status` says so, or an
/// `audit_history` entry of its front matter does (`status:` / `note:`). The
/// status is read the way the index reads it (`td_index::parse_sheet`). An
/// entry a human wrote (reviewer `Kronn … card/review`, a user) states the
/// human's call, not a claim of this run, and is not judged here.
pub fn sheet_declares_settled(content: &str) -> bool {
    if declares_settled(&crate::core::td_index::parse_sheet("sheet", content).status) {
        return true;
    }
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    // (status + note text, reviewer) of each history entry.
    let mut entries: Vec<(String, String)> = Vec::new();
    for raw in lines.take_while(|line| line.trim() != "---") {
        let line = raw.trim_start();
        if line.starts_with("- date:") || entries.is_empty() {
            entries.push((String::new(), String::new()));
        }
        let line = line.strip_prefix("- ").unwrap_or(line);
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        if let Some(text) = line
            .strip_prefix("note:")
            .or_else(|| line.strip_prefix("status:"))
        {
            entry.0.push_str(text);
            entry.0.push('\n');
        } else if let Some(reviewer) = line.strip_prefix("reviewer:") {
            entry.1 = reviewer.to_lowercase();
        }
    }
    entries.iter().any(|(text, reviewer)| {
        let human = ["kronn", "user", "human"]
            .iter()
            .any(|who| reviewer.contains(who));
        !human && declares_settled(text)
    })
}

/// A file the proof cannot be: prose cannot prove what code does.
fn is_prose_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    path.starts_with("docs/")
        || [".md", ".mdx", ".txt", ".rst", ".adoc"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// Whether `text` carries a `[src: file: path:line]` marker (a line or a range)
/// on code: the evidence that a settled claim was checked against the source.
/// A whole-file marker, a documentation page or a `deps:` / `url:` marker is
/// not that proof.
pub fn cites_code(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("[src:") {
        let tail = &rest[start + "[src:".len()..];
        let end = tail.find(']').unwrap_or(tail.len());
        let body = tail[..end].trim();
        if let Some(target) = body.strip_prefix("file:") {
            let target = target.trim();
            if let Some((path, lines)) = target.rsplit_once(':') {
                let lines = lines.trim();
                let has_line = lines.starts_with(|c: char| c.is_ascii_digit())
                    && lines
                        .chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, '-' | ',' | ' '));
                if has_line && !is_prose_path(path.trim()) {
                    return true;
                }
            }
        }
        rest = &tail[end..];
    }
    false
}

/// The refusal sent back to the agent, naming what is missing.
pub const EVIDENCE_REQUEST: &str = "cite the code that proves it (`[src: file: path:line]` in the same row or sheet) or keep the item open";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_docroms_declarations_are_recognised() {
        for text in [
            "RESOLVED — code now uses StrictHostKeyChecking=yes. Issue fully mitigated.",
            "| SSH `StrictHostKeyChecking=no` at deploy.yml:186 | FALSE ALARM | Line 186 is a comment |",
            "host key verification — **verified correct**",
            "Status: Accepted decision (RESOLVED)",
            "not applicable: the site has no database",
        ] {
            assert!(declares_settled(text), "{text}");
        }
    }

    #[test]
    fn a_negation_or_a_compound_declares_nothing() {
        for text in [
            "still not resolved",
            "unresolved",
            "to be fixed before launch",
            "fixed-width layout",
            "partially mitigated",
            "an open finding",
        ] {
            assert!(!declares_settled(text), "{text}");
        }
    }

    #[test]
    fn only_a_line_of_code_is_proof() {
        assert!(cites_code(
            "fully mitigated [src: file: .github/workflows/deploy.yml:197-206]"
        ));
        assert!(cites_code("ok [src: file: src/a.rs:12]"));
        assert!(!cites_code("fully mitigated"));
        assert!(!cites_code("[src: file: src/a.rs]"));
        assert!(!cites_code(
            "[src: file: docs/inconsistencies-tech-debt.md:40]"
        ));
        assert!(!cites_code("[src: file: notes/plan.md:3]"));
        assert!(!cites_code("[src: url: https://example.com/page:3]"));
    }
}
