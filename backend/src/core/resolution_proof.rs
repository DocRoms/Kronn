//! KT-1388 — a debt declared settled must cite the code that settles it.
//!
//! docroms' audit wrote "fully mitigated / verified correct" over an SSH
//! host-key fallback it had not read, and dismissed the detector signals as
//! false alarms: a conclusion that closes a finding is the one claim a
//! reader cannot afford to take on trust. One vocabulary and one proof rule
//! serve the detector disposition gate and the documentary gate, so the two
//! can never disagree on what "resolved" needs.
//!
//! The vocabulary reads a word as a closure only where it closes something:
//! not `fixed CSS declarations` (an adjective), not `partially mitigated` or
//! `cannot be considered resolved` (a negation), not `previous outcome: not
//! applicable` or a finding that was reopened (history), and, in a sentence
//! naming several debts, only for the debt the clause is about.

use std::ops::Range;

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

/// The word right before the phrase that turns it into something still to do
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

/// A negation anywhere in the three words before the phrase ("is not yet
/// resolved", "cannot be considered resolved", "neither fixed nor").
const NEGATORS: &[&str] = &[
    "not",
    "never",
    "no",
    "cannot",
    "can't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "hasn't",
    "haven't",
    "won't",
    "without",
    "nor",
    "neither",
    "yet",
    "partially",
    "partly",
];

/// A clause that says one of these tells what a finding WAS or may become: a
/// closure in it is history, not the state of the finding now.
const HISTORY_CUES: &[&str] = &[
    "previous",
    "previously",
    "formerly",
    "earlier",
    "reopened",
    "reopen",
    "reopens",
    "regressed",
    "recurred",
    "again",
];

/// After `fixed`, a word that shows it is a verb ("fixed in source", "fixed
/// the header"), not an adjective ("fixed CSS declarations", "fixed width").
const FIXED_VERB_NEXT: &[&str] = &[
    "in", "by", "on", "at", "as", "with", "via", "since", "during", "after", "now", "and", "or",
    "to", "for", "the", "a", "an", "this", "that", "upstream", "already", "here", "it", "them",
    "all", "both", "but", "so", "then", "from",
];

/// Before `fixed`, a word that shows it is a verb ("is fixed", "now fixed").
const FIXED_VERB_PREV: &[&str] = &[
    "is", "was", "been", "now", "are", "were", "already", "be", "got", "has", "have", "fully",
    "also", "status",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The words before, nearest first. A hyphenated name stays one word: the
/// `no` of `TD-…-no-skip-link` is a slug, not a negation.
fn words_before(head: &str) -> impl Iterator<Item = &str> {
    head.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .rev()
        .filter(|word| !word.is_empty() && !word.chars().all(|c| c == '-'))
}

/// The byte range of the clause around `at`: a sentence or a `;` part of the
/// text. A table row is one clause: its cells are read together.
pub fn clause_bounds(text: &str, at: usize) -> Range<usize> {
    let is_break = |i: usize, c: char| match c {
        ';' | '\n' => true,
        '.' | '!' | '?' => text[i + c.len_utf8()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace),
        _ => false,
    };
    let start = text[..at]
        .char_indices()
        .rev()
        .find(|&(i, c)| is_break(i, c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = text[at..]
        .char_indices()
        .find(|&(i, c)| is_break(at + i, c))
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// Whether `phrase` found at `at` of `lower` (the lower-cased text) closes a
/// finding.
fn closes(lower: &str, at: usize, phrase: &str) -> bool {
    let before = &lower[..at];
    let after = &lower[at + phrase.len()..];
    let bounded = !before.chars().next_back().is_some_and(is_word_char)
        && !after.chars().next().is_some_and(is_word_char);
    if !bounded {
        return false;
    }
    // `fixed-width`, `non-fixed`.
    let compound_after =
        after.starts_with('-') && after[1..].chars().next().is_some_and(char::is_alphabetic);
    let compound_before = before.ends_with('-')
        && before[..before.len() - 1]
            .chars()
            .next_back()
            .is_some_and(char::is_alphabetic);
    if compound_after || compound_before {
        return false;
    }
    let clause = clause_bounds(lower, at);
    // A negation only reaches the phrase within its own stretch of text: the
    // `no` of "no skip link — resolved" is not about what follows the dash.
    let head = &lower[clause.start..at];
    let head = head
        .rsplit(['—', '–', '|', ':', ',', '(', ')'])
        .next()
        .unwrap_or(head);
    if words_before(head)
        .next()
        .is_some_and(|word| NEGATING_WORDS.contains(&word))
        || words_before(head)
            .take(3)
            .any(|word| NEGATORS.contains(&word))
    {
        return false;
    }
    let whole = &lower[clause.clone()];
    if whole.contains("re-open")
        || whole
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| HISTORY_CUES.contains(&word))
    {
        return false;
    }
    if phrase == "fixed" {
        let next = after.trim_start_matches([' ', '\t']);
        let next_word: String = next.chars().take_while(|c| c.is_alphabetic()).collect();
        let verb_next = next_word.is_empty() || FIXED_VERB_NEXT.contains(&next_word.as_str());
        let verb_prev = words_before(head)
            .next()
            .is_some_and(|word| FIXED_VERB_PREV.contains(&word))
            || head.trim_end().ends_with(['✓', '✔', '☑']);
        return verb_next || verb_prev;
    }
    true
}

/// The byte offsets (in `text`) where it declares a finding resolved,
/// mitigated, a false alarm or not applicable.
pub fn settled_at(text: &str) -> Vec<usize> {
    // ASCII-only lowering keeps every offset of `text` valid in `lower`.
    let lower = text.to_ascii_lowercase();
    let mut found: Vec<usize> = SETTLED_PHRASES
        .iter()
        .flat_map(|phrase| {
            lower
                .match_indices(phrase)
                .filter(|&(at, _)| closes(&lower, at, phrase))
                .map(|(at, _)| at)
                .collect::<Vec<_>>()
        })
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// Whether `text` declares a finding resolved, mitigated, a false alarm or
/// not applicable. A line that merely mentions the word in a negation ("not
/// resolved", "to be fixed"), as an adjective ("fixed CSS declarations"), in
/// history ("previously resolved", "reopened") or inside a compound
/// ("fixed-width") does not.
pub fn declares_settled(text: &str) -> bool {
    !settled_at(text).is_empty()
}

/// Whether a TD sheet settles its own finding: its `Status` says so, or — when
/// it states none — its latest `audit_history` entry does (`status:` /
/// `note:`). The status is read the way the index reads it
/// (`td_index::parse_sheet`); an older entry a later status contradicts
/// (reopened) is history. An entry a human wrote (reviewer `Kronn … card/review`,
/// a user) states the human's call, not a claim of this run, and is not judged
/// here.
pub fn sheet_declares_settled(content: &str) -> bool {
    let status = crate::core::td_index::parse_sheet("sheet", content).status;
    if !status.eq_ignore_ascii_case("unknown") {
        return declares_settled(&status);
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
    entries
        .iter()
        .rev()
        .find(|(_, reviewer)| {
            !["kronn", "user", "human"]
                .iter()
                .any(|who| reviewer.contains(who))
        })
        .is_some_and(|(text, _)| declares_settled(text))
}

/// A file the proof cannot be: prose cannot prove what code does.
fn is_prose_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    path.starts_with("docs/")
        || [".md", ".mdx", ".txt", ".rst", ".adoc"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// Whether `target` (`path:line`, `path:start-end`, `path:line#symbol`, with a
/// hyphen, en dash or em dash in a range) points at a line of code.
fn is_code_line_target(target: &str) -> bool {
    let target = target.trim();
    // The `#symbol` anchor of `path:line#symbol` names the code the line holds.
    let target = match target.rsplit_once('#') {
        Some((head, symbol)) if crate::core::anti_halluc::is_anchor_symbol(symbol.trim()) => {
            head.trim_end()
        }
        _ => target,
    };
    let Some((path, lines)) = target.rsplit_once(':') else {
        return false;
    };
    let lines = lines.trim();
    let has_line = lines.starts_with(|c: char| c.is_ascii_digit())
        && lines
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '–' | '—' | ',' | ' '));
    has_line && !is_prose_path(path.trim())
}

/// Whether `text` carries a `[src: file: path:line]` marker (a line, a range
/// or an anchored `path:line#symbol`) on code: the evidence that a settled
/// claim was checked against the source. A whole-file marker, a documentation
/// page or a `deps:` / `url:` marker is not that proof. A bare `` `path:line` ``
/// is not either: it is how a finding names its own location.
pub fn cites_code(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("[src:") {
        let tail = &rest[start + "[src:".len()..];
        let end = tail.find(']').unwrap_or(tail.len());
        if let Some(target) = tail[..end].trim().strip_prefix("file:") {
            if is_code_line_target(target) {
                return true;
            }
        }
        rest = &tail[end..];
    }
    false
}

/// Extensions of the files an inline `` `path:line` `` may name for it to be code
/// (a host such as `example.com:443` is not).
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "php", "twig", "html", "css", "scss",
    "yml", "yaml", "json", "toml", "xml", "sh", "go", "java", "kt", "rb", "swift", "c", "h", "cpp",
    "vue", "svelte", "sql", "tf", "conf", "ini", "hbs", "j2", "gradle",
];

/// Whether `text` carries an inline `` `path:line` `` (or `path:start-end`,
/// `path:line#symbol`) on a code file: a location a reader can open. Weaker than
/// [`cites_code`] — a finding names its own location this way — so only a
/// documentary check on a row that already says the finding is closed may rely
/// on it, never the detector gate, whose signals are named exactly so.
pub fn names_code_line(text: &str) -> bool {
    text.split('`').skip(1).step_by(2).any(|span| {
        let span = span.trim();
        let span = span.rsplit_once('#').map_or(span, |(head, symbol)| {
            if crate::core::anti_halluc::is_anchor_symbol(symbol.trim()) {
                head.trim_end()
            } else {
                span
            }
        });
        let Some((path, _)) = span.rsplit_once(':') else {
            return false;
        };
        let ext = path
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase());
        is_code_line_target(span)
            && !path.contains(char::is_whitespace)
            && ext.is_some_and(|ext| CODE_EXTENSIONS.contains(&ext.as_str()))
    })
}

/// [`cites_code`] or [`names_code_line`]: the code a closed finding points at.
pub fn locates_code(text: &str) -> bool {
    cites_code(text) || names_code_line(text)
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
            // The measured closures that are real ones.
            "| **TD-20260719-a11y-no-skip-link** | ✓ Fixed | `base.html.twig:52` |",
            "Status: Fixed in source",
            "- `TD-x` — Unescaped `$RELEASE_DIR` in deploy heredoc — resolved in 2026-09-04 recovery",
            "- TD-20260719-cq-inline-css-per-page — **RESOLVED**: no inline `<style>` blocks found",
            "The header was fixed upstream.",
            // The `no` of a slug or of a finding's title is not a negation.
            "- `TD-20260719-a11y-no-skip-link` — No skip link — resolved in 2026-09-04 recovery",
            "- `TD-20260719-no-tests` — no tests — RESOLVED (273 tests)",
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
            "is not yet resolved",
            "cannot be considered resolved",
            "neither fixed nor mitigated",
        ] {
            assert!(!declares_settled(text), "{text}");
        }
    }

    #[test]
    fn fixed_as_an_adjective_is_not_a_closure() {
        // PHP site TD-20260828-inline-template-assets.
        for text in [
            "Shared and feature Twig templates contain fixed CSS declarations and executable JavaScript bodies.",
            "The header uses a fixed position and a fixed width.",
        ] {
            assert!(!declares_settled(text), "{text}");
        }
    }

    #[test]
    fn a_closure_in_history_is_not_the_state_of_the_finding() {
        for text in [
            // Node service resync row.
            "Kronn resync: 5 active TDs on file (previous outcome: N/A: not applicable — library/API-only project)",
            "It was resolved last year, then reopened after the rewrite.",
            "Previously mitigated by a WAF rule; the rule was removed.",
            "Fixed in 1.2, regressed in 1.4.",
        ] {
            assert!(!declares_settled(text), "{text}");
        }
    }

    #[test]
    fn a_clause_is_a_sentence_or_a_semicolon_part_not_a_table_cell() {
        let text = "| TD-a | Fixed | row | one. Next sentence; other part";
        let at = text.find("Fixed").unwrap();
        let clause = clause_bounds(text, at);
        assert_eq!(&text[clause], "| TD-a | Fixed | row | one");
        // A period inside a name or a number does not end it.
        let text = "see app.css:191 and 99.23% coverage, resolved";
        let clause = clause_bounds(text, text.find("resolved").unwrap());
        assert_eq!(clause, 0..text.len());
    }

    #[test]
    fn a_sheet_judges_its_status_not_a_history_it_has_moved_past() {
        let reopened = "---\nname: td-x\naudit_history:\n  - date: 2026-07-01\n    status: Fixed in source\n    reviewer: Full audit\n    note: fixed\n---\n\n# TD-x\n\n- **Status**: Open (reopened)\n";
        assert!(!sheet_declares_settled(reopened));
        let fixed = "---\nname: td-x\n---\n\n# TD-x\n\n**Status**: RESOLVED ✓\n";
        assert!(sheet_declares_settled(fixed));
        // No status of its own: the latest entry decides.
        let history = "---\naudit_history:\n  - date: 2026-07-01\n    status: Fixed in source\n    reviewer: Full audit\n    note: fixed\n  - date: 2026-08-01\n    status: Verified in source\n    reviewer: Security audit\n    note: still present\n---\n\n# TD-x\n";
        assert!(!sheet_declares_settled(history));
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
        // A finding's own location, in backticks, is not a `[src:]` proof…
        assert!(!cites_code("Fixed `app.css:191`"));
    }

    #[test]
    fn a_row_may_point_at_the_code_it_closed_with_an_inline_location() {
        // …but a closed row of the index that opens a line of code is located.
        for text in [
            "| **TD-x** | ✓ Fixed | Anchor links respond to `:focus-visible` `app.css:191` |",
            "| ✓ Fixed | `templates/menu/header.html.twig:60` |",
            "resolved `app.css:348-355`",
            "resolved `src/a.rs:12#retry_once`",
        ] {
            assert!(locates_code(text), "{text}");
        }
        for text in [
            "resolved, see `example.com:443`",
            "resolved `docs/guide.md:12`",
            "resolved `localhost:8080`",
            "resolved `app.css`",
            "resolved",
        ] {
            assert!(!locates_code(text), "{text}");
        }
    }

    #[test]
    fn an_anchored_or_dash_ranged_citation_is_proof() {
        for text in [
            "[src: file: src/a.rs:12#retry_once]",
            "[src: file: src/a.rs:12-20#Type.method]",
            "[src: file: assets/styles/app.css:299–304]",
            "[src: file: assets/styles/app.css:299—304]",
        ] {
            assert!(cites_code(text), "{text}");
        }
        // The anchor does not turn a page into code.
        assert!(!cites_code("[src: file: docs/a.md:3#section]"));
    }
}
