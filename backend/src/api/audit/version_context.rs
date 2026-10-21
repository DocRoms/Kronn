// Dated version sources for one audit run (KT-1000): loaded once, injected
// into the steps that judge versions and standards, and handed to the
// citation gate so a `deps:` marker is checked against what was injected.

use std::path::Path;

use crate::core::repo_facts::RepoFacts;
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
    /// Tracked tests, CI triggers and runtimes, committed secrets (KT-1404),
    /// handed to the steps that state or judge them.
    repo_facts: RepoFacts,
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
        let (runtimes, ci_files, stack_facts, repo_facts) =
            tokio::task::spawn_blocking(move || {
                (
                    ci_runtime_versions(&root),
                    ci_files_present(&root),
                    crate::core::stack_consistency::facts_block(&root),
                    RepoFacts::collect(&root),
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
            repo_facts,
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
        // KT-1404 — what the repository holds, computed: the tests, the CI and
        // the committed secrets are stated from these, never from a reading.
        if states_repo_facts(target_file) {
            if let Some(tests) = &self.repo_facts.tests {
                out.push_str(&format!("\n\n{tests}\n"));
            }
            if !self.repo_facts.ci.is_empty() {
                out.push_str(&format!("\n\n{}\n", self.repo_facts.ci));
            }
        }
        if judges_secrets(target_file) && !self.repo_facts.secrets.is_empty() {
            out.push_str(&format!("\n\n{}\n", self.repo_facts.secrets));
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The steps that write the stack's versions in prose — the project index, the
/// coding rules, the tech-debt index (Dependencies) and the final review — are
/// handed the computed stack facts (KT-1388, KT-1404): a version copied from
/// them cannot contradict the manifests or another document.
pub(super) fn states_versions(target_file: &str) -> bool {
    is_stack_table_step(target_file)
        || target_file.ends_with("coding-rules.md")
        || target_file.ends_with("inconsistencies-tech-debt.md")
        || target_file.ends_with("decisions.md")
}

/// The steps that state what the tests and the CI are — the project index, the
/// testing document, the operations document — and the final review, which
/// makes every document agree with the computed facts (KT-1404).
pub(super) fn states_repo_facts(target_file: &str) -> bool {
    is_stack_table_step(target_file)
        || target_file.ends_with("testing-quality.md")
        || target_file.ends_with("debug-operations.md")
        || target_file.ends_with("decisions.md")
}

/// The steps that judge or state security: the security pass, the Docker pass
/// (credential hygiene of compose files), the tech-debt index (Security
/// dimension) and the final review (KT-1404).
pub(super) fn judges_secrets(target_file: &str) -> bool {
    target_file.ends_with("inconsistencies-security.md")
        || target_file.ends_with("inconsistencies-docker.md")
        || target_file.ends_with("inconsistencies-tech-debt.md")
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
            repo_facts: RepoFacts::default(),
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
    fn the_stack_facts_reach_the_index_the_coding_rules_the_debt_index_and_the_final_review_only() {
        // KT-1388 — computed, not asked: the steps that write a version in
        // prose copy it from the manifests. KT-1404: the debt index too, whose
        // Dependencies dimension cites the project version and the old pins.
        let context = VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: "JUDGEMENT".into(),
            stack_block: "STACK".into(),
            stack_facts: Some("FACTS".into()),
            repo_facts: RepoFacts::default(),
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
        // The tech-debt index keeps its judgement block and now carries the facts.
        let debt = context
            .block_for("docs/inconsistencies-tech-debt.md")
            .unwrap();
        assert!(
            debt.contains("JUDGEMENT") && debt.contains("FACTS"),
            "{debt}"
        );
        for other in ["docs/glossary.md", "docs/repo-map.md", "REVIEW"] {
            assert!(context.block_for(other).is_none(), "{other}");
        }
    }

    fn repo_context() -> VersionContext {
        VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: "JUDGEMENT".into(),
            stack_block: "STACK".into(),
            stack_facts: None,
            repo_facts: RepoFacts {
                tests: Some("TESTS-FACTS".into()),
                ci: "CI-FACTS".into(),
                secrets: "SECRET-FACTS".into(),
            },
        }
    }

    #[test]
    fn tests_and_ci_facts_reach_the_index_testing_operations_and_the_final_review() {
        // KT-1404 — steps 1, 5, 7 and 9 state what the tests and the CI are;
        // step 9 makes every document agree with these facts.
        let context = repo_context();
        for target in [
            "docs/AGENTS.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/decisions.md",
        ] {
            let block = context.block_for(target).unwrap();
            assert!(
                block.contains("TESTS-FACTS") && block.contains("CI-FACTS"),
                "{target}: {block}"
            );
        }
        for other in [
            "docs/glossary.md",
            "docs/repo-map.md",
            "docs/coding-rules.md",
            "docs/architecture/overview.md",
            "docs/inconsistencies-security.md",
        ] {
            let block = context.block_for(other).unwrap_or_default();
            assert!(
                !block.contains("TESTS-FACTS") && !block.contains("CI-FACTS"),
                "{other}: {block}"
            );
        }
    }

    #[test]
    fn the_secret_facts_reach_the_security_and_docker_passes_the_debt_index_and_the_final_review_only(
    ) {
        let context = repo_context();
        for target in [
            "docs/inconsistencies-security.md",
            "docs/inconsistencies-docker.md",
            "docs/inconsistencies-tech-debt.md",
            "docs/decisions.md",
        ] {
            let block = context.block_for(target).unwrap();
            assert!(block.contains("SECRET-FACTS"), "{target}: {block}");
        }
        for other in [
            "docs/AGENTS.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/coding-rules.md",
            "docs/glossary.md",
        ] {
            let block = context.block_for(other).unwrap_or_default();
            assert!(!block.contains("SECRET-FACTS"), "{other}: {block}");
        }
    }

    fn write(root: &std::path::Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A git repository shaped like the two audited ones: committed secrets,
    /// a label-gated CI on Node 20, a filled `tests/`, old pinned dependencies.
    fn incident_repository() -> (tempfile::TempDir, [String; 3]) {
        let dir = tempfile::tempdir().unwrap();
        let token = ["a1B2c3D4", "e5F6g7H8", "i9J0k1L2"].concat();
        let webhook = format!(
            "https://contoso.webhook.office.com/webhookb2/{}/IncomingWebhook/{}",
            "11111111-2222-3333-4444-555555555555@aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "q7Zk2mX9vB4nR8tY1wC5eH3jL6pD0sFg"
        );
        let key = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ".to_string();
        write(
            dir.path(),
            ".env.dist",
            &format!("APP_ENV=dev\nGLOBAL_API_TOKEN={token}\n"),
        );
        write(
            dir.path(),
            "Jenkinsfile.staging",
            &format!("pipeline {{ environment {{ HOOK = '{webhook}' }} }}\n"),
        );
        write(
            dir.path(),
            ".ssh/deploy-key",
            &format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{key}\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
        );
        write(dir.path(), ".ssh/deploy-key.pub", "ssh-ed25519 AAAA host\n");
        write(
            dir.path(),
            ".github/workflows/ci.yml",
            "name: CI\non:\n  pull_request:\n    types: [labeled]\njobs:\n  test:\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n",
        );
        write(
            dir.path(),
            "package.json",
            "{\n  \"name\": \"webapp\",\n  \"version\": \"1.46.1\",\n  \"engines\": {\"node\": \">=22\"},\n  \"dependencies\": {\"request\": \"^2.88.0\", \"lodash\": \"^3.10.1\"}\n}\n",
        );
        write(
            dir.path(),
            "package-lock.json",
            "{\n  \"lockfileVersion\": 3,\n  \"packages\": {\n    \"node_modules/request\": {\"version\": \"2.88.2\"},\n    \"node_modules/lodash\": {\"version\": \"3.10.1\"}\n  }\n}\n",
        );
        for n in 0..43 {
            write(
                dir.path(),
                &format!("tests/Unit/Case{n}Test.php"),
                "<?php\n",
            );
        }
        let git = |args: &[&str]| {
            let status = crate::core::cmd::git_cmd()
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        (dir, [token, webhook, key])
    }

    #[test]
    fn a_repository_with_committed_secrets_and_a_label_gated_ci_is_described_as_it_is() {
        // KT-1404 — a Node app and a PHP app, end to end through git.
        let (dir, values) = incident_repository();
        let context = VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: "JUDGEMENT".into(),
            stack_block: "STACK".into(),
            stack_facts: crate::core::stack_consistency::facts_block(dir.path()),
            repo_facts: RepoFacts::collect(dir.path()),
        };
        let security = context
            .block_for("docs/inconsistencies-security.md")
            .unwrap();
        for path in [".env.dist", "Jenkinsfile.staging", ".ssh/deploy-key"] {
            assert!(
                security.contains(&format!("`{path}`")),
                "{path}: {security}"
            );
        }
        assert!(!security.contains(".ssh/deploy-key.pub`"), "{security}");
        assert!(security.contains("whatever its name says"), "{security}");
        assert!(
            security.contains("Never write \"no secrets\""),
            "{security}"
        );
        // A secret value appears in no block of any step.
        for target in [
            "docs/AGENTS.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/inconsistencies-tech-debt.md",
            "docs/inconsistencies-security.md",
            "docs/decisions.md",
        ] {
            let block = context.block_for(target).unwrap();
            for value in &values {
                assert!(
                    !block.contains(value.as_str()),
                    "{target} leaks a value: {block}"
                );
            }
            assert!(!block.contains("webhookb2"), "{target}: {block}");
        }
        // Steps 1, 5, 7 and 9 get the same tests and CI facts.
        for target in [
            "docs/AGENTS.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/decisions.md",
        ] {
            let block = context.block_for(target).unwrap();
            assert!(
                block.contains("43 tracked test file(s)"),
                "{target}: {block}"
            );
            assert!(
                block.contains("runs on: pull_request (types: [labeled])"),
                "{target}: {block}"
            );
            assert!(
                block.contains("only when `if: contains(github.event.pull_request.labels"),
                "{target}: {block}"
            );
            assert!(
                block.contains("node 20 (.github/workflows/ci.yml:12)"),
                "{target}: {block}"
            );
        }
        // The project version and the old pins reach the debt index.
        let debt = context
            .block_for("docs/inconsistencies-tech-debt.md")
            .unwrap();
        assert!(
            debt.contains("Project version: 1.46.1 (package.json:3)"),
            "{debt}"
        );
        assert!(
            debt.contains("request 2.88.2 — superseded package"),
            "{debt}"
        );
        assert!(
            debt.contains("lodash 3.10.1 — major 3, before the 4.x line"),
            "{debt}"
        );
    }

    /// A git repository of `files`, as the audit context of its project.
    fn tracked_context(files: &[(&str, &str)]) -> (tempfile::TempDir, VersionContext) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            write(dir.path(), rel, body);
        }
        for args in [&["init", "-q"][..], &["add", "-A"][..]] {
            let status = crate::core::cmd::git_cmd()
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        }
        let context = VersionContext {
            snapshot: None,
            source: VersionSource::None,
            block: String::new(),
            stack_block: String::new(),
            stack_facts: crate::core::stack_consistency::facts_block(dir.path()),
            repo_facts: RepoFacts::collect(dir.path()),
        };
        (dir, context)
    }

    #[test]
    fn a_credential_in_a_ci_condition_reaches_no_step_prompt() {
        // KT-1404 review P1 — the `if:` text was copied verbatim into four steps.
        let marker = [
            "ghp_", "q7Zk2mX9", "vB4nR8tY", "1wC5eH3j", "L6pD0sFg", "M9aV",
        ]
        .concat();
        let workflow = format!(
            "on: push\njobs:\n  deploy:\n    if: env.DEPLOY_TOKEN == '{marker}'\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ready\n"
        );
        let (_dir, context) = tracked_context(&[(".github/workflows/ci.yml", &workflow)]);
        assert!(context.repo_facts.secrets.contains("vendor token"));
        for target in [
            "docs/AGENTS.md",
            "docs/testing-quality.md",
            "docs/operations/debug-operations.md",
            "docs/decisions.md",
            "docs/inconsistencies-security.md",
        ] {
            let block = context.block_for(target).unwrap();
            assert!(
                !block.contains(&marker),
                "{target} leaks the value: {block}"
            );
        }
        let ci = context.block_for("docs/testing-quality.md").unwrap();
        assert!(ci.contains("env.DEPLOY_TOKEN =="), "{ci}");
    }

    #[test]
    fn a_public_certificate_is_never_a_mandatory_secret_finding() {
        // KT-1404 review P2 — `*.pem` was forced as Critical by its name.
        let cert = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            "MIIDdzCCAl+gAwIBAgIEAgAAuTANBgkqhkiG9w0BAQUFADBaMQswCQYDVQQGEwJJ"
        );
        let (_dir, context) = tracked_context(&[
            ("certs/public-ca.pem", &cert),
            ("certs/store.p12", "opaque"),
        ]);
        let block = context
            .block_for("docs/inconsistencies-security.md")
            .unwrap();
        assert!(!block.contains("`certs/public-ca.pem`"), "{block}");
        // The opaque store stays a candidate, worded as one.
        assert!(block.contains("`certs/store.p12`"), "{block}");
        assert!(block.contains("CANDIDATES matched by NAME only"), "{block}");
        assert!(!block.contains("is a committed secret"), "{block}");
    }

    #[test]
    fn an_env_anchor_keeps_a_label_gated_job_conditional() {
        // KT-1404 review P2 — `env: &defaults` cut the job short.
        let workflow = "on: pull_request\njobs:\n  unit:\n    env: &defaults\n      NODE_ENV: test\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n";
        let (_dir, context) = tracked_context(&[(".github/workflows/ci.yml", workflow)]);
        let block = context.block_for("docs/testing-quality.md").unwrap();
        assert!(block.contains("ci-test"), "{block}");
        assert!(block.contains("node 20"), "{block}");
        assert!(!block.contains("no condition of its own"), "{block}");
    }

    #[test]
    fn inline_rust_tests_are_counted_and_never_denied() {
        // KT-1404 review P2 — `src/lib.rs` with `#[test]` read "none".
        let (_dir, context) = tracked_context(&[
            (
                "Cargo.toml",
                "[package]\nname = \"inline-tests\"\nversion = \"0.1.0\"\n",
            ),
            (
                "src/lib.rs",
                "pub fn answer() -> u32 { 42 }\n#[cfg(test)] mod checks { #[test] fn is_42() { assert_eq!(super::answer(), 42); } }\n",
            ),
        ]);
        let block = context.block_for("docs/testing-quality.md").unwrap();
        assert!(!block.contains("found by Kronn: none"), "{block}");
        assert!(
            block.contains("at least 1 tracked `.rs` file(s)"),
            "{block}"
        );
        assert!(!block.contains("never claim tests exist"), "{block}");
    }

    #[test]
    fn a_service_token_is_listed_like_a_global_one() {
        // KT-1404 review P2 — `SERVICE_TOKEN` was dropped for containing `service`.
        let marker = ["a1B2c3D4", "e5F6g7H8", "i9J0k1L2"].concat();
        let (_dir, context) =
            tracked_context(&[(".env.dist", &format!("SERVICE_TOKEN={marker}\n"))]);
        let block = context
            .block_for("docs/inconsistencies-security.md")
            .unwrap();
        assert!(block.contains("`.env.dist`"), "{block}");
        assert!(!block.contains("Detected: none"), "{block}");
        assert!(!block.contains(&marker), "{block}");
    }
}
