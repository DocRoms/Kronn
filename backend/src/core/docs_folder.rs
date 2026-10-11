//! What the AI doc folder already holds (KT-1192): nothing, Kronn's own docs
//! (updated in place, never asked about), or a human's docs (the briefing
//! asks: another folder, or move them to `<folder>/legacy/`).

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

/// Markers only Kronn writes. `.kronn.json` and these decide; the AGENTS.md
/// first-line signature is a weak extra signal.
const KRONN_MARKERS: &[&str] = &[
    "kronn:generated",
    "<!-- kronn:section",
    "kronn:doc-version",
    "KRONN-MANAGED-BLOCK",
];

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

fn is_marker_file(path: &Path) -> bool {
    let text_like = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "md" | "markdown" | "mdx"));
    text_like
        && std::fs::read_to_string(path)
            .is_ok_and(|content| KRONN_MARKERS.iter().any(|m| content.contains(m)))
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
    if dir
        .join(crate::core::kronn_state::KRONN_STATE_FILENAME)
        .is_file()
    {
        return DocsFolderState::KronnManaged;
    }
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
        // The briefing Kronn writes from the human's answers is not their docs.
        if entry.depth() == 1 && entry.file_name() == "briefing.md" {
            continue;
        }
        files += 1;
        if !managed && (files as usize) <= MAX_SCANNED && is_marker_file(entry.path()) {
            managed = true;
        }
    }
    if managed || (files > 0 && crate::core::legacy_docs::is_kronn_managed_docs(&dir)) {
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

    #[test]
    fn kronn_docs_of_any_version_are_recognised_without_the_signature() {
        let t = tempfile::tempdir().unwrap();
        // 0.8-style: a section marker deep in a hand-edited file, no state file,
        // a first line a human rewrote.
        write(
            t.path(),
            "docs/AGENTS.md",
            "# Our project, edited by hand\n\nText.\n",
        );
        write(
            t.path(),
            "docs/architecture/overview.md",
            "<!-- kronn:section name=\"overview\" curated=\"ai\" -->\nBody\n<!-- kronn:section:end -->\n",
        );
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );

        let t = tempfile::tempdir().unwrap();
        write(t.path(), "docs/notes.md", "# Notes\n");
        write(t.path(), "docs/.kronn.json", "{}");
        assert_eq!(
            folder_state(t.path(), "docs"),
            DocsFolderState::KronnManaged
        );
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
