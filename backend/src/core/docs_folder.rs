//! What the AI doc folder already holds (KT-1192): nothing, Kronn's own docs
//! (updated in place, never asked about), or a human's docs (the briefing
//! asks: another folder, or move them to `<folder>/legacy/`). Kronn's docs
//! need both proofs: a record (a `.kronn.json` recording an audit, a bootstrap
//! or a template install, or a Full run this instance recorded for the
//! project) and a structural marker. Anything less is a human's docs.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

/// Line-leading metadata comments only Kronn writes.
const KRONN_HEADERS: &[&str] = &["<!-- kronn:doc-version=", "<!-- kronn:generated="];

/// Static-site generators whose config would break if the docs moved.
const SITE_GENERATORS: &[&str] = &[
    "mkdocs.yml",
    "mkdocs.yaml",
    "docusaurus.config.js",
    "docusaurus.config.ts",
    "docusaurus.config.mjs",
    "docusaurus.config.cjs",
    ".vitepress",
];

/// Files scanned for markers: enough for any real docs folder, bounded for a
/// monorepo pointed at by mistake.
const MAX_SCANNED: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocsFolderState {
    /// No folder, or an empty one.
    Missing,
    /// Docs Kronn wrote, any version: updated in place.
    KronnManaged,
    /// Docs Kronn did not write.
    HumanDocs {
        /// Files a move to `legacy/` would carry.
        files: u32,
        /// A static-site generator config that references the folder.
        site_generator: Option<String>,
        /// `<folder>/legacy/` already exists: Kronn never merges into it.
        legacy_exists: bool,
    },
}

/// A Kronn structure, not a mention: outside fenced code, a line opening with
/// a Kronn header, or `kronn:section` / managed-block markers that pair up.
fn has_structural_marker(content: &str) -> bool {
    let prose = crate::core::anti_halluc::strip_fenced_code(content);
    let lines: Vec<&str> = prose.lines().map(str::trim_start).collect();
    let count = |prefix: &str| lines.iter().filter(|l| l.starts_with(prefix)).count();
    let paired = |open: &str, close: &str| {
        let opens = count(open);
        opens > 0 && opens == count(close)
    };
    lines
        .iter()
        .any(|l| KRONN_HEADERS.iter().any(|h| l.starts_with(h)))
        || paired("<!-- kronn:section ", "<!-- kronn:section:end")
        || paired(
            "<!-- KRONN-MANAGED-BLOCK:START",
            "<!-- KRONN-MANAGED-BLOCK:END",
        )
}

fn is_marker_file(path: &Path) -> bool {
    let text_like = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "md" | "markdown" | "mdx"));
    text_like && std::fs::read_to_string(path).is_ok_and(|c| has_structural_marker(&c))
}

/// The `.kronn.json` of `dir`, when it parses.
fn read_state(dir: &Path) -> Option<crate::core::kronn_state::KronnState> {
    std::fs::read_to_string(dir.join(crate::core::kronn_state::KRONN_STATE_FILENAME))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

/// `.kronn.json` records an audit, a bootstrap or a template install.
fn has_kronn_state(state: Option<&crate::core::kronn_state::KronnState>) -> bool {
    state.is_some_and(|state| {
        state.has_any_audit()
            || state.bootstrapped_at.is_some()
            || state.template_installed_at.is_some()
    })
}

/// The recorded files of `state` that are still what Kronn wrote: a recorded
/// entry stands only while the file's current sha256 equals the recorded one.
/// A file the human rewrote since (or a hand-crafted record naming a file Kronn
/// never wrote) is the human's again: counted, moved on their choice, restored
/// by the template steps.
fn intact_written(
    dir: &Path,
    state: Option<&crate::core::kronn_state::KronnState>,
) -> BTreeSet<String> {
    let Some(state) = state else {
        return BTreeSet::new();
    };
    state
        .kronn_written
        .iter()
        .filter(|(rel, hash)| {
            let path = dir.join(rel.as_str());
            std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
                && std::fs::read(&path)
                    .is_ok_and(|bytes| &crate::core::docs_merge::sha256_hex(&bytes) == *hash)
        })
        .map(|(rel, _)| rel.clone())
        .collect()
}

/// The files Kronn recorded as its own in `dir` (paths relative to it) and that
/// still hold the bytes it wrote.
pub(crate) fn kronn_written(dir: &Path) -> BTreeSet<String> {
    intact_written(dir, read_state(dir).as_ref())
}

/// `path` relative to `dir`, `/`-separated: the key of `kronn_written`.
pub(crate) fn rel_key(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every regular file under `dir`, relative to it (`legacy/` and `var/` apart),
/// to tell what a writer created from what was already there.
pub fn list_files(dir: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .max_depth(16)
        .into_iter()
        .filter_entry(|e| {
            !(e.depth() == 1 && (e.file_name() == "legacy" || e.file_name() == "var"))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| rel_key(dir, e.path()))
        .collect()
}

/// The briefing Kronn writes from the human's answers and its state files: not
/// the human's docs, counted by no gate and moved by no choice.
pub(crate) fn is_kronn_state_file(name: &str) -> bool {
    name == "briefing.md"
        || name == crate::core::kronn_state::KRONN_STATE_FILENAME
        || name == crate::core::audit_run_state::RUN_STATE_FILENAME
}

/// The state of `<root>/<rel>`. Never follows a symlink.
pub fn folder_state(root: &Path, rel: &str) -> DocsFolderState {
    folder_state_with_runs(root, rel, false)
}

/// `folder_state`, where a Full run this instance recorded for the project
/// stands for the record: a first audit stopped before writing `.kronn.json`.
pub fn folder_state_with_runs(root: &Path, rel: &str, recorded_run: bool) -> DocsFolderState {
    let dir = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&dir) else {
        return DocsFolderState::Missing;
    };
    if !meta.is_dir() {
        return DocsFolderState::Missing;
    }
    let state = read_state(&dir);
    let recorded = recorded_run || has_kronn_state(state.as_ref());
    // What Kronn wrote is its own, never a human's. It stands as proof the
    // folder is Kronn's only once an install made the folder Kronn's: written
    // beside a human's docs, it must not turn them into Kronn's (KT-1299).
    let written = intact_written(&dir, state.as_ref());
    let owned = state
        .as_ref()
        .is_some_and(|s| s.template_installed_at.is_some());
    let mut files = 0u32;
    let mut visited = 0usize;
    let mut unproven_entry = false;
    let mut managed = false;
    let walker = walkdir::WalkDir::new(&dir)
        .follow_links(false)
        .max_depth(8)
        .into_iter()
        // `legacy/` is the quarantine a previous choice made and `var/` Kronn's
        // scratch: neither is human docs still in the way, nor moved.
        .filter_entry(|e| {
            !(e.depth() == 1 && (e.file_name() == "legacy" || e.file_name() == "var"))
        });
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        // The briefing Kronn writes from the human's answers and its state
        // file are not their docs.
        if entry.depth() == 1 && is_kronn_state_file(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let kronns = written.contains(&rel_key(&dir, entry.path()));
        if kronns && !owned {
            unproven_entry |= entry.depth() == 1 && entry.file_name() == "AGENTS.md";
        } else {
            visited += 1;
            if recorded && !managed && visited <= MAX_SCANNED && is_marker_file(entry.path()) {
                managed = true;
            }
        }
        if !kronns {
            files += 1;
        }
    }
    let signed = || {
        (files > 0 || owned)
            && !unproven_entry
            && crate::core::legacy_docs::is_kronn_managed_docs(&dir)
    };
    if recorded && (managed || signed()) {
        return DocsFolderState::KronnManaged;
    }
    if files == 0 {
        return DocsFolderState::Missing;
    }
    let site_generator = SITE_GENERATORS
        .iter()
        .find(|name| root.join(name).exists() || dir.join(name).exists())
        .map(|name| name.to_string());
    DocsFolderState::HumanDocs {
        files,
        site_generator,
        legacy_exists: dir.join("legacy").exists(),
    }
}

/// Move the human docs of `<root>/<rel>` to `<root>/<rel>/legacy/`, only on
/// the human's explicit choice. Refuses when `legacy/` already exists.
pub fn move_to_legacy(
    root: &Path,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> Result<u32, String> {
    let dir = docs_root.dir(root);
    match folder_state(root, docs_root.rel()) {
        DocsFolderState::HumanDocs {
            legacy_exists: true,
            ..
        } => Err(format!(
            "{} already exists: Kronn never merges into it. Choose another folder.",
            docs_root.path("legacy/")
        )),
        DocsFolderState::HumanDocs { files, .. } => {
            crate::core::fs_guard::assert_contained_no_symlink(root, &dir.join("legacy"))?;
            crate::core::legacy_docs::move_user_docs_to_legacy_in(&dir, docs_root)
                .map_err(|e| format!("moving the docs to legacy/ failed: {e}"))?;
            Ok(files)
        }
        _ => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn missing_and_empty_folders_ask_nothing() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(folder_state(t.path(), "docs"), DocsFolderState::Missing);
        std::fs::create_dir_all(t.path().join("docs")).unwrap();
        assert_eq!(folder_state(t.path(), "docs"), DocsFolderState::Missing);
        write(t.path(), "docs/briefing.md", "# Project Briefing\n");
        assert_eq!(folder_state(t.path(), "docs"), DocsFolderState::Missing);
    }

    const AUDITED: &str =
        r#"{"audits":[{"date":"2026-05-01","kronn_version":"0.8.4","type":"full"}]}"#;
    const SECTION: &str =
        "<!-- kronn:section name=\"overview\" curated=\"ai\" -->\nBody\n<!-- kronn:section:end -->\n";

    #[test]
    fn kronn_docs_need_a_recorded_audit_and_a_structural_marker() {
        // A section marker deep in a hand-edited file, a first line a human
        // rewrote, and the audit recorded.
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "docs/AGENTS.md",
            "# Our project, edited by hand\n\nText.\n",
        );
        write(t.path(), "docs/architecture/overview.md", SECTION);
        write(t.path(), "docs/.kronn.json", AUDITED);
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );

        // A bootstrap counts as recorded.
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "docs/AGENTS.md",
            "<!-- kronn:doc-version=\"1.0\" -->\n# P\n",
        );
        write(
            t.path(),
            "docs/.kronn.json",
            r#"{"bootstrapped_at":"2026-05-01"}"#,
        );
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );
    }

    #[test]
    fn a_kronn_mention_or_an_empty_state_file_is_human_docs() {
        let human = |files| DocsFolderState::HumanDocs {
            files,
            site_generator: None,
            legacy_exists: false,
        };
        // Markers without a recorded audit: a project documenting Kronn, or
        // an exported template.
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/architecture/overview.md", SECTION);
        assert_eq!(folder_state(t.path(), "docs"), human(1));

        // A state file recording nothing.
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/architecture/overview.md", SECTION);
        write(t.path(), "docs/.kronn.json", "{}");
        assert_eq!(folder_state(t.path(), "docs"), human(1));

        // A recorded audit, but Kronn only mentioned in prose and in code.
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "docs/kronn.md",
            "Kronn writes `<!-- kronn:section ... -->` and kronn:generated headers.\n\n\
             ```md\n<!-- kronn:doc-version=\"1.0\" -->\n<!-- kronn:section name=\"x\" -->\n```\n",
        );
        write(t.path(), "docs/.kronn.json", AUDITED);
        assert_eq!(folder_state(t.path(), "docs"), human(1));

        // An unpaired section opener is not a structure.
        let t = tempfile::tempdir().unwrap();
        write(
            t.path(),
            "docs/a.md",
            "<!-- kronn:section name=\"x\" -->\nNo end.\n",
        );
        write(t.path(), "docs/.kronn.json", AUDITED);
        assert_eq!(folder_state(t.path(), "docs"), human(1));
    }

    #[test]
    fn human_docs_are_counted_with_their_site_generator() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        write(t.path(), "docs/guide/usage.md", "# Usage\n");
        write(t.path(), "mkdocs.yml", "site_name: x\n");
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::HumanDocs {
                files: 2,
                site_generator: Some("mkdocs.yml".into()),
                legacy_exists: false,
            }
        );
    }

    const KRONN_DOC: &str = "<!-- kronn:doc-version=\"1\" -->\n# AI agent context — Entry point\n";

    #[test]
    fn kronn_written_files_are_not_the_humans_and_prove_nothing_beside_theirs() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        write(t.path(), "docs/AGENTS.md", KRONN_DOC);
        write(t.path(), "docs/guide/kronn.md", KRONN_DOC);
        write(t.path(), "docs/.kronn.json", AUDITED);
        crate::core::kronn_state::record_kronn_written(
            t.path(),
            &["AGENTS.md".to_string(), "guide/kronn.md".to_string()],
        )
        .unwrap();
        let state = read_state(&t.path().join("docs")).unwrap();
        assert_eq!(state.kronn_written.len(), 2);
        // An earlier audit is recorded, but only the human's file is counted
        // and Kronn's own marked files make nothing of the folder.
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::HumanDocs {
                files: 1,
                site_generator: None,
                legacy_exists: false,
            }
        );
        // Once an install made the folder Kronn's, they are proof again.
        crate::core::kronn_state::mark_template_installed(t.path()).unwrap();
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );
    }

    #[test]
    fn a_move_to_legacy_leaves_kronn_written_files_even_inside_a_shared_directory() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        write(t.path(), "docs/guide/usage.md", "# Usage\n");
        write(t.path(), "docs/guide/kronn.md", KRONN_DOC);
        write(t.path(), "docs/AGENTS.md", KRONN_DOC);
        write(t.path(), "docs/ops/runbook.md", "# Runbook\n");
        crate::core::kronn_state::record_kronn_written(
            t.path(),
            &["AGENTS.md".to_string(), "guide/kronn.md".to_string()],
        )
        .unwrap();
        let docs = crate::core::docs_location::DocsRoot::default_root();
        assert_eq!(move_to_legacy(t.path(), &docs).unwrap(), 3);
        for kept in ["AGENTS.md", "guide/kronn.md"] {
            assert!(t.path().join("docs").join(kept).is_file(), "{kept}");
        }
        for moved in ["install.md", "guide/usage.md", "ops/runbook.md"] {
            assert!(
                t.path().join("docs/legacy").join(moved).is_file(),
                "{moved}"
            );
            assert!(!t.path().join("docs").join(moved).exists(), "{moved}");
        }
        assert!(!t.path().join("docs/legacy/AGENTS.md").exists());
        assert!(!t.path().join("docs/legacy/guide/kronn.md").exists());
    }

    const ENTRY: &str = "# AI agent context — Entry point\n\nWritten by Kronn.\n";

    fn human_count(state: &DocsFolderState) -> Option<u32> {
        match state {
            DocsFolderState::HumanDocs { files, .. } => Some(*files),
            _ => None,
        }
    }

    #[test]
    fn a_recorded_file_the_human_rewrote_is_theirs_again() {
        // An installed, audited Kronn folder whose entry file is Kronn's.
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/AGENTS.md", ENTRY);
        write(t.path(), "docs/glossary.md", "# Glossary\n");
        write(t.path(), "docs/repo-map.md", "# Repo map\n");
        write(t.path(), "docs/.kronn.json", AUDITED);
        crate::core::kronn_state::mark_template_installed(t.path()).unwrap();
        crate::core::kronn_state::record_kronn_written(
            t.path(),
            &[
                "AGENTS.md".to_string(),
                "glossary.md".to_string(),
                "repo-map.md".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );

        // The human overwrites two of them with their own text: they are
        // counted, the entry is no longer Kronn's, the choice is due.
        write(t.path(), "docs/AGENTS.md", "# Our own entry point\n");
        write(t.path(), "docs/glossary.md", "# Our own glossary\n");
        let state = folder_state(t.path(), "docs");
        assert_eq!(human_count(&state), Some(2), "{state:?}");

        // Moving carries exactly those two; the untouched file stays.
        let docs = crate::core::docs_location::DocsRoot::default_root();
        assert_eq!(move_to_legacy(t.path(), &docs).unwrap(), 2);
        for moved in ["AGENTS.md", "glossary.md"] {
            assert!(
                t.path().join("docs/legacy").join(moved).is_file(),
                "{moved}"
            );
            assert!(!t.path().join("docs").join(moved).exists(), "{moved}");
        }
        assert_eq!(
            std::fs::read_to_string(t.path().join("docs/legacy/AGENTS.md")).unwrap(),
            "# Our own entry point\n"
        );
        assert!(t.path().join("docs/repo-map.md").is_file());
    }

    #[test]
    fn a_recorded_file_in_a_shared_directory_rewritten_by_the_human_moves() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        write(t.path(), "docs/guide/kronn.md", KRONN_DOC);
        write(t.path(), "docs/guide/other.md", KRONN_DOC);
        crate::core::kronn_state::record_kronn_written(
            t.path(),
            &["guide/kronn.md".to_string(), "guide/other.md".to_string()],
        )
        .unwrap();
        write(t.path(), "docs/guide/kronn.md", "# My own guide\n");

        let docs = crate::core::docs_location::DocsRoot::default_root();
        assert_eq!(move_to_legacy(t.path(), &docs).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(t.path().join("docs/legacy/guide/kronn.md")).unwrap(),
            "# My own guide\n"
        );
        assert!(t.path().join("docs/legacy/install.md").is_file());
        // The file Kronn wrote and nobody touched stays.
        assert!(t.path().join("docs/guide/other.md").is_file());
        assert!(!t.path().join("docs/guide/kronn.md").exists());
    }

    #[test]
    fn a_hand_crafted_record_does_not_hide_a_human_file() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        write(t.path(), "docs/guide/usage.md", "# Usage\n");
        write(
            t.path(),
            "docs/.kronn.json",
            r#"{"kronn_written":{"install.md":"bogus","ghost.md":"bogus"}}"#,
        );
        let state = folder_state(t.path(), "docs");
        assert_eq!(human_count(&state), Some(2), "{state:?}");

        let docs = crate::core::docs_location::DocsRoot::default_root();
        assert_eq!(move_to_legacy(t.path(), &docs).unwrap(), 2);
        assert!(t.path().join("docs/legacy/install.md").is_file());
        assert!(!t.path().join("docs/install.md").exists());
    }

    #[test]
    fn moving_to_legacy_never_merges_into_an_existing_one() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/install.md", "# Install\n");
        let docs = crate::core::docs_location::DocsRoot::default_root();
        assert_eq!(move_to_legacy(t.path(), &docs).unwrap(), 1);
        assert!(t.path().join("docs/legacy/install.md").is_file());
        // The quarantine itself is not human docs in the way any more.
        assert_eq!(folder_state(t.path(), "docs"), DocsFolderState::Missing);

        write(t.path(), "docs/new.md", "# New\n");
        let err = move_to_legacy(t.path(), &docs).unwrap_err();
        assert!(err.contains("never merges"), "{err}");
        assert!(t.path().join("docs/new.md").is_file());
    }
}
