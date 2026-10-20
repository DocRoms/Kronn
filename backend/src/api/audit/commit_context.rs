// Commit facts for the steps that write about commits (KT-1387): computed by
// Kronn from the git history and the repository's commit tooling, handed to
// the step as ground truth, so the commit-conventions document describes what
// the history shows and says "enforced" only for what CI checks (a local hook is
// not enforcement).

use std::path::Path;

use crate::core::commit_facts::CommitFacts;
use crate::core::docs_location::DocsRoot;

/// The commit-conventions document, under the docs root.
pub(super) const COMMITS_DOC: &str = "workflow/commits.md";

/// The step that writes [`COMMITS_DOC`]: the coding rules, where the
/// conventions observed in the repository are recorded.
pub(super) fn writes_commits_doc(target_file: &str) -> bool {
    target_file.ends_with("coding-rules.md")
}

/// The entry file also states a few facts about commits (message language,
/// workflow constraints): it must not contradict the writer.
fn states_commit_facts(target_file: &str) -> bool {
    target_file.ends_with("AGENTS.md")
}

pub(super) fn block_for(project: &Path, target_file: &str, docs_root: &DocsRoot) -> Option<String> {
    let owner = writes_commits_doc(target_file);
    if !owner && !states_commit_facts(target_file) {
        return None;
    }
    let doc = docs_root.path(COMMITS_DOC);
    let mut block = CommitFacts::detect(project).render_block(&doc);
    if !owner {
        block.push_str(&format!(
            "This step does not write `{doc}` (the coding-rules step does): use these facts only for what this file says about commits, and never state the convention differently.\n"
        ));
    }
    Some(format!("\n\n{block}\n"))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn only_the_coding_rules_and_the_entry_file_get_the_block() {
        let dir = tempfile::tempdir().unwrap();
        let root = DocsRoot::default();
        let rules = block_for(dir.path(), "docs/coding-rules.md", &root).unwrap();
        assert!(rules.contains("Commit facts"));
        assert!(rules.contains("Rules for `docs/workflow/commits.md`"));
        assert!(!rules.contains("does not write"));
        let entry = block_for(dir.path(), "docs/AGENTS.md", &root).unwrap();
        assert!(entry.contains("does not write `docs/workflow/commits.md`"));
        for other in [
            "docs/glossary.md",
            "docs/repo-map.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/inconsistencies-tech-debt.md",
            "docs/decisions.md",
            "REVIEW",
        ] {
            assert!(block_for(dir.path(), other, &root).is_none(), "{other}");
        }
    }
}
