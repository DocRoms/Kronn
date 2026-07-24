//! Deterministic audit detectors (chantier 1, 2026-06-03).
//!
//! The dimension-coverage matrix (Step 8 § B) is the UNIVERSAL socle —
//! it forces the agent to *account* for every dimension, but it is a
//! declaration, not proof the scan happened. These detectors are the
//! ground-truth ANCHOR: cheap, mechanical, stack-specific scans of the
//! project SOURCE that surface signals an LLM single-pass commonly
//! misses (the DOCROMS_WEB audit missed all four below: 0-test gap,
//! `target="_blank"` without `rel=noopener`, CSP `unsafe-*`, missing
//! README/SECURITY).
//!
//! Contract:
//!   - Detectors NEVER auto-emit a TD. They produce `DetectedSignal`s
//!     injected into the Step 8 prompt. The agent decides whether each
//!     becomes a TD, is folded into a baseline note, or is justified
//!     away (with a verifiable reason in the coverage matrix).
//!   - This is COMPLEMENTARY to `core::anti_halluc`, which lints the
//!     audit OUTPUT for fabricated citations. Detectors scan the INPUT
//!     (the project), anti_halluc scans the output. Different surfaces.
//!   - Phase 1 ships web/PHP/JS-leaning detectors + generic packs.
//!     Adding a detector = one function + one line in [`run_detectors`].
//!     Coverage is intentionally honest: [`render_signals_block`] states
//!     which detectors ran so a silent gap never reads as "all clear".

use std::path::Path;
use walkdir::WalkDir;

/// Heavy dirs skipped by every detector walk. Mirrors
/// `scanner::scan_kronn_markers` so the two scans agree on scope.
static SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "target",
    ".git",
    "dist",
    "build",
    ".next",
    ".kronn",
    ".kronn-worktrees",
    ".venv",
    "__pycache__",
    "docs", // never scan Kronn's own output as if it were source
];

/// Severity of a detected signal. Ordered Critical > High > Medium > Low
/// for stable sorting in the injected block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    // Order matters: derive(Ord) ranks by declaration order, so declare
    // most-severe LAST and reverse-sort, OR declare most-severe FIRST.
    // We declare most-severe first and sort ascending → Critical leads.
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::High => "High",
            Severity::Medium => "Medium",
            Severity::Low => "Low",
        }
    }
}

/// One mechanical signal surfaced to Step 8. `evidence` is a concrete,
/// human-verifiable anchor (`file:line` or a count) so the agent can
/// confirm it instead of trusting the detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSignal {
    /// Stable kebab id (`zero-tests`, `blank-noopener`, …).
    pub detector_id: &'static str,
    /// Which of the 10 coverage dimensions this maps to.
    pub dimension: &'static str,
    pub severity: Severity,
    pub title: String,
    /// `path:line` or a count summary — always verifiable.
    pub evidence: String,
}

/// Source-file extensions counted by the test-gap detector.
static SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "php", "py", "go", "java", "kt", "rb", "cs", "c",
    "h", "cpp", "hpp", "cc", "swift", "scala",
];

/// Markup extensions scanned for `_blank` / CSP signals.
static MARKUP_EXTS: &[&str] = &[
    "html", "htm", "twig", "vue", "svelte", "jsx", "tsx", "php", "erb", "blade",
];

/// Per-detector cap on emitted signals so a pathological repo can't
/// flood the Step 8 prompt. When a detector truncates, it appends a
/// final summary signal so the omission is never silent.
const MAX_PER_DETECTOR: usize = 15;

/// Run every Phase-1 detector against `project_path` and return the
/// flattened, severity-sorted signal list.
pub fn run_detectors(project_path: &Path) -> Vec<DetectedSignal> {
    let mut out = Vec::new();
    out.extend(detect_zero_tests(project_path));
    out.extend(detect_blank_without_noopener(project_path));
    out.extend(detect_csp_unsafe(project_path));
    out.extend(detect_missing_community_files(project_path));
    // Stable order: severity first, then detector id, then evidence.
    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.detector_id.cmp(b.detector_id))
            .then_with(|| a.evidence.cmp(&b.evidence))
    });
    out
}

/// The list of detector ids that `run_detectors` runs, for the honest
/// "what was scanned" footer in [`render_signals_block`].
pub const DETECTOR_IDS: &[&str] = &[
    "zero-tests",
    "blank-noopener",
    "csp-unsafe",
    "missing-community-files",
];

// ─── Detector 1: test gap ────────────────────────────────────────────────

/// `test_count == 0 && source_count > MIN`. A non-trivial codebase with
/// zero discoverable test files. Signal, not verdict — the project may
/// keep tests in a sibling repo (the agent reconciles against the
/// briefing).
fn detect_zero_tests(root: &Path) -> Vec<DetectedSignal> {
    const MIN_SOURCES: usize = 5;
    let mut source_count = 0usize;
    let mut test_count = 0usize;
    for entry in walk(root) {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !SOURCE_EXTS.contains(&ext) {
            continue;
        }
        if is_test_path(path) {
            test_count += 1;
        } else {
            source_count += 1;
        }
    }
    if source_count >= MIN_SOURCES && test_count == 0 {
        vec![DetectedSignal {
            detector_id: "zero-tests",
            dimension: "Maintainability",
            severity: Severity::Medium,
            title: "No test files discovered for a non-trivial source tree".into(),
            evidence: format!(
                "{source_count} source files, 0 test files (no `*_test.*` / `*.test.*` / `*.spec.*` / `test_*.py` / `tests/` dir)"
            ),
        }]
    } else {
        Vec::new()
    }
}

/// Heuristic: is this path a test file? Covers the common per-language
/// conventions without language-specific config.
fn is_test_path(path: &Path) -> bool {
    // Directory-based conventions: match any path COMPONENT (so a
    // top-level `tests/` with no leading slash is caught too).
    static TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec", "specs", "testing"];
    let in_test_dir = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .any(|seg| TEST_DIRS.contains(&seg.to_lowercase().as_str()));
    if in_test_dir {
        return true;
    }
    let Some(orig) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // CamelCase convention (Java/C#/Kotlin): `UserServiceTest.java`,
    // `FooSpec.scala`. Checked on the ORIGINAL case so we don't match
    // "contest"/"latest"/"greatest" (which only end in lowercase "test").
    let orig_stem = orig.rsplit_once('.').map(|(s, _)| s).unwrap_or(orig);
    if orig_stem.ends_with("Test") || orig_stem.ends_with("Tests") || orig_stem.ends_with("Spec") {
        return true;
    }
    // snake / dotted conventions (lowercase-safe).
    let name = orig.to_lowercase();
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
    stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || name.starts_with("test_")
        || name.starts_with("test.")
}

// ─── Detector 2: target="_blank" without rel=noopener ────────────────────

/// `target="_blank"` on a tag lacking `rel=...noopener|noreferrer`.
/// Reverse-tabnabbing surface. Low severity (modern browsers imply
/// noopener for `_blank`) but a commonly-missed hygiene signal.
fn detect_blank_without_noopener(root: &Path) -> Vec<DetectedSignal> {
    let mut out = Vec::new();
    let mut truncated = 0usize;
    for entry in walk(root) {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !MARKUP_EXTS.contains(&ext) {
            continue;
        }
        // Skip test fixtures/snapshots: `<a target="_blank">` in a test
        // HTML is not a prod surface and would inject false positives
        // (Codex review 2026-06-03).
        if is_test_path(path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !content.contains("_blank") {
            continue;
        }
        for (idx, line) in content.lines().enumerate() {
            if !line_has_blank_without_noopener(line) {
                continue;
            }
            if out.len() >= MAX_PER_DETECTOR {
                truncated += 1;
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(path);
            out.push(DetectedSignal {
                detector_id: "blank-noopener",
                dimension: "Security",
                severity: Severity::Low,
                title: "`target=\"_blank\"` without `rel=\"noopener\"`".into(),
                evidence: format!("{}:{}", rel.display(), idx + 1),
            });
        }
    }
    if truncated > 0 {
        out.push(DetectedSignal {
            detector_id: "blank-noopener",
            dimension: "Security",
            severity: Severity::Low,
            title: "more `_blank` occurrences not shown (cap reached)".into(),
            evidence: format!("+{truncated} additional `target=\"_blank\"` hits beyond the first {MAX_PER_DETECTOR}"),
        });
    }
    out
}

/// True if the line opens/contains a `target="_blank"` (single or double
/// quotes) and does NOT carry a `rel=` with `noopener`/`noreferrer`.
/// Line-scoped: misses tags split across lines (rare); documented limit.
fn line_has_blank_without_noopener(line: &str) -> bool {
    let l = line.to_lowercase();
    let has_blank = l.contains("target=\"_blank\"")
        || l.contains("target='_blank'")
        || l.contains("target=_blank");
    if !has_blank {
        return false;
    }
    // Look for a rel attribute carrying a safe token on the same line.
    if let Some(rel_pos) = l.find("rel=") {
        let after = &l[rel_pos..];
        // Grab the attribute value up to the next quote-close (cheap).
        if after.contains("noopener") || after.contains("noreferrer") {
            return false;
        }
    }
    true
}

// ─── Detector 3: CSP unsafe-inline / unsafe-eval ─────────────────────────

/// Content-Security-Policy directives weakened with `unsafe-inline` or
/// `unsafe-eval`. Scans all text source (the CSP can live in a header
/// subscriber, middleware, nginx conf, meta tag, …).
fn detect_csp_unsafe(root: &Path) -> Vec<DetectedSignal> {
    static CSP_EXTS: &[&str] = &[
        "php", "rs", "ts", "js", "py", "go", "rb", "java", "conf", "html", "htm", "twig", "yml",
        "yaml", "json", "ini", "nginx",
    ];
    let mut out = Vec::new();
    let mut truncated = 0usize;
    for entry in walk(root) {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !CSP_EXTS.contains(&ext) {
            continue;
        }
        // Skip test fixtures: CSP sample strings in tests are not the prod
        // policy (Codex review 2026-06-03).
        if is_test_path(path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !content.contains("unsafe-inline") && !content.contains("unsafe-eval") {
            continue;
        }
        for (idx, line) in content.lines().enumerate() {
            let tok = if line.contains("unsafe-inline") {
                "unsafe-inline"
            } else if line.contains("unsafe-eval") {
                "unsafe-eval"
            } else {
                continue;
            };
            if out.len() >= MAX_PER_DETECTOR {
                truncated += 1;
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(path);
            out.push(DetectedSignal {
                detector_id: "csp-unsafe",
                dimension: "Security",
                severity: Severity::Medium,
                title: format!("CSP weakened with `{tok}`"),
                evidence: format!("{}:{}", rel.display(), idx + 1),
            });
        }
    }
    if truncated > 0 {
        out.push(DetectedSignal {
            detector_id: "csp-unsafe",
            dimension: "Security",
            severity: Severity::Medium,
            title: "more CSP `unsafe-*` occurrences not shown (cap reached)".into(),
            evidence: format!("+{truncated} additional hits beyond the first {MAX_PER_DETECTOR}"),
        });
    }
    out
}

// ─── Detector 4: missing community files ─────────────────────────────────

/// README.md / SECURITY.md absent at the repo root. Low-severity
/// documentation/compliance signal; the agent decides relevance (a
/// private proprietary repo may intentionally omit both).
fn detect_missing_community_files(root: &Path) -> Vec<DetectedSignal> {
    let mut out = Vec::new();
    if !root_has_file(root, "readme.md") {
        out.push(DetectedSignal {
            detector_id: "missing-community-files",
            dimension: "Documentation drift",
            severity: Severity::Low,
            title: "No README.md at repo root".into(),
            evidence: "root listing has no `README.md` (case-insensitive)".into(),
        });
    }
    if !root_has_file(root, "security.md") && !root_has_file(root, ".github/security.md") {
        out.push(DetectedSignal {
            detector_id: "missing-community-files",
            dimension: "Security",
            severity: Severity::Low,
            title: "No SECURITY.md (no vulnerability-disclosure policy)".into(),
            evidence: "neither `SECURITY.md` nor `.github/SECURITY.md` present".into(),
        });
    }
    out
}

/// Case-insensitive check for a file at a root-relative path.
fn root_has_file(root: &Path, rel_lower: &str) -> bool {
    // Split into components and resolve each case-insensitively so
    // `Readme.md` / `.github/Security.md` match.
    let mut cur = root.to_path_buf();
    let parts: Vec<&str> = rel_lower.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let Ok(entries) = std::fs::read_dir(&cur) else {
            return false;
        };
        let want_dir = i < parts.len() - 1;
        let mut matched = None;
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.eq_ignore_ascii_case(part) {
                let is_dir = e.path().is_dir();
                if is_dir == want_dir {
                    matched = Some(e.path());
                    break;
                }
            }
        }
        match matched {
            Some(p) => cur = p,
            None => return false,
        }
    }
    true
}

// ─── Disposition gate (chantier 1b) ──────────────────────────────────────

/// The output surface anchor we look for to decide a signal was "disposed"
/// (addressed) by Step 8. For a `file:line` signal it's the file path (line
/// numbers drift; the path is stable); for a count/other signal it's a
/// detector-specific keyword. `None` for synthetic truncation markers
/// (`evidence` starting with `+`) — those are not gated.
fn disposition_anchor(sig: &DetectedSignal) -> Option<String> {
    if sig.evidence.starts_with('+') {
        return None; // "+N more …" truncation summary, not a real anchor
    }
    // `path:line` → file path (strip the trailing `:digits`).
    if let Some((path, line)) = sig.evidence.rsplit_once(':') {
        if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) {
            return Some(path.to_lowercase());
        }
    }
    // Count / structural signals: fall back to a stable keyword.
    let kw = match sig.detector_id {
        "zero-tests" => "test",
        "missing-community-files" => {
            if sig.title.to_lowercase().contains("readme") {
                "readme"
            } else {
                "security"
            }
        }
        _ => return Some(sig.title.to_lowercase()),
    };
    Some(kw.to_string())
}

/// Given the signals injected into Step 8 and the COMBINED Step-8 output
/// (the tech-debt index + every `TD-*.md` detail file), return the signals
/// that show NO sign of having been addressed. Deduped by anchor so three
/// `_blank` hits in one file report once.
///
/// LENIENT BY DESIGN: matching is file-level / keyword-level, so it catches
/// total OMISSIONS (a flagged file never mentioned anywhere) — not
/// mischaracterizations (a file cited but described wrongly; that's a
/// semantic judgement beyond a mechanical gate). Leniency is deliberate: a
/// too-strict gate would false-fail and trap Step 8 in a re-run loop (cf.
/// the 2026-06-03 exact-match incident).
pub fn undisposed_signals<'a>(
    signals: &'a [DetectedSignal],
    combined_output: &str,
) -> Vec<&'a DetectedSignal> {
    let haystack = combined_output.to_lowercase();
    let mut seen_anchors = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sig in signals {
        let Some(anchor) = disposition_anchor(sig) else {
            continue;
        };
        if !seen_anchors.insert(anchor.clone()) {
            continue; // already accounted for this file/keyword
        }
        if !haystack.contains(&anchor) {
            out.push(sig);
        }
    }
    out
}

// ─── Rendering ───────────────────────────────────────────────────────────

/// Markdown block injected into the Step 8 prompt. Frames the signals
/// as anchors the agent MUST account for (emit a TD, fold into baseline,
/// or justify in the coverage matrix) and names every detector that ran
/// so an empty result reads as "scanned, clean" — never as "skipped".
pub fn render_signals_block(signals: &[DetectedSignal]) -> String {
    let mut out = String::new();
    out.push_str("## Deterministic detector signals (ground-truth anchors)\n\n");
    out.push_str(
        "Kronn ran cheap mechanical detectors over the project source BEFORE this step. \
Each signal below is a verifiable anchor (`file:line` / count). **For every signal you MUST do ONE of:** \
(a) emit/refresh a TD, (b) fold it into a baseline-checklist note, or (c) justify dismissal in the \
`## Dimension coverage` matrix with a verifiable reason. A signal silently ignored = **incomplete audit**. \
These detectors do NOT replace your own scan — they only anchor the dimensions they cover.\n\n",
    );

    if signals.is_empty() {
        out.push_str("**No signals fired.** All Phase-1 detectors ran and found nothing.\n\n");
    } else {
        out.push_str("| Severity | Dimension | Signal | Evidence |\n");
        out.push_str("|----------|-----------|--------|----------|\n");
        for s in signals {
            out.push_str(&format!(
                "| {} | {} | {} | `{}` |\n",
                s.severity.label(),
                s.dimension,
                s.title,
                s.evidence
            ));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "_Detectors run this audit: {}. A dimension not anchored by any detector still requires your manual scan + a matrix row._\n",
        DETECTOR_IDS.join(", ")
    ));
    out
}

// ─── Internals ───────────────────────────────────────────────────────────

/// Shared bounded, skip-aware walk. Files only; depth-capped; heavy dirs
/// pruned. Matches `scanner::scan_kronn_markers` conventions.
fn walk(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            e.file_name()
                .to_str()
                .map(|name| !SKIP_DIRS.contains(&name))
                .unwrap_or(true)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn proj() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }
    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    // ── zero-tests ──
    #[test]
    fn zero_tests_fires_when_sources_present_no_tests() {
        let t = proj();
        for i in 0..6 {
            write(t.path(), &format!("src/mod{i}.rs"), "fn x() {}");
        }
        let s = detect_zero_tests(t.path());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detector_id, "zero-tests");
        assert!(s[0].evidence.contains("6 source files"));
    }

    #[test]
    fn zero_tests_silent_when_a_test_file_exists() {
        let t = proj();
        for i in 0..6 {
            write(t.path(), &format!("src/mod{i}.rs"), "fn x() {}");
        }
        write(t.path(), "tests/it.rs", "#[test] fn t() {}");
        assert!(
            detect_zero_tests(t.path()).is_empty(),
            "a tests/ file must silence the detector"
        );
    }

    #[test]
    fn zero_tests_silent_below_threshold() {
        let t = proj();
        write(t.path(), "src/main.rs", "fn main() {}");
        assert!(
            detect_zero_tests(t.path()).is_empty(),
            "trivial repo must not fire"
        );
    }

    #[test]
    fn is_test_path_recognizes_conventions() {
        assert!(is_test_path(Path::new("src/foo_test.go")));
        assert!(is_test_path(Path::new("a/b/Foo.test.ts")));
        assert!(is_test_path(Path::new("pkg/bar.spec.js")));
        assert!(is_test_path(Path::new("tests/integration.rs")));
        assert!(is_test_path(Path::new("app/__tests__/x.tsx")));
        assert!(is_test_path(Path::new("test_thing.py")));
        assert!(is_test_path(Path::new("src/UserServiceTest.java")));
        assert!(!is_test_path(Path::new("src/user_service.rs")));
        assert!(
            !is_test_path(Path::new("src/contest.rs")),
            "contest is not a test file"
        );
    }

    // ── _blank / noopener ──
    #[test]
    fn blank_without_noopener_fires() {
        let t = proj();
        write(
            t.path(),
            "templates/page.twig",
            "<a href=\"x\" target=\"_blank\">link</a>",
        );
        let s = detect_blank_without_noopener(t.path());
        assert_eq!(s.len(), 1);
        assert!(s[0].evidence.ends_with(":1"));
        assert_eq!(s[0].dimension, "Security");
    }

    #[test]
    fn blank_with_noopener_is_silent() {
        let t = proj();
        write(
            t.path(),
            "a.html",
            "<a target=\"_blank\" rel=\"noopener noreferrer\">ok</a>",
        );
        assert!(detect_blank_without_noopener(t.path()).is_empty());
    }

    #[test]
    fn blank_with_noreferrer_only_is_silent() {
        let t = proj();
        write(
            t.path(),
            "a.html",
            "<a rel=\"noreferrer\" target=\"_blank\">ok</a>",
        );
        assert!(detect_blank_without_noopener(t.path()).is_empty());
    }

    #[test]
    fn line_has_blank_helper_matches_quote_variants() {
        assert!(line_has_blank_without_noopener("<a target=\"_blank\">"));
        assert!(line_has_blank_without_noopener("<a target='_blank'>"));
        assert!(!line_has_blank_without_noopener(
            "<a target=\"_blank\" rel=\"noopener\">"
        ));
        assert!(
            !line_has_blank_without_noopener("<a href=\"_blanket\">"),
            "substring _blank in _blanket alone must not match without target="
        );
    }

    #[test]
    fn blank_in_test_fixture_is_skipped() {
        // Codex review: `<a target="_blank">` inside a test fixture is not a
        // prod surface and must not produce a signal.
        let t = proj();
        write(
            t.path(),
            "tests/fixtures/page.html",
            "<a target=\"_blank\">x</a>",
        );
        write(
            t.path(),
            "src/__tests__/snap.tsx",
            "<a target=\"_blank\">y</a>",
        );
        assert!(
            detect_blank_without_noopener(t.path()).is_empty(),
            "test-fixture _blank must be skipped"
        );
    }

    // ── CSP ──
    #[test]
    fn csp_unsafe_inline_and_eval_fire_with_file_line() {
        let t = proj();
        write(
            t.path(),
            "src/Headers.php",
            "line1\n$csp = \"default-src 'self'; script-src 'unsafe-inline'\";\nfoo\n$x='unsafe-eval';\n",
        );
        let s = detect_csp_unsafe(t.path());
        assert_eq!(
            s.len(),
            2,
            "one for unsafe-inline (line 2), one for unsafe-eval (line 4)"
        );
        assert!(s.iter().any(|x| x.evidence.ends_with(":2")));
        assert!(s.iter().any(|x| x.evidence.ends_with(":4")));
    }

    #[test]
    fn csp_unsafe_in_test_file_is_skipped() {
        // CSP sample strings in tests are not the prod policy (Codex review).
        let t = proj();
        write(
            t.path(),
            "tests/csp_spec.ts",
            "const p = \"script-src 'unsafe-inline'\";",
        );
        write(t.path(), "src/headers_test.php", "$c=\"'unsafe-eval'\";");
        assert!(
            detect_csp_unsafe(t.path()).is_empty(),
            "test-file CSP samples must be skipped"
        );
    }

    #[test]
    fn csp_clean_policy_is_silent() {
        let t = proj();
        write(
            t.path(),
            "nginx.conf",
            "add_header Content-Security-Policy \"default-src 'self'\";",
        );
        assert!(detect_csp_unsafe(t.path()).is_empty());
    }

    // ── community files ──
    #[test]
    fn missing_readme_and_security_both_fire_on_empty_repo() {
        let t = proj();
        write(t.path(), "src/main.rs", "fn main(){}");
        let s = detect_missing_community_files(t.path());
        assert_eq!(s.len(), 2);
        assert!(s.iter().any(|x| x.title.contains("README")));
        assert!(s.iter().any(|x| x.title.contains("SECURITY")));
    }

    #[test]
    fn present_readme_case_insensitive_silences_it() {
        let t = proj();
        write(t.path(), "ReadMe.md", "# hi");
        write(t.path(), ".github/SECURITY.md", "# policy");
        let s = detect_missing_community_files(t.path());
        assert!(
            s.is_empty(),
            "case-insensitive README + .github/SECURITY must both be found: {s:?}"
        );
    }

    // ── orchestration + render ──
    #[test]
    fn run_detectors_sorts_by_severity_then_id() {
        let t = proj();
        // CSP (Medium) + _blank (Low) + missing community (Low).
        write(t.path(), "a.html", "<a target=\"_blank\">x</a>");
        write(t.path(), "h.php", "$c=\"script-src 'unsafe-inline'\";");
        let s = run_detectors(t.path());
        assert!(!s.is_empty());
        // First must be the highest severity present (Medium CSP).
        assert_eq!(s[0].severity, Severity::Medium);
        // Severities are non-decreasing.
        for w in s.windows(2) {
            assert!(
                w[0].severity <= w[1].severity,
                "must be severity-sorted: {s:?}"
            );
        }
    }

    #[test]
    fn render_block_lists_signals_and_names_detectors() {
        let sig = vec![DetectedSignal {
            detector_id: "csp-unsafe",
            dimension: "Security",
            severity: Severity::Medium,
            title: "CSP weakened with `unsafe-inline`".into(),
            evidence: "src/H.php:2".into(),
        }];
        let block = render_signals_block(&sig);
        assert!(block.contains("Deterministic detector signals"));
        assert!(block.contains("src/H.php:2"));
        assert!(block.contains("unsafe-inline"));
        assert!(block.contains("incomplete audit"));
        // Honest footer names all detectors that ran.
        for id in DETECTOR_IDS {
            assert!(block.contains(id), "footer must name detector {id}");
        }
    }

    // ── disposition gate (chantier 1b) ──
    fn sig(
        id: &'static str,
        dim: &'static str,
        sev: Severity,
        title: &str,
        ev: &str,
    ) -> DetectedSignal {
        DetectedSignal {
            detector_id: id,
            dimension: dim,
            severity: sev,
            title: title.into(),
            evidence: ev.into(),
        }
    }

    #[test]
    fn undisposed_flags_a_file_never_mentioned() {
        let signals = vec![
            sig(
                "blank-noopener",
                "Security",
                Severity::Low,
                "_blank w/o noopener",
                "templates/pages/projets.html.twig:12",
            ),
            sig(
                "csp-unsafe",
                "Security",
                Severity::Medium,
                "CSP unsafe-eval",
                "src/EventSubscriber/HeadersSubscriber.php:87",
            ),
        ];
        // Output mentions the CSP file but NOT projets.html.twig.
        let output = "## Baseline\nCSP in src/EventSubscriber/HeadersSubscriber.php:61 verified.\n";
        let un = undisposed_signals(&signals, output);
        assert_eq!(un.len(), 1, "only the unmentioned file is undisposed");
        assert_eq!(un[0].detector_id, "blank-noopener");
    }

    #[test]
    fn undisposed_empty_when_all_files_cited() {
        let signals = vec![
            sig(
                "blank-noopener",
                "Security",
                Severity::Low,
                "x",
                "a/b/projets.html.twig:12",
            ),
            sig(
                "csp-unsafe",
                "Security",
                Severity::Medium,
                "x",
                "src/Headers.php:87",
            ),
        ];
        let output = "TD cites a/b/projets.html.twig and src/Headers.php both.";
        assert!(undisposed_signals(&signals, output).is_empty());
    }

    #[test]
    fn undisposed_dedupes_same_file() {
        // Three _blank hits in one file → one anchor → reported once if missing.
        let signals = vec![
            sig(
                "blank-noopener",
                "Security",
                Severity::Low,
                "x",
                "p/page.twig:1",
            ),
            sig(
                "blank-noopener",
                "Security",
                Severity::Low,
                "x",
                "p/page.twig:9",
            ),
            sig(
                "blank-noopener",
                "Security",
                Severity::Low,
                "x",
                "p/page.twig:20",
            ),
        ];
        let un = undisposed_signals(&signals, "nothing relevant here");
        assert_eq!(un.len(), 1, "same file deduped to one undisposed entry");
    }

    #[test]
    fn undisposed_count_signals_use_keyword() {
        let signals = vec![
            sig(
                "zero-tests",
                "Maintainability",
                Severity::Medium,
                "no tests",
                "14 source files, 0 test files",
            ),
            sig(
                "missing-community-files",
                "Documentation drift",
                Severity::Low,
                "No README.md at repo root",
                "root listing has no README",
            ),
        ];
        // Output addresses tests but not README.
        let output = "Maintainability: zero committed tests is a documented gap.";
        let un = undisposed_signals(&signals, output);
        assert_eq!(un.len(), 1);
        assert!(un[0].title.contains("README"));
    }

    #[test]
    fn undisposed_skips_truncation_markers() {
        let signals = vec![sig(
            "blank-noopener",
            "Security",
            Severity::Low,
            "more …",
            "+7 additional hits",
        )];
        assert!(
            undisposed_signals(&signals, "").is_empty(),
            "+N truncation markers are not gated"
        );
    }

    #[test]
    fn render_block_empty_states_scanned_clean() {
        let block = render_signals_block(&[]);
        assert!(block.contains("No signals fired"));
        assert!(block.contains("All Phase-1 detectors ran"));
    }

    #[test]
    fn walk_skips_heavy_dirs_and_docs() {
        let t = proj();
        write(t.path(), "src/a.rs", "fn a(){}");
        write(t.path(), "node_modules/dep/index.js", "x");
        write(t.path(), "docs/inconsistencies-tech-debt.md", "TD");
        let files: Vec<_> = walk(t.path()).map(|e| e.path().to_path_buf()).collect();
        assert!(files.iter().any(|p| p.ends_with("src/a.rs")));
        assert!(!files
            .iter()
            .any(|p| p.to_string_lossy().contains("node_modules")));
        assert!(!files.iter().any(|p| p.to_string_lossy().contains("docs/")));
    }
}
