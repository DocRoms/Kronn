// Dated version sources for one audit run (KT-1000): loaded once, injected
// into the steps that judge versions and standards, and handed to the
// citation gate so a `deps:` marker is checked against what was injected.

use std::path::Path;

use crate::core::version_sources::{
    ci_files_present, ci_runtime_versions, render_version_block_with_ci, select_version_source,
    DependencySnapshot, VersionPurpose, VersionSource,
};
use crate::models::AgentType;

pub(super) struct VersionContext {
    pub(super) snapshot: Option<DependencySnapshot>,
    pub(super) source: VersionSource,
    block: String,
    /// The same rows, worded for the project index's Stack table (KT-819).
    stack_block: String,
    /// What the manifests, lock files and runtime files state about the stack
    /// (KT-1388), handed to the steps that write a version.
    stack_facts: Option<String>,
}

impl VersionContext {
    /// Reads the stored dependency watch (never refreshes it: no network)
    /// and the repository's runtime pins, then picks the source cascade.
    pub(super) async fn load(
        db: &crate::db::Database,
        project_id: &str,
        project_path: &Path,
        project_path_str: &str,
        launcher: &super::agent_launch::AuditAgentLauncher,
        agent: &AgentType,
    ) -> Self {
        let pid = project_id.to_string();
        let snapshot = match db
            .with_read_conn(move |conn| crate::db::projects::get_dependency_monitoring(conn, &pid))
            .await
        {
            Ok(record) => record
                .summary
                .as_ref()
                .map(DependencySnapshot::from_summary),
            Err(e) => {
                tracing::warn!("audit: dependency watch unreadable, not injected: {e}");
                None
            }
        };
        let root = project_path.to_path_buf();
        let (runtimes, ci_files, stack_facts) = tokio::task::spawn_blocking(move || {
            (
                ci_runtime_versions(&root),
                ci_files_present(&root),
                crate::core::stack_consistency::facts_block(&root),
            )
        })
        .await
        .unwrap_or_default();
        let inputs = launcher.version_source_inputs(
            agent,
            project_path,
            project_path_str,
            snapshot.is_some(),
        );
        let source = select_version_source(inputs);
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let render = |purpose| {
            render_version_block_with_ci(
                purpose,
                source,
                snapshot.as_ref(),
                &runtimes,
                &ci_files,
                &today,
            )
        };
        let block = render(VersionPurpose::Judgement);
        let stack_block = render(VersionPurpose::StackTable);
        Self {
            snapshot,
            source,
            block,
            stack_block,
            stack_facts,
        }
    }

    /// The prompt block for `target_file`, if that step judges versions, fills
    /// the Stack table or states the stack's versions in prose.
    pub(super) fn block_for(&self, target_file: &str) -> Option<String> {
        let mut out = String::new();
        if is_stack_table_step(target_file) {
            out.push_str(&format!("\n\n{}\n", self.stack_block));
        } else if applies_to(target_file) {
            out.push_str(&format!("\n\n{}\n", self.block));
        }
        if states_versions(target_file) {
            if let Some(facts) = &self.stack_facts {
                out.push_str(&format!("\n\n{facts}\n"));
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The steps that write the stack's versions in prose — the project index, the
/// coding rules and the final review — are handed the computed stack facts
/// (KT-1388): a version copied from them cannot contradict the manifests or
/// another document.
pub(super) fn states_versions(target_file: &str) -> bool {
    is_stack_table_step(target_file)
        || target_file.ends_with("coding-rules.md")
        || target_file.ends_with("decisions.md")
}

/// The project index owns the Stack table: each row's latest stable version
/// and support end come from the same dated sources (KT-819 DoD 8).
pub(super) fn is_stack_table_step(target_file: &str) -> bool {
    target_file.ends_with("AGENTS.md")
}

/// The tech-debt index (Dependencies dimension) and the security pass
/// (supply chain) are the steps that judge versions.
pub(super) fn applies_to(target_file: &str) -> bool {
    target_file.ends_with("inconsistencies-tech-debt.md")
        || target_file.ends_with("inconsistencies-security.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_version_judging_steps_get_the_block() {
        assert!(applies_to("docs/inconsistencies-tech-debt.md"));
        assert!(applies_to("docs/inconsistencies-security.md"));
        for other in [
            "docs/AGENTS.md",
            "docs/inconsistencies-docker.md",
            "docs/decisions.md",
            "REVIEW",
        ] {
            assert!(!applies_to(other), "{other}");
        }
    }

    #[test]
    fn the_project_index_gets_the_stack_wording_and_the_judging_steps_theirs() {
        // KT-819 DoD 8 — step 1 fills the Stack table's currency cells from the
        // dated rows; it must not be told to write a TD or a coverage row.
        let context = VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: "JUDGEMENT".into(),
            stack_block: "STACK".into(),
            stack_facts: None,
        };
        assert!(context
            .block_for("docs/AGENTS.md")
            .unwrap()
            .contains("STACK"));
        assert!(context
            .block_for(".ai/AGENTS.md")
            .unwrap()
            .contains("STACK"));
        assert!(context
            .block_for("docs/inconsistencies-tech-debt.md")
            .unwrap()
            .contains("JUDGEMENT"));
        assert!(context.block_for("docs/glossary.md").is_none());
    }

    #[test]
    fn the_stack_facts_reach_the_index_the_coding_rules_and_the_final_review_only() {
        // KT-1388 — computed, not asked: the three steps that write a version
        // in prose copy it from the manifests.
        let context = VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: "JUDGEMENT".into(),
            stack_block: "STACK".into(),
            stack_facts: Some("FACTS".into()),
        };
        let index = context.block_for("docs/AGENTS.md").unwrap();
        assert!(
            index.contains("STACK") && index.contains("FACTS"),
            "{index}"
        );
        for target in ["docs/coding-rules.md", "docs/decisions.md"] {
            let block = context.block_for(target).unwrap();
            assert!(
                block.contains("FACTS") && !block.contains("STACK"),
                "{block}"
            );
        }
        // The tech-debt index keeps its judgement block, without the facts.
        let debt = context
            .block_for("docs/inconsistencies-tech-debt.md")
            .unwrap();
        assert!(
            debt.contains("JUDGEMENT") && !debt.contains("FACTS"),
            "{debt}"
        );
        for other in ["docs/glossary.md", "docs/repo-map.md", "REVIEW"] {
            assert!(context.block_for(other).is_none(), "{other}");
        }
    }
}
