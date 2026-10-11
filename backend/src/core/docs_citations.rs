//! CI check of the `[src: file:]` citations in Kronn's own docs (KT-1059).
//!
//! Stricter than the runtime verifier it reuses: a doc citation must name a
//! regular file by its path from the repo root (no basename or suffix walk, no
//! absolute path) and carry a line or range that exists. Only a binary file
//! may be cited without a line. Fenced code and placeholder examples are skipped.

use crate::core::anti_halluc::{
    file_citation, is_binary_file, line_bounds_status, markers_at, partition_inline_code,
    resolve_in_root, scan_fences, strip_inline_code, Jailed, SourceStatus,
};
use std::path::{Path, PathBuf};

/// Doc trees checked, relative to the repo root.
pub const DOC_ROOTS: &[&str] = &["docs", "backend/docs"];
/// Historical release notes describe the code of their time and are not repaired.
pub const EXCLUDED: &[&str] = &["docs/releases"];

/// One citation of a doc that does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocCitationFailure {
    /// 1-based line of the doc holding the marker's `[`.
    pub line: usize,
    /// The marker's inner text, after `src:`.
    pub citation: String,
    pub reason: String,
}

/// A syntax example such as `<path>:<line>` or `src/{a,b}.rs`, not a citation.
fn is_placeholder(citation: &str) -> bool {
    citation.contains(['<', '>', '{', '}', '*', '…']) || citation.contains("...")
}

/// `None` when the citation holds, else why it does not.
fn check_reference(path: &str, lines: Option<(usize, usize)>, root: &Path) -> Option<String> {
    let rel = Path::new(path);
    if rel.has_root() {
        return Some("absolute path: cite it relative to the repo root".into());
    }
    let found = match resolve_in_root(root, rel) {
        Jailed::Inside(found) => found,
        Jailed::Missing => return Some(format!("file not found: {path}")),
        Jailed::Escapes => return Some("path leaves the repo root".into()),
    };
    if !found.is_file() {
        return Some(format!("not a file: {path}"));
    }
    match lines {
        None if is_binary_file(&found) => None,
        None => Some("no line: cite `path:line` or `path:start-end`".into()),
        Some(span) => match line_bounds_status(&found, Some(span)) {
            (SourceStatus::Verified, _) => None,
            (_, detail) => Some(detail),
        },
    }
}

/// Every failing `file:` citation of one doc, prose and backticked alike.
pub fn check_doc(text: &str, root: &Path) -> Vec<DocCitationFailure> {
    let fences = scan_fences(text, &['`'], false);
    let visible: Vec<&str> = text
        .lines()
        .zip(&fences.in_fence)
        .map(|(line, fenced)| if *fenced { "" } else { line })
        .collect();

    let mut markers: Vec<(usize, String)> = Vec::new();
    // Inline-code stripping keeps one output line per input line.
    let prose = strip_inline_code(&visible.join("\n"));
    let mut line_of = Vec::with_capacity(prose.len());
    let mut line = 1usize;
    for ch in prose.chars() {
        line_of.push(line);
        if ch == '\n' {
            line += 1;
        }
    }
    for (offset, inner) in markers_at(&prose) {
        markers.push((line_of[offset], inner));
    }
    for (index, text_line) in visible.iter().enumerate() {
        if text_line.contains('`') {
            let quoted = partition_inline_code(text_line).1;
            markers.extend(markers_at(&quoted).into_iter().map(|(_, m)| (index + 1, m)));
        }
    }
    markers.sort_by_key(|(line, _)| *line);

    markers
        .into_iter()
        .filter_map(|(line, citation)| {
            // On the raw text: the path cleanup peels `<`/`>` off `<REAL_FILE>`.
            if is_placeholder(&citation) {
                return None;
            }
            let (path, lines) = file_citation(&citation)?;
            let reason = check_reference(&path, lines, root)?;
            Some(DocCitationFailure {
                line,
                citation,
                reason,
            })
        })
        .collect()
}

fn collect_docs(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if EXCLUDED.iter().any(|ex| rel == Path::new(ex)) {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_docs(&path, root, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "md") => {
                out.push(rel.to_path_buf())
            }
            _ => {}
        }
    }
}

/// Every failing citation of the checked doc trees, as `(doc, failure)`.
pub fn check_corpus(root: &Path) -> Vec<(PathBuf, DocCitationFailure)> {
    let mut docs = Vec::new();
    for dir in DOC_ROOTS {
        collect_docs(&root.join(dir), root, &mut docs);
    }
    docs.sort();
    let mut failures = Vec::new();
    for doc in docs {
        let Ok(text) = std::fs::read_to_string(root.join(&doc)) else {
            continue;
        };
        for failure in check_doc(&text, root) {
            failures.push((doc.clone(), failure));
        }
    }
    failures
}

/// One line per failure: `doc:line  [src: …]  reason`.
pub fn report(failures: &[(PathBuf, DocCitationFailure)]) -> String {
    failures
        .iter()
        .map(|(doc, f)| {
            format!(
                "{}:{}  [src: {}]  {}\n",
                doc.display(),
                f.line,
                f.citation,
                f.reason
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0body").unwrap();
        tmp
    }

    fn reasons(doc: &str, root: &Path) -> Vec<String> {
        check_doc(doc, root).into_iter().map(|f| f.reason).collect()
    }

    #[test]
    fn resolving_citations_pass() {
        let tmp = repo();
        let doc = "A [src: file: src/a.rs:2] B `[src: file: src/a.rs:1-3]`\n\
                   C [src: file: logo.png] D [src: url: https://x.y] E [src: user: 2026-10-11: ok]\n";
        assert_eq!(check_doc(doc, tmp.path()), vec![]);
    }

    #[test]
    fn missing_file_out_of_bounds_and_lineless_fail() {
        let tmp = repo();
        let root = tmp.path();
        assert_eq!(
            reasons("[src: file: src/gone.rs:1]", root),
            ["file not found: src/gone.rs"]
        );
        assert_eq!(
            reasons("[src: file: src/a.rs:2-4]", root),
            ["line 4 beyond file length 3"]
        );
        assert_eq!(
            reasons("[src: file: src/a.rs]", root),
            ["no line: cite `path:line` or `path:start-end`"]
        );
    }

    #[test]
    fn the_last_line_passes_and_the_next_one_fails() {
        let tmp = repo();
        assert!(reasons("[src: file: src/a.rs:3]", tmp.path()).is_empty());
        assert_eq!(reasons("[src: file: src/a.rs:4]", tmp.path()).len(), 1);
    }

    #[test]
    fn directories_absolute_paths_escapes_and_bare_names_fail() {
        let tmp = repo();
        let root = tmp.path();
        assert_eq!(
            reasons("[src: file: src/sub:1]", root),
            ["not a file: src/sub"]
        );
        let abs = format!("[src: file: {}:1]", root.join("src/a.rs").display());
        assert_eq!(
            reasons(&abs, root),
            ["absolute path: cite it relative to the repo root"]
        );
        assert_eq!(
            reasons("[src: file: ../a.rs:1]", root),
            ["path leaves the repo root"]
        );
        // The runtime verifier walks for a unique basename; docs must not rely on it.
        assert_eq!(
            reasons("[src: file: a.rs:1]", root),
            ["file not found: a.rs"]
        );
    }

    #[test]
    fn a_backticked_citation_is_checked() {
        let tmp = repo();
        let failures = check_doc("x\n`[src: file: src/a.rs:9]`\n", tmp.path());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].line, 2);
    }

    #[test]
    fn fenced_blocks_and_placeholders_are_skipped() {
        let tmp = repo();
        let doc = "```md\n[src: file: src/gone.rs:1]\n```\n\
                   `[src: file: <path>:<line>]` [src: file: <path>:<start-end>]\n\
                   `[src: file: src/{a,b}.rs:1]` `[src: file: .../x.rs:1]` [src: file: <REAL_FILE>:1]\n";
        assert_eq!(check_doc(doc, tmp.path()), vec![]);
    }

    #[test]
    fn failures_carry_the_doc_line_and_the_marker() {
        let tmp = repo();
        let doc = "intro\n\n```\ncode\n```\nsee [src:\nfile: src/a.rs:7] and `x`\n";
        let failures = check_doc(doc, tmp.path());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].line, 6);
        assert_eq!(failures[0].citation, "file: src/a.rs:7");
        let text = report(&[(PathBuf::from("docs/x.md"), failures[0].clone())]);
        assert_eq!(
            text,
            "docs/x.md:6  [src: file: src/a.rs:7]  line 7 beyond file length 3\n"
        );
    }

    #[test]
    fn the_corpus_walk_skips_release_notes() {
        let tmp = repo();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/releases")).unwrap();
        std::fs::create_dir_all(root.join("backend/docs/conventions")).unwrap();
        std::fs::write(root.join("docs/releases/0.1.md"), "[src: file: gone.rs:1]").unwrap();
        std::fs::write(root.join("docs/a.md"), "[src: file: src/a.rs:1]").unwrap();
        std::fs::write(
            root.join("backend/docs/conventions/b.md"),
            "ok\n[src: file: src/gone.rs:1]",
        )
        .unwrap();
        let failures = check_corpus(root);
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].0,
            PathBuf::from("backend/docs/conventions/b.md")
        );
        assert_eq!(failures[0].1.line, 2);
    }

    /// The gate itself: every citation in the repo's docs resolves.
    #[test]
    fn docs_citations_resolve_in_the_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let failures = check_corpus(root);
        assert!(
            failures.is_empty(),
            "{} doc citation(s) do not resolve; fix the path or line, or rewrite the claim:\n{}",
            failures.len(),
            report(&failures)
        );
    }
}
