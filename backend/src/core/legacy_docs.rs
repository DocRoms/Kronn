//! Pre-audit migration of user-curated `docs/` into `docs/legacy/`.
//!
//! Problem solved: a project bootstrapped to Kronn for the FIRST time
//! often already has a hand-curated `docs/` folder (installation
//! guides, ADRs, internal onboarding, etc.). The Kronn audit installs
//! template files alongside but never READS the pre-existing
//! content — so the AI fills the Kronn templates from `README.md` /
//! source code only, ignoring 6 months of human knowledge.
//!
//! This module preserves the existing content by moving it under
//! `docs/legacy/<original-path>` BEFORE templates install. The audit
//! pipeline then includes a prompt instruction to read
//! `docs/legacy/**/*.md` as the PRIMARY source of truth when filling
//! the fresh Kronn templates.
//!
//! Detection — the migration is **idempotent + non-destructive on
//! Kronn-managed projects**:
//!   - No `docs/` directory → nothing to migrate, normal flow
//!   - `docs/AGENTS.md` exists AND starts with the Kronn signature
//!     `# AI agent context — Entry point` → already-Kronn-managed, no
//!     migration (re-audits stay fast + safe)
//!   - Else → user-managed legacy `docs/`, migrate everything except
//!     `docs/legacy/` itself (don't recurse into our destination)
//!
//! The signature check covers both fresh template installs (verbatim
//! line) and audit-filled files (first line is preserved per the
//! prompt's "keep file structure" rule).
//!
//! See companion entry [[project_legacy_docs_migration_0_8_3]] in user
//! memory and the audit Phase 1 caller (`api/audit/full.rs`).

use std::fs;
use std::path::{Path, PathBuf};

/// Kronn-template signature line. Present in the template AGENTS.md
/// and preserved by audit fills (the agent only replaces
/// `{{PLACEHOLDERS}}`, never the header). Stable across 0.7+ — if we
/// change the header line in a future template revision, ALSO add the
/// old line here so older bootstrapped projects keep being detected.
const KRONN_SIGNATURE_LINES: &[&str] = &[
    "# AI agent context — Entry point",
];

/// Names directly under `docs/` that the migration MUST NOT move:
///   - `legacy` — our own destination; recursing in would shuffle
///     forever.
///   - `var` — gitignored runtime data (no semantic value to preserve
///     in legacy/; also pre-Kronn projects don't have this name).
const PROTECTED_TOP_LEVEL: &[&str] = &["legacy", "var"];

/// Outcome of a migration call — surfaced to the SSE stream so the
/// frontend can render a toast + a list of what was moved.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LegacyMigrationReport {
    /// True when migration ran (user-managed docs/ detected) — false
    /// when skipped (no docs/, or already Kronn-managed).
    pub migrated: bool,
    /// Reason for the skip when `migrated == false`. Empty string when
    /// migration ran. Helps the frontend show a meaningful message.
    pub skip_reason: String,
    /// Top-level entries (file or directory names, relative to
    /// `docs/`) that were moved into `docs/legacy/`. Up to 50 entries
    /// surfaced; the count is the full total.
    pub moved_entries: Vec<String>,
    /// Total number of top-level entries moved. Always equals
    /// `moved_entries.len()` today but is a separate field so future
    /// truncation (when the list is huge) doesn't lose the count.
    pub moved_count: usize,
}

impl LegacyMigrationReport {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            migrated: false,
            skip_reason: reason.into(),
            moved_entries: vec![],
            moved_count: 0,
        }
    }
}

/// Detect whether the docs dir is already Kronn-managed. Returns
/// `true` when `docs/AGENTS.md` exists and its first non-empty line
/// matches a known Kronn signature. False otherwise (including when
/// `docs/AGENTS.md` is missing entirely).
pub fn is_kronn_managed_docs(docs_dir: &Path) -> bool {
    let agents = docs_dir.join("AGENTS.md");
    let Ok(content) = fs::read_to_string(&agents) else { return false; };
    let first_line = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    KRONN_SIGNATURE_LINES.iter().any(|s| first_line.trim() == *s)
}

/// Run the migration on the given `docs/` directory. Idempotent: a
/// second call after a successful first migration returns
/// `skip_reason = "already-kronn-managed"` because the freshly-
/// installed template AGENTS.md (laid down by `copy_dir_nondestructive`
/// AFTER this function returns) carries the Kronn signature on its
/// first non-empty line.
///
/// Caller must run this BEFORE the template install — running AFTER
/// would move the freshly-installed Kronn templates into legacy/ on
/// every audit.
pub fn migrate_user_docs_to_legacy(docs_dir: &Path) -> std::io::Result<LegacyMigrationReport> {
    if !docs_dir.exists() {
        return Ok(LegacyMigrationReport::skipped("no-docs-dir"));
    }
    if !docs_dir.is_dir() {
        return Ok(LegacyMigrationReport::skipped("docs-not-a-directory"));
    }
    if is_kronn_managed_docs(docs_dir) {
        return Ok(LegacyMigrationReport::skipped("already-kronn-managed"));
    }

    // Gather candidate top-level entries to move. Anything NOT in
    // `PROTECTED_TOP_LEVEL` is fair game — files AND directories.
    // Reading the dir once into a Vec before any move so the iterator
    // doesn't observe its own mutations (filesystem readdir semantics
    // vary across platforms).
    let mut to_move: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(docs_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if PROTECTED_TOP_LEVEL.iter().any(|p| name_str == *p) {
            continue;
        }
        to_move.push(entry.path());
    }

    if to_move.is_empty() {
        return Ok(LegacyMigrationReport::skipped("nothing-to-migrate"));
    }

    let legacy_dir = docs_dir.join("legacy");
    fs::create_dir_all(&legacy_dir)?;

    let mut moved_entries: Vec<String> = Vec::new();
    for src in &to_move {
        let file_name = match src.file_name() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        let dst = legacy_dir.join(&file_name);
        // If a destination already exists from a partial prior run,
        // append a numeric suffix so we don't clobber. Cheap, rare
        // case (truly partial migration after a crash).
        let dst_final = unique_destination(&dst);
        fs::rename(src, &dst_final)?;
        moved_entries.push(file_name.to_string_lossy().to_string());
    }

    // 0.8.3 (#273) — write a navigational README inside docs/legacy/
    // so a future agent (or the user revisiting weeks later) lands on
    // an explanation of why this folder exists and what to do with
    // it. Token cost = 0 (deterministic write). Idempotent : if the
    // user has hand-edited the file post-audit, we don't clobber.
    write_legacy_readme_if_absent(&legacy_dir)?;

    let moved_count = moved_entries.len();
    // Cap the surfaced list to keep SSE payloads small on legacy
    // dumps with hundreds of files. moved_count keeps the full total.
    let surfaced = if moved_entries.len() > 50 {
        moved_entries.truncate(50);
        moved_entries
    } else {
        moved_entries
    };

    Ok(LegacyMigrationReport {
        migrated: true,
        skip_reason: String::new(),
        moved_entries: surfaced,
        moved_count,
    })
}

/// Body of the navigational README we drop inside `docs/legacy/`
/// after a successful migration. Pure constant — kept here (not in a
/// template file) because:
///   - it's tiny, no need for the template-resolver overhead
///   - it must not be confused with the project's templated docs
///   - we want a single source of truth for the wording (one place
///     to update if the message changes across releases)
pub(crate) const LEGACY_README_BODY: &str = "# Legacy docs — preserved from before Kronn bootstrap\n\
\n\
This folder holds documentation that lived in `docs/` BEFORE this project was \
onboarded to Kronn. The audit moved it here automatically so the freshly-\
installed Kronn templates above could install cleanly, then **used this content \
as the primary source of truth** while filling them.\n\
\n\
## After the audit\n\
\n\
1. Open the templates in `docs/` (`AGENTS.md`, `glossary.md`, `repo-map.md`, …) \
and confirm the audit captured everything that matters. Inline citations like \
`cf docs/legacy/installation.md` should point you at each source.\n\
2. If anything is missing, copy the relevant pieces from `docs/legacy/` into \
the Kronn templates manually — the templates are the future source of truth.\n\
3. Once you've validated the audit output, **you can safely delete \
`docs/legacy/` entirely** — the content survives in the Kronn templates plus \
git history.\n\
\n\
## Do NOT edit files here\n\
\n\
This folder is a frozen snapshot from before Kronn took over `docs/` \
management. Future audits won't read it again (the project is now Kronn-\
managed — re-runs are idempotent). Edit the templates in `docs/` instead, \
where your changes will survive Kronn updates.\n\
";

/// Write the navigational README inside `docs/legacy/` IF it doesn't
/// already exist. Idempotent : a hand-edited copy (the user added
/// their own notes) is never clobbered. The file name `README.md`
/// matches Git/GitHub convention so it surfaces in folder views.
fn write_legacy_readme_if_absent(legacy_dir: &Path) -> std::io::Result<()> {
    let readme = legacy_dir.join("README.md");
    if readme.exists() {
        return Ok(());
    }
    fs::write(&readme, LEGACY_README_BODY)
}

/// If `dst` exists, return `dst-1`, `dst-2`, … so a re-run after a
/// partial prior migration doesn't fail-or-clobber. Stops at -99 and
/// returns the last attempt (lets the caller's `rename` fail loudly
/// if we're somehow at 100 conflicts — a real-world impossibility).
fn unique_destination(dst: &Path) -> PathBuf {
    if !dst.exists() {
        return dst.to_path_buf();
    }
    for i in 1..=99 {
        let candidate = match dst.file_name().and_then(|n| n.to_str()) {
            Some(name) => dst.with_file_name(format!("{}-{}", name, i)),
            None => return dst.to_path_buf(),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    dst.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn no_docs_dir_is_a_no_op() {
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        // Don't create docs/ at all.
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(!r.migrated);
        assert_eq!(r.skip_reason, "no-docs-dir");
    }

    #[test]
    fn already_kronn_managed_skips_with_clear_reason() {
        // Project that's been bootstrapped before — has AGENTS.md
        // with the Kronn signature on its first non-empty line.
        // Even with user content nearby, we DON'T move anything.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("AGENTS.md"),
              "\n# AI agent context — Entry point\n\nSome filled content...\n");
        write(&docs.join("user-doc.md"), "user-curated content");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(!r.migrated);
        assert_eq!(r.skip_reason, "already-kronn-managed");
        // Sanity: user-doc.md stays in place — was NOT migrated.
        assert!(docs.join("user-doc.md").exists());
        assert!(!docs.join("legacy").exists());
    }

    #[test]
    fn user_managed_docs_are_moved_to_legacy() {
        // The headline case: a brand-new Kronn user pointing at a
        // project with hand-curated docs. AGENTS.md either missing
        // OR exists with a non-Kronn first line.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("installation.md"), "1. Clone the repo\n2. ...");
        write(&docs.join("api.md"), "## Endpoints\n...");
        write(&docs.join("architecture/overview.md"), "We use hexagonal arch");
        write(&docs.join("internal/onboarding.md"), "Welcome new hires");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        assert_eq!(r.moved_count, 4);
        // Each top-level entry now lives under docs/legacy/<name>
        // with its subtree intact.
        assert!(docs.join("legacy/installation.md").exists());
        assert!(docs.join("legacy/api.md").exists());
        assert!(docs.join("legacy/architecture/overview.md").exists(),
            "directory subtrees must be moved whole");
        assert!(docs.join("legacy/internal/onboarding.md").exists());
        // Originals are gone from docs/ root.
        assert!(!docs.join("installation.md").exists());
        assert!(!docs.join("architecture/overview.md").exists());
    }

    #[test]
    fn user_managed_docs_with_collision_on_kronn_named_file_still_migrate() {
        // The trickier case: user has a `docs/architecture/overview.md`
        // BUT no Kronn signature anywhere — so it's a pre-Kronn
        // user-curated file that happens to collide with a Kronn
        // template name. It MUST be moved to legacy/, otherwise the
        // template install would skip it (non-destructive) and the
        // audit would have a Frankenstein file.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("architecture/overview.md"),
              "USER'S 200-line architecture doc, predates Kronn");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        assert!(docs.join("legacy/architecture/overview.md").exists());
        let lifted = fs::read_to_string(docs.join("legacy/architecture/overview.md")).unwrap();
        assert!(lifted.contains("USER'S 200-line"),
            "user content must be preserved verbatim under legacy/");
    }

    #[test]
    fn protected_dirs_are_left_alone() {
        // `var/` (gitignored runtime) and `legacy/` (our destination)
        // never get moved. Pre-existing `legacy/` content survives a
        // re-run without being recursively wrapped (legacy/legacy/X).
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("var/cache.json"), "{}");
        write(&docs.join("legacy/prior-run.md"), "from a prior migration");
        write(&docs.join("user-doc.md"), "moves to legacy");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        // var/ stays in place
        assert!(docs.join("var/cache.json").exists());
        assert!(!docs.join("legacy/var").exists(), "var/ must NOT be wrapped under legacy/");
        // prior legacy/ content preserved
        assert!(docs.join("legacy/prior-run.md").exists());
        assert!(!docs.join("legacy/legacy").exists(), "must not recurse into our own destination");
        // user-doc.md got moved
        assert!(docs.join("legacy/user-doc.md").exists());
    }

    #[test]
    fn migration_is_idempotent_across_two_calls_post_template_install() {
        // First call: user-managed docs → migrate.
        // Then SIMULATE the template install putting AGENTS.md with
        // the Kronn signature in place.
        // Second call: must detect already-Kronn-managed, no-op.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("legacy-doc.md"), "pre-Kronn content");
        let r1 = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r1.migrated);
        // Audit pipeline installs templates AFTER us; emulate that.
        write(&docs.join("AGENTS.md"),
              "# AI agent context — Entry point\n\n{{PROJECT_NAME}} — ...\n");
        write(&docs.join("glossary.md"), "{{PLACEHOLDER}}");
        // Re-run migration (e.g. user clicks Re-audit later).
        let r2 = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(!r2.migrated);
        assert_eq!(r2.skip_reason, "already-kronn-managed");
        // Templates installed by the audit pipeline are untouched.
        assert!(docs.join("AGENTS.md").exists());
        assert!(docs.join("glossary.md").exists());
        // Legacy content still in place from the first run.
        assert!(docs.join("legacy/legacy-doc.md").exists());
    }

    #[test]
    fn empty_docs_dir_yields_nothing_to_migrate() {
        // An empty docs/ — odd but possible (user mkdir'd it but
        // never wrote anything). No migration needed, no error.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        fs::create_dir(&docs).unwrap();
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(!r.migrated);
        assert_eq!(r.skip_reason, "nothing-to-migrate");
    }

    #[test]
    fn is_kronn_managed_docs_handles_leading_whitespace_and_blank_lines() {
        // Defensive parse: the first NON-EMPTY trimmed line is what
        // we compare against the signature. A file starting with a
        // BOM, a blank line, or leading spaces must still be detected
        // as Kronn-managed.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("AGENTS.md"),
              "\n   \n   # AI agent context — Entry point   \n\nrest");
        assert!(is_kronn_managed_docs(&docs),
            "leading whitespace/blank lines must not defeat detection");
    }

    #[test]
    fn is_kronn_managed_docs_returns_false_when_first_line_is_user_content() {
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("AGENTS.md"),
              "# My project's docs\n\nI wrote this before Kronn existed.");
        assert!(!is_kronn_managed_docs(&docs));
    }

    #[test]
    fn moved_entries_are_capped_to_50_but_count_preserved() {
        // Pathological case: a dump of 100 markdown files. The SSE
        // payload should stay small (50 names max) but the count
        // surface the real number so the UI can say "+50 more".
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        for i in 0..100 {
            write(&docs.join(format!("doc-{i:03}.md")), &format!("body {i}"));
        }
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        assert_eq!(r.moved_count, 100, "full count must reach the UI even when list is truncated");
        assert!(r.moved_entries.len() <= 50, "surfaced list must be bounded for SSE payload size");
    }

    // ── Data-safety tests — the migration MOVES user files, so any
    // bug here can lose hand-curated content. Each test below pins
    // a specific failure mode we have to guarantee against.

    #[test]
    fn unicode_and_special_chars_in_filenames_survive_the_move() {
        // Real-world: French accents, emoji, spaces. fs::rename
        // handles them on Unix; this test pins the byte-perfect
        // preservation so a future "normalize names" refactor can't
        // silently mangle filenames or lose content.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        let names = [
            "installation-français.md",
            "présentation-équipe.md",
            "doc with spaces.md",
            "doc-with-émoji-📝.md",
        ];
        for n in &names {
            write(&docs.join(n), &format!("body of {n}"));
        }
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert_eq!(r.moved_count, names.len());
        for n in &names {
            let moved = docs.join("legacy").join(n);
            assert!(moved.exists(), "{} must be preserved verbatim under legacy/", n);
            let body = fs::read_to_string(&moved).unwrap();
            assert!(body.contains(&format!("body of {n}")),
                "{} content must be byte-identical to source", n);
        }
    }

    #[test]
    fn dotfiles_in_docs_are_migrated_too() {
        // Pre-Kronn projects sometimes have `.gitkeep`, `.DS_Store`,
        // or custom dotfiles in docs/. They MUST move with the rest —
        // they're user content like any other file.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join(".gitkeep"), "");
        write(&docs.join(".secret-notes"), "internal jotting");
        write(&docs.join("README.md"), "main");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert_eq!(r.moved_count, 3, "dotfiles count toward the migration");
        assert!(docs.join("legacy/.gitkeep").exists());
        assert!(docs.join("legacy/.secret-notes").exists());
        assert!(docs.join("legacy/README.md").exists());
    }

    #[test]
    fn destination_collision_in_legacy_uses_unique_suffix() {
        // Edge case: a previous (partial) migration left
        // `docs/legacy/installation.md`. Now the user re-runs the
        // audit with a NEW `docs/installation.md` at root (different
        // content). Behaviour : the existing legacy file must NOT be
        // overwritten — the new arrival gets a `-1` suffix. Without
        // this, users could silently lose their prior legacy copy.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("legacy/installation.md"), "OLD legacy content");
        write(&docs.join("installation.md"), "NEW content to migrate");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        // OLD file untouched
        let old = fs::read_to_string(docs.join("legacy/installation.md")).unwrap();
        assert_eq!(old, "OLD legacy content",
            "pre-existing legacy/ content must NEVER be overwritten");
        // NEW arrival lives at legacy/installation.md-1
        let new = fs::read_to_string(docs.join("legacy/installation.md-1")).unwrap();
        assert_eq!(new, "NEW content to migrate",
            "new collisions take the next available suffix");
    }

    #[test]
    fn deep_subtree_is_moved_intact_with_all_levels() {
        // 4-deep nested user docs — verifies `fs::rename` on a
        // directory preserves the whole subtree atomically. If we
        // ever switch to a manual recursive copy (e.g. cross-device
        // moves), the equivalent test must still pass.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("a/b/c/d/leaf.md"), "deep content");
        write(&docs.join("a/b/sibling.md"), "sibling content");
        write(&docs.join("a/peer.md"), "peer content");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        assert!(docs.join("legacy/a/b/c/d/leaf.md").exists(),
            "deep nested files survive the move");
        assert!(docs.join("legacy/a/b/sibling.md").exists());
        assert!(docs.join("legacy/a/peer.md").exists());
        // Originals are gone
        assert!(!docs.join("a/b/c/d/leaf.md").exists());
        assert!(!docs.join("a").exists());
        // Content byte-identical
        let leaf = fs::read_to_string(docs.join("legacy/a/b/c/d/leaf.md")).unwrap();
        assert_eq!(leaf, "deep content");
    }

    #[test]
    fn file_named_agents_md_without_kronn_signature_still_migrates() {
        // CRITICAL: a user with a hand-curated `docs/AGENTS.md` (no
        // Kronn signature on line 1) MUST have it preserved under
        // legacy/. Without this guard, the template install would
        // skip it (non-destructive) AND the user's content would
        // both stay in place AND be partially overwritten by audit
        // fills → Frankenstein file.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("AGENTS.md"),
              "# My homebrew AI guidance\n\nWe use Symfony 6.4 ...");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        let moved = fs::read_to_string(docs.join("legacy/AGENTS.md")).unwrap();
        assert!(moved.contains("My homebrew AI guidance"),
            "user's AGENTS.md must end up under legacy/ verbatim");
        // Root AGENTS.md is gone — about to be re-installed from
        // Kronn template by the caller.
        assert!(!docs.join("AGENTS.md").exists());
    }

    #[test]
    fn migration_does_not_touch_files_outside_docs_dir() {
        // Belt-and-suspenders: even though `migrate_user_docs_to_legacy`
        // only reads `docs/`, future changes might accidentally walk
        // upward. Pin a file just outside docs/ and assert it survives.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("inside.md"), "inside docs/");
        let outside = tmp.path().join("README.md");
        write(&outside, "PROJECT README — must NEVER move");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        assert!(outside.exists(), "README.md outside docs/ must NEVER be touched");
        let body = fs::read_to_string(&outside).unwrap();
        assert_eq!(body, "PROJECT README — must NEVER move");
    }

    #[test]
    fn migration_writes_navigational_readme_in_legacy_dir() {
        // 0.8.3 (#273) — after the move, `docs/legacy/README.md`
        // explains the folder's purpose and how to retire it. Without
        // this, the folder is invisible to anyone opening the project
        // weeks later, and the user has no obvious "what's this?"
        // pointer.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("installation.md"), "user content");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);
        let readme = docs.join("legacy/README.md");
        assert!(readme.exists(),
            "migration must drop a navigational README in legacy/");
        let body = fs::read_to_string(&readme).unwrap();
        // Critical wording the user must see: it's pre-Kronn, it's
        // safe to delete after audit validation.
        assert!(body.contains("preserved from before Kronn"),
            "README must explain provenance: {body}");
        assert!(body.contains("you can safely delete `docs/legacy/`")
                || body.contains("safely delete"),
            "README must spell out the retire-when-validated path");
        assert!(body.contains("Do NOT edit"),
            "README must warn against editing the snapshot");
    }

    #[test]
    fn migration_skip_does_not_create_legacy_readme() {
        // When `already-kronn-managed` skips the migration, we must
        // NOT touch the legacy/ folder (might not even exist on a
        // freshly-bootstrapped Kronn project). Writing the README on
        // every audit run would muddy the tree and look like Kronn
        // keeps "doing something" with legacy files even when nothing
        // moved.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        write(&docs.join("AGENTS.md"),
              "# AI agent context — Entry point\n\nfilled content");
        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(!r.migrated);
        assert!(!docs.join("legacy").exists(),
            "skipped migration must NEVER create a docs/legacy/ folder");
    }

    #[test]
    fn legacy_readme_is_not_clobbered_on_partial_re_run() {
        // The classical recovery scenario: a prior migration succeeded
        // but the user then HAND-EDITED `docs/legacy/README.md`
        // (added their own notes about which files matter most). A
        // subsequent partial re-run (e.g. someone copied more files
        // into docs/ then re-launched the audit) must NOT clobber
        // those hand-edits. We re-emit the README ONLY when absent.
        let tmp = TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        // First pass — migration writes default README.
        write(&docs.join("a.md"), "first batch");
        migrate_user_docs_to_legacy(&docs).unwrap();
        // User hand-edits the README.
        let custom = "# CUSTOM — my own legacy notes\n\nDon't touch.";
        fs::write(docs.join("legacy/README.md"), custom).unwrap();
        // Wipe Kronn signature so the next call doesn't take the
        // `already-kronn-managed` short-circuit; new user content
        // arrives in docs/.
        write(&docs.join("b.md"), "second batch");
        // Second pass — migration moves b.md into legacy/ but must
        // NOT overwrite the custom README.
        let r2 = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r2.migrated, "fresh user files in docs/ → migration runs");
        let preserved = fs::read_to_string(docs.join("legacy/README.md")).unwrap();
        assert_eq!(preserved, custom,
            "hand-edited legacy/README.md must NEVER be clobbered by a later migration");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_docs_are_moved_as_symlinks_never_followed() {
        // CRITICAL data-safety guard. A user might have
        // `docs/spec.md → ../external/spec.md` for cross-repo
        // referencing. Following the symlink during the move would
        // either (a) clobber the target file, or (b) escape the
        // docs/ sandbox entirely. fs::rename respects symlinks
        // (moves the link, not its target); this test pins that
        // behaviour so any future replacement (e.g. shutil-style
        // copy+delete) preserves it.
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("external-target.md");
        write(&target, "REAL TARGET — must never be touched");
        let docs = tmp.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        let link = docs.join("spec.md");
        symlink(&target, &link).unwrap();
        // Sanity: link points where we expect.
        assert_eq!(fs::read_link(&link).unwrap(), target);

        let r = migrate_user_docs_to_legacy(&docs).unwrap();
        assert!(r.migrated);

        // External target file UNTOUCHED (byte-identical).
        let target_body = fs::read_to_string(&target).unwrap();
        assert_eq!(target_body, "REAL TARGET — must never be touched",
            "symlink target outside docs/ must never be overwritten or deleted");
        // Link itself moved to legacy/, still a symlink, still
        // pointing at the original target.
        let moved_link = docs.join("legacy/spec.md");
        assert!(moved_link.exists(), "the symlink itself must end up under legacy/");
        let meta = fs::symlink_metadata(&moved_link).unwrap();
        assert!(meta.file_type().is_symlink(),
            "the moved entry must remain a symlink (never deref'd)");
    }
}
