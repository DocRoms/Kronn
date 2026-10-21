//! Which context entry point a linked repository really has (KT-999): the
//! prompt and the UI state what exists instead of assuming `docs/AGENTS.md`.

use std::path::Path;

use crate::models::{LinkedRepo, LinkedRepoAiDoc, LinkedRepoDocStatus};

/// Root entries scanned for a README; a repository root is rarely larger.
const README_SCAN_MAX: usize = 2_000;

/// The first `README*` file at the root, `README.md` preferred, by name order.
fn find_readme(root: &Path) -> Option<String> {
    if root.join("README.md").is_file() {
        return Some("README.md".into());
    }
    let mut names: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .take(README_SCAN_MAX)
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.to_ascii_uppercase().starts_with("README"))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// `docs/AGENTS.md`, else legacy `ai/index.md`, else a README.
pub fn detect_ai_doc(root: &Path) -> (LinkedRepoAiDoc, Option<String>) {
    if !root.is_dir() {
        return (LinkedRepoAiDoc::Missing, None);
    }
    if root.join("docs/AGENTS.md").is_file() {
        return (LinkedRepoAiDoc::AgentsMd, Some("docs/AGENTS.md".into()));
    }
    if root.join("ai/index.md").is_file() {
        return (LinkedRepoAiDoc::LegacyAiIndex, Some("ai/index.md".into()));
    }
    match find_readme(root) {
        Some(readme) => (LinkedRepoAiDoc::ReadmeOnly, Some(readme)),
        None => (LinkedRepoAiDoc::NoAiDoc, None),
    }
}

pub fn status_of(repo: &LinkedRepo) -> LinkedRepoDocStatus {
    let (ai_doc, doc_path) = match repo.local_location() {
        None => (LinkedRepoAiDoc::Remote, None),
        Some(location) => detect_ai_doc(&crate::core::scanner::resolve_host_path(location)),
    };
    LinkedRepoDocStatus {
        id: repo.id.clone(),
        ai_doc,
        doc_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(location: &str) -> LinkedRepo {
        LinkedRepo {
            id: "lr".into(),
            name: "x".into(),
            kind: "api".into(),
            location: location.into(),
            description: String::new(),
        }
    }

    #[test]
    fn agents_md_wins_over_the_legacy_index_and_the_readme() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::create_dir_all(dir.path().join("ai")).unwrap();
        std::fs::write(dir.path().join("README.md"), "r").unwrap();
        std::fs::write(dir.path().join("ai/index.md"), "i").unwrap();
        assert_eq!(
            detect_ai_doc(dir.path()),
            (LinkedRepoAiDoc::LegacyAiIndex, Some("ai/index.md".into()))
        );
        std::fs::write(dir.path().join("docs/AGENTS.md"), "a").unwrap();
        assert_eq!(
            detect_ai_doc(dir.path()),
            (LinkedRepoAiDoc::AgentsMd, Some("docs/AGENTS.md".into()))
        );
    }

    #[test]
    fn a_readme_alone_is_reported_as_readme_only_and_nothing_as_no_ai_doc() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_ai_doc(dir.path()), (LinkedRepoAiDoc::NoAiDoc, None));
        std::fs::write(dir.path().join("readme.rst"), "r").unwrap();
        assert_eq!(
            detect_ai_doc(dir.path()),
            (LinkedRepoAiDoc::ReadmeOnly, Some("readme.rst".into()))
        );
        // A directory named like the entry point is not one.
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("docs/AGENTS.md")).unwrap();
        assert_eq!(
            detect_ai_doc(other.path()),
            (LinkedRepoAiDoc::NoAiDoc, None)
        );
    }

    #[test]
    fn a_url_is_remote_and_a_missing_path_is_missing() {
        assert_eq!(
            status_of(&repo("https://github.com/org/api")).ai_doc,
            LinkedRepoAiDoc::Remote
        );
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert_eq!(
            status_of(&repo(gone.to_str().unwrap())).ai_doc,
            LinkedRepoAiDoc::Missing
        );
        assert_eq!(
            status_of(&repo(dir.path().to_str().unwrap())).ai_doc,
            LinkedRepoAiDoc::NoAiDoc
        );
    }
}
