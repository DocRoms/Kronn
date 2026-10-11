//! What the AI doc folder already holds (KT-1192): nothing, Kronn's own docs
//! (updated in place, never asked about), or a human's docs (the briefing
//! asks: another folder, or move them to `<folder>/legacy/`). Kronn's docs
//! need both proofs: a `.kronn.json` recording an audit or a bootstrap, and a
//! structural marker. Anything less is a human's docs.

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

/// `.kronn.json` in `dir` records an audit or a bootstrap.
fn has_kronn_state(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(crate::core::kronn_state::KRONN_STATE_FILENAME))
        .ok()
        .and_then(|raw| serde_json::from_str::<crate::core::kronn_state::KronnState>(&raw).ok())
        .is_some_and(|state| state.has_any_audit() || state.bootstrapped_at.is_some())
}

/// The state of `<root>/<rel>`. Never follows a symlink.
pub fn folder_state(root: &Path, rel: &str) -> DocsFolderState {
    let dir = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&dir) else {
        return DocsFolderState::Missing;
    };
    if !meta.is_dir() {
        return DocsFolderState::Missing;
    }
    let recorded = has_kronn_state(&dir);
    let mut files = 0u32;
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
        if entry.depth() == 1
            && (entry.file_name() == "briefing.md"
                || entry.file_name() == crate::core::kronn_state::KRONN_STATE_FILENAME)
        {
            continue;
        }
        files += 1;
        if recorded && !managed && (files as usize) <= MAX_SCANNED && is_marker_file(entry.path()) {
            managed = true;
        }
    }
    let signed = || files > 0 && crate::core::legacy_docs::is_kronn_managed_docs(&dir);
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
            crate::core::legacy_docs::migrate_user_docs_to_legacy_in(&dir, docs_root)
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
