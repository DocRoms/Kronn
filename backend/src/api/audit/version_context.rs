// Dated version sources for one audit run (KT-1000): loaded once, injected
// into the steps that judge versions and standards, and handed to the
// citation gate so a `deps:` marker is checked against what was injected.

use std::path::Path;

use crate::core::version_sources::{
    ci_runtime_versions, render_version_block_for, select_version_source, DependencySnapshot,
    VersionPurpose, VersionSource,
};
use crate::models::AgentType;

pub(super) struct VersionContext {
    pub(super) snapshot: Option<DependencySnapshot>,
    pub(super) source: VersionSource,
    block: String,
    /// The same rows, worded for the project index's Stack table (KT-819).
    stack_block: String,
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
        let runtimes = tokio::task::spawn_blocking(move || ci_runtime_versions(&root))
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
            render_version_block_for(purpose, source, snapshot.as_ref(), &runtimes, &today)
        };
        let block = render(VersionPurpose::Judgement);
        let stack_block = render(VersionPurpose::StackTable);
        Self {
            snapshot,
            source,
            block,
            stack_block,
        }
    }

    /// The prompt block for `target_file`, if that step judges versions or
    /// fills the Stack table.
    pub(super) fn block_for(&self, target_file: &str) -> Option<String> {
        if is_stack_table_step(target_file) {
            return Some(format!("\n\n{}\n", self.stack_block));
        }
        applies_to(target_file).then(|| format!("\n\n{}\n", self.block))
    }
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
}
