//! KT-1192 — one root for every audit artefact. A project whose AI doc lives
//! in `.ai/` gets the whole pipeline there (Full, partial, revalidation) and
//! nothing in `docs/`; the default `docs/` root is unchanged.
use super::*;
use crate::core::docs_location::DocsRoot;
use std::collections::BTreeSet;

/// Clears the project's override when the test ends, even on a panic.
struct Relocated(String);
impl Drop for Relocated {
    fn drop(&mut self) {
        crate::core::docs_location::set_project_override(&self.0, None);
    }
}

fn relocate(project: &Path, rel: &str) -> Relocated {
    let path = project.to_string_lossy().into_owned();
    crate::core::docs_location::set_project_override(&path, Some(rel));
    Relocated(path)
}

/// Every file under `dir`, relative to it.
fn files_under(dir: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            e.path()
                .strip_prefix(dir)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}

fn targets_for(docs_root: &DocsRoot) -> Vec<&'static str> {
    crate::api::audit::steps_for(crate::models::AuditKind::Full, docs_root)
        .iter()
        .map(|s| s.target_file)
        .collect()
}

/// A Full audit filled by the scripted agent, ended `complete`, its
/// validation answered. Returns the step prompts in order.
async fn audited(h: &Harness, docs_root: &DocsRoot) -> Vec<String> {
    let targets = targets_for(docs_root);
    arm(h.tools.path(), "fill", &targets);
    settle_on_validation(h.tools.path());
    let body = run_full(&h.state, None).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("no terminal done: {body}"));
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
    (1..=targets.len())
        .filter_map(|n| std::fs::read_to_string(h.tools.path().join(format!("prompt.{n}"))).ok())
        .collect()
}

/// KT-1278 — each prompt the agent was handed ends on the directive of the
/// step it runs (step n = n-th prompt: every step succeeds first time here).
fn assert_every_step_ends_on_its_directive(prompts: &[String], docs_root: &DocsRoot) {
    let steps = crate::api::audit::steps_for(crate::models::AuditKind::Full, docs_root);
    assert_eq!(prompts.len(), steps.len());
    for (index, (prompt, step)) in prompts.iter().zip(&steps).enumerate() {
        let directive = crate::api::audit::step_closing_directive(index + 1, step, docs_root);
        assert!(
            prompt.ends_with(&directive),
            "step {} must end on its directive:\n…{}",
            index + 1,
            &prompt[prompt.len().saturating_sub(500)..]
        );
        assert_eq!(prompt.matches("=== RUN THIS STEP NOW ===").count(), 1);
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn every_full_step_prompt_the_agent_receives_ends_on_its_directive() {
    let h = harness().await;
    let docs_root = DocsRoot::default_root();
    let prompts = audited(&h, &docs_root).await;
    assert_every_step_ends_on_its_directive(&prompts, &docs_root);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_full_audit_of_a_dot_ai_project_writes_everything_there_and_nothing_in_docs() {
    // The default run first: its file set and prompts are the reference.
    let reference = harness().await;
    let default_prompts = audited(&reference, &DocsRoot::default_root()).await;
    let default_files = files_under(&reference.project.path().join("docs"));
    let reference_path = reference.project.path().display().to_string();
    drop(reference);

    let h = harness().await;
    let project = h.project.path();
    let _relocated = relocate(project, ".ai");
    let ai = DocsRoot::from_rel(".ai").unwrap();
    let targets = targets_for(&ai);
    assert!(
        targets
            .iter()
            .all(|t| *t == "REVIEW" || t.starts_with(".ai/")),
        "{targets:?}"
    );

    let prompts = audited(&h, &ai).await;
    assert_every_step_ends_on_its_directive(&prompts, &ai);

    assert!(!project.join("docs").exists(), "nothing may land in docs/");
    for artefact in [
        "AGENTS.md",
        ".kronn.json",
        "checksums.json",
        "inconsistencies-tech-debt.md",
        "decisions.md",
        "conventions/agents-md-format-v1.md",
    ] {
        assert!(project.join(".ai").join(artefact).is_file(), "{artefact}");
    }
    // Same artefacts as the default run, under the other root.
    // `.ai/mcp/` is Kiro's MCP config, synced in both runs: not an audit artefact.
    let mut relocated_files = files_under(&project.join(".ai"));
    relocated_files.remove("mcp/mcp.json");
    assert_eq!(
        relocated_files,
        default_files,
        "only relocated: {:?}, only default: {:?}",
        relocated_files
            .difference(&default_files)
            .collect::<Vec<_>>(),
        default_files
            .difference(&relocated_files)
            .collect::<Vec<_>>()
    );

    // Every step prompt is the default one with the root rewritten.
    assert_eq!(prompts.len(), default_prompts.len());
    let path = project.display().to_string();
    for (relocated, default) in prompts.iter().zip(&default_prompts) {
        assert!(!relocated.contains("`docs/"), "{relocated}");
        let expected = ai
            .relocate(&default.replace(&reference_path, &path))
            .into_owned();
        assert_eq!(relocated, &expected);
    }
    assert!(prompts[0].contains("`.ai/AGENTS.md`"));

    // The root redirector points at the relocated entry.
    let redirector = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
    assert!(redirector.contains(".ai/AGENTS.md"), "{redirector}");
    assert!(!redirector.contains("docs/AGENTS.md"), "{redirector}");
    assert_eq!(
        crate::core::scanner::detect_audit_status(&path),
        crate::models::AiAuditStatus::Audited
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn briefing_residuals_phase_one_materializes_notes_before_the_first_agent_step() {
    for rel in ["docs", ".ai"] {
        let h = harness().await;
        let _relocated = relocate(h.project.path(), rel);
        let root = DocsRoot::from_rel(rel).unwrap();
        let notes = h
            .state
            .db
            .with_conn(|conn| crate::db::projects::get_project_briefing_notes(conn, PROJECT_ID))
            .await
            .unwrap()
            .unwrap();
        assert!(!root.dir(h.project.path()).exists());
        arm(h.tools.path(), "fill", &targets_for(&root));
        std::fs::write(
            h.tools.path().join("undo"),
            format!(
                "cp '{}/briefing.md' '{}/briefing-before-agent'\n",
                root.dir(h.project.path()).display(),
                h.tools.path().display()
            ),
        )
        .unwrap();
        let body = run_full(&h.state, None).await;
        let done = sse_events(&body, "done").pop().expect("terminal done");
        validation_settled(&h.state, &done).await;
        assert_eq!(done["status"], "complete", "{body}");
        assert_eq!(
            std::fs::read_to_string(h.tools.path().join("briefing-before-agent")).unwrap(),
            notes
        );
        assert_eq!(
            std::fs::read_to_string(root.dir(h.project.path()).join("briefing.md")).unwrap(),
            notes
        );
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_partial_refresh_of_a_dot_ai_project_rewrites_its_section_there() {
    let h = harness().await;
    let project = h.project.path();
    let _relocated = relocate(project, ".ai");
    let ai = DocsRoot::from_rel(".ai").unwrap();
    audited(&h, &ai).await;

    let targets = targets_for(&ai);
    let glossary = ".ai/glossary.md";
    let step = targets.iter().position(|t| *t == glossary).unwrap() + 1;
    let before = std::fs::read_to_string(project.join(glossary)).unwrap();
    arm(h.tools.path(), "fill", &[glossary]);
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![step],
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&h.state, &done).await;
    assert_eq!(started_steps(&body), vec![step as u64], "{body}");
    assert_ne!(
        std::fs::read_to_string(project.join(glossary)).unwrap(),
        before
    );
    assert!(!project.join("docs").exists(), "{body}");
    let prompt = std::fs::read_to_string(h.tools.path().join("prompt.1")).unwrap();
    assert!(prompt.contains(glossary), "{prompt}");
    assert!(!prompt.contains("`docs/"), "{prompt}");
    // KT-1278 — the partial run's prompt ends on its own step's directive.
    let steps = crate::api::audit::steps_for(crate::models::AuditKind::Full, &ai);
    let directive = crate::api::audit::step_closing_directive(step, &steps[step - 1], &ai);
    assert!(
        prompt.ends_with(&directive),
        "…{}",
        &prompt[prompt.len().saturating_sub(500)..]
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_revalidation_of_a_dot_ai_project_finds_and_fixes_its_citations_there() {
    let h = harness().await;
    let project = h.project.path();
    let _relocated = relocate(project, ".ai");
    let ai = DocsRoot::from_rel(".ai").unwrap();
    std::fs::create_dir_all(project.join("src")).unwrap();
    let lines: Vec<String> = (1..=6).map(|i| format!("fn cited_{i}() {{}}")).collect();
    std::fs::write(project.join("src/kept.rs"), lines.join("\n") + "\n").unwrap();
    arm(h.tools.path(), "fill", &targets_for(&ai));
    settle_on_validation(h.tools.path());
    std::fs::write(h.tools.path().join("validation-hold"), "").unwrap();
    let body = run_full(&h.state, None).await;
    let done = sse_events(&body, "done")
        .pop()
        .expect("completed full audit");

    let rules = ".ai/coding-rules.md";
    let mut content = std::fs::read_to_string(project.join(rules)).unwrap();
    content.push_str(
        "\n## Verified facts\n\n- Errors are mapped in one place [src: file: src/kept.rs:3]\n",
    );
    std::fs::write(project.join(rules), content).unwrap();
    std::fs::remove_file(h.tools.path().join("validation-hold")).unwrap();
    validation_settled(&h.state, &done).await;
    let validated = crate::api::audit::validate_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0;
    assert!(validated.success, "{:?}", validated.error);

    let mut moved = lines.clone();
    moved[2] = "fn renamed() {}".into();
    std::fs::write(project.join("src/kept.rs"), moved.join("\n") + "\n").unwrap();
    let plan = crate::api::audit::revalidate::plan(project);
    let section = plan
        .sections
        .iter()
        .find(|s| s.doc == rules)
        .unwrap_or_else(|| panic!("{plan:?}"));
    let step = targets_for(&ai).iter().position(|t| *t == rules).unwrap() + 1;
    assert_eq!(section.step, Some(step));
    assert_eq!(plan.agent_steps, vec![step]);

    arm(h.tools.path(), "revalidate", &[rules]);
    let response = crate::api::audit::revalidate_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::RevalidateAuditRequest {
            deterministic_only: None,
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
    let after = std::fs::read_to_string(project.join(rules)).unwrap();
    assert!(
        after.contains("Revalidated against the current code"),
        "{after}"
    );
    assert!(!project.join("docs").exists());
}

/// A TD sheet the review and the step-8 dedup read.
fn td_sheet(id: &str) -> String {
    format!(
        "---\nname: {id}\ndescription: Problem {id}\nmetadata:\n  type: tech-debt\n---\n\n# {id}\n\n\
- **Area**: Backend\n- **Severity**: Low\n- **Status**: Verified in source\n- **Effort**: S\n- **Blast radius**: Module\n\n\
## Problem (fact)\nIt breaks.\n\n## Where (pointers)\n- `src/mod0.rs:1`\n\n## Verification\nIt no longer breaks.\n"
    )
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn provenance_citations_td_review_and_step_8_follow_a_dot_ai_root() {
    let h = harness().await;
    let project = h.project.path();
    let _relocated = relocate(project, ".ai");
    let ai = DocsRoot::from_rel(".ai").unwrap();
    audited(&h, &ai).await;
    assert!(project.join(".ai/citations.json").is_file());

    // KT-1079: the validation records its provenance next to citations.json.
    let validated = crate::api::audit::validate_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0;
    assert!(validated.success, "{:?}", validated.error);
    assert!(project.join(".ai/provenance.json").is_file());

    // KT-982: the review lists and settles the sheets of the root.
    let id = "TD-20261001-relocated";
    let sheet = project.join(format!(".ai/tech-debt/{id}.md"));
    std::fs::create_dir_all(sheet.parent().unwrap()).unwrap();
    std::fs::write(&sheet, td_sheet(id)).unwrap();
    let listed = crate::api::audit::td_review::list_tds(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0
    .data
    .expect("listed");
    let item = listed
        .items
        .iter()
        .find(|i| i.id == id)
        .expect("sheet listed");
    assert_eq!(item.source_path, format!(".ai/tech-debt/{id}.md"));
    let decided = crate::api::audit::td_review::decide_tds(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::TdDecideRequest {
            items: vec![crate::models::TdDecisionItem {
                td_id: id.to_string(),
                decision: crate::models::TdReviewDecision::Confirm,
                reason: None,
            }],
        }),
    )
    .await
    .0;
    assert!(decided.success, "{:?}", decided.error);
    assert!(std::fs::read_to_string(&sheet)
        .unwrap()
        .contains("- **Status**: Confirmed by user"));

    // KT-152 / KT-1174: step 8 sees the detectors and the root's known debt.
    // Untested sources make the zero-tests detector fire; only the prompt counts.
    std::fs::create_dir_all(project.join("src")).unwrap();
    for i in 0..6 {
        std::fs::write(project.join(format!("src/mod{i}.rs")), "fn x() {}\n").unwrap();
    }
    let index = ".ai/inconsistencies-tech-debt.md";
    let step = targets_for(&ai).iter().position(|t| *t == index).unwrap() + 1;
    arm(h.tools.path(), "fill", &[index]);
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![step],
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&h.state, &done).await;
    let prompt = std::fs::read_to_string(h.tools.path().join("prompt.1")).unwrap();
    assert!(
        prompt.contains("Deterministic detector signals"),
        "{prompt}"
    );
    assert!(prompt.contains("`.ai/tech-debt/`"), "{prompt}");
    assert!(prompt.contains(id), "{prompt}");
    assert!(!prompt.contains("`docs/"), "{prompt}");
    assert!(!project.join("docs").exists(), "{body}");
}

/// Each new root interns its relocated step text once; the process keeps it.
/// The footprint per root stays bounded by Kronn's own step definitions.
#[test]
fn a_new_root_interns_a_bounded_footprint() {
    let ai = DocsRoot::from_rel("some/deeper/folder-name").unwrap();
    let mut bytes = 0usize;
    for kind in [
        crate::models::AuditKind::Full,
        crate::models::AuditKind::Drift,
        crate::models::AuditKind::Security,
        crate::models::AuditKind::Docker,
        crate::models::AuditKind::Performance,
        crate::models::AuditKind::Accessibility,
        crate::models::AuditKind::Rgaa,
        crate::models::AuditKind::Database,
        crate::models::AuditKind::ApiDesign,
        crate::models::AuditKind::CodeQuality,
    ] {
        for step in crate::api::audit::assemble_chained_steps(kind) {
            for text in [step.target_file, step.prompt]
                .into_iter()
                .chain(step.sources.iter().copied())
            {
                if let std::borrow::Cow::Owned(owned) = ai.relocate(text) {
                    bytes += owned.len();
                }
            }
        }
    }
    // About 92 KiB today: a budget, so a prompt that grows tenfold is seen.
    assert!(bytes < 256 << 10, "{bytes} bytes interned per root");
}

#[test]
fn the_default_root_leaves_every_step_and_prompt_untouched() {
    let docs = DocsRoot::default_root();
    for kind in [
        crate::models::AuditKind::Full,
        crate::models::AuditKind::Security,
        crate::models::AuditKind::Rgaa,
        crate::models::AuditKind::Drift,
    ] {
        let raw = crate::api::audit::assemble_chained_steps(kind);
        let relocated = crate::api::audit::steps_for(kind, &docs);
        assert_eq!(raw.len(), relocated.len());
        for (a, b) in raw.iter().zip(&relocated) {
            assert!(std::ptr::eq(a.target_file, b.target_file));
            assert!(std::ptr::eq(a.prompt, b.prompt));
            assert!(std::ptr::eq(a.sources, b.sources));
        }
    }
    let now = chrono::Utc::now();
    let prompt = crate::api::audit::dated_step_prompt("", "BODY", now, &docs);
    assert!(prompt.contains(crate::api::audit::PROMPT_PREAMBLE));
}

#[test]
fn template_reinstall_on_a_dot_ai_project_writes_relocated_templates_there() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("repo");
    std::fs::create_dir_all(project.join(".ai")).unwrap();
    let _relocated = relocate(&project, ".ai");
    let templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    let plan =
        crate::api::projects::template_reinstall::plan_reinstall(&project, &templates, false)
            .unwrap();
    assert_eq!(plan.docs_dir, ".ai");
    assert!(
        plan.files.iter().all(|f| f.path.starts_with(".ai/")),
        "{:?}",
        plan.files
    );
    crate::api::projects::template_reinstall::reinstall(&project, &templates, "stamp", None)
        .unwrap();
    let entry = std::fs::read_to_string(project.join(".ai/AGENTS.md")).unwrap();
    assert!(!entry.contains("`docs/"), "{entry}");
    assert!(!project.join("docs").exists());
}

#[test]
fn a_template_merge_preview_on_a_dot_ai_project_targets_its_root() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("repo");
    std::fs::create_dir_all(project.join(".ai")).unwrap();
    std::fs::write(project.join("README.md"), "# Repo\n").unwrap();
    let _relocated = relocate(&project, ".ai");
    let templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    let view = crate::api::projects::template_merge::preview_plan(&project, &templates).unwrap();
    assert_eq!(view.docs_dir, ".ai");
    assert!(
        view.files.iter().all(|f| f.path.starts_with(".ai/")),
        "{:?}",
        view.files
    );
    let entry = view
        .files
        .iter()
        .find(|f| f.path == ".ai/AGENTS.md")
        .expect("the entry is proposed");
    assert!(!entry.diff.contains("`docs/"), "{}", entry.diff);
}

// ─── Existing folder: old Kronn docs, human docs (KT-1192) ─────────────────

fn briefing(
    docs_dir: &str,
    move_to_legacy: bool,
) -> crate::api::audit::briefing::BriefingFormRequest {
    crate::api::audit::briefing::BriefingFormRequest {
        purpose: "P".into(),
        team: "T".into(),
        maturity: "M".into(),
        dependencies: "D".into(),
        traps: "Tr".into(),
        // The scripted agent recognises step prompts by this marker.
        additional: BRIEFING_MARKER.into(),
        docs_dir: Some(docs_dir.into()),
        move_existing_to_legacy: move_to_legacy,
        ..Default::default()
    }
}

fn human_docs(project: &Path) {
    std::fs::create_dir_all(project.join("docs/guide")).unwrap();
    std::fs::write(project.join("docs/install.md"), "# Install guide\n").unwrap();
    std::fs::write(project.join("docs/guide/usage.md"), "# Usage\n").unwrap();
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn human_docs_moved_to_legacy_on_choice_are_read_as_the_primary_source() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    assert!(project.join("docs/legacy/install.md").is_file());
    assert!(project.join("docs/legacy/guide/usage.md").is_file());
    assert!(!project.join("docs/install.md").exists());

    let prompts = audited(&h, &DocsRoot::default_root()).await;
    assert!(prompts[0].contains("PRIMARY SOURCE"), "{}", prompts[0]);
    assert!(prompts[0].contains("docs/legacy/"), "{}", prompts[0]);
    assert!(project.join("docs/AGENTS.md").is_file());
    assert_eq!(
        std::fs::read_to_string(project.join("docs/legacy/install.md")).unwrap(),
        "# Install guide\n"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn human_docs_with_another_folder_leave_everything_in_place() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("kronn-docs", false),
    )
    .await
    .unwrap();
    audited(&h, &DocsRoot::from_rel("kronn-docs").unwrap()).await;
    assert_eq!(
        std::fs::read_to_string(project.join("docs/install.md")).unwrap(),
        "# Install guide\n"
    );
    assert!(!project.join("docs/legacy").exists());
    assert!(!project.join("docs/AGENTS.md").exists());
    assert!(project.join("kronn-docs/AGENTS.md").is_file());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn no_audit_moves_human_docs_without_the_choice() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    // The briefing was saved before KT-1192: it made no choice.
    arm(
        h.tools.path(),
        "fill",
        &targets_for(&DocsRoot::default_root()),
    );
    let body = run_full(&h.state, None).await;
    let error = sse_events(&body, "error").pop().expect("refused");
    assert_eq!(error["error_code"], "docs_folder_choice_required", "{body}");
    assert!(
        error["error"].as_str().unwrap().contains("docs/legacy/"),
        "{body}"
    );
    assert!(project.join("docs/install.md").is_file());
    assert!(!project.join("docs/legacy").exists());
    assert!(!project.join("docs/AGENTS.md").exists());

    // The form refuses the same folder without the choice, with the warning.
    let refused = crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", false),
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .message()
            .contains("already exists and holds 2 file(s)"),
        "{refused:?}"
    );
    assert!(project.join("docs/install.md").is_file());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn old_kronn_docs_are_updated_in_place_without_a_question() {
    let h = harness().await;
    let project = h.project.path();
    audited(&h, &DocsRoot::default_root()).await;
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", false),
    )
    .await
    .unwrap();
    arm(
        h.tools.path(),
        "noop",
        &targets_for(&DocsRoot::default_root()),
    );
    let body = run_full(&h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
    assert!(!project.join("docs/legacy").exists());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_0_8_style_kronn_folder_is_not_asked_about() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("docs/architecture")).unwrap();
    std::fs::write(
        project.join("docs/AGENTS.md"),
        "# Our project, edited by hand\n",
    )
    .unwrap();
    std::fs::write(
        project.join("docs/architecture/overview.md"),
        "<!-- kronn:section name=\"overview\" curated=\"ai\" -->\nBody\n<!-- kronn:section:end -->\n",
    )
    .unwrap();
    // The audit Kronn recorded then: markers alone would be a human's docs.
    std::fs::write(
        project.join("docs/.kronn.json"),
        r#"{"audits":[{"date":"2026-05-01","kronn_version":"0.8.4","type":"full"}]}"#,
    )
    .unwrap();
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", false),
    )
    .await
    .unwrap();
    arm(
        h.tools.path(),
        "fill",
        &targets_for(&DocsRoot::default_root()),
    );
    let body = run_full(&h.state, None).await;
    assert!(
        sse_events(&body, "error")
            .iter()
            .all(|e| e["error_code"] != "docs_folder_choice_required"),
        "{body}"
    );
    assert!(!project.join("docs/legacy").exists());
}

// ─── Install decided from the folder, on both launch routes ────────────────

/// A Full launch through the HTTP route, as the UI (`bridge: false`) or an
/// MCP agent (`bridge: true`) sends it: the briefing gate, then the audit.
async fn launch_route(h: &Harness, bridge: bool) -> String {
    launch_route_fresh(h, bridge, None).await
}

async fn launch_route_fresh(h: &Harness, bridge: bool, fresh: Option<bool>) -> String {
    let caller = bridge.then(|| {
        axum::Extension(crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: Some(PROJECT_ID.into()),
            own_discussions: vec![],
            own_run: None,
            agent: None,
        })
    });
    let response = crate::api::audit::briefing_cards::full_audit_route(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        caller,
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: None,
            retry_steps: None,
            redo_steps: None,
            fresh,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

/// Every file under the project with its bytes, `.git/` excluded.
fn snapshot(project: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    walkdir::WalkDir::new(project)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let rel = e.path().strip_prefix(project).unwrap();
            (
                rel.to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

/// Both routes refuse with `code` and leave every file as it was.
async fn refused_on_both_routes(h: &Harness, code: &str, mentions: &str) {
    arm(
        h.tools.path(),
        "fill",
        &targets_for(&DocsRoot::default_root()),
    );
    let before = snapshot(h.project.path());
    for bridge in [false, true] {
        let body = launch_route(h, bridge).await;
        let error = sse_events(&body, "error")
            .pop()
            .unwrap_or_else(|| panic!("bridge={bridge}: not refused: {body}"));
        assert_eq!(error["error_code"], code, "bridge={bridge}: {body}");
        assert!(
            error["error"].as_str().unwrap().contains(mentions),
            "bridge={bridge}: {body}"
        );
        assert!(sse_events(&body, "start").is_empty(), "{body}");
        assert_eq!(snapshot(h.project.path()), before, "bridge={bridge}");
    }
}

/// `audited`, launched through the route.
async fn audited_via_route(h: &Harness, docs_root: &DocsRoot, bridge: bool) {
    arm(h.tools.path(), "fill", &targets_for(docs_root));
    settle_on_validation(h.tools.path());
    let body = launch_route(h, bridge).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("no terminal done: {body}"));
    validation_settled(&h.state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
}

fn installed(project: &Path, rel: &str) {
    for file in [
        "AGENTS.md",
        "conventions/agents-md-format-v1.md",
        ".kronn.json",
    ] {
        assert!(project.join(rel).join(file).is_file(), "{rel}/{file}");
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_human_docusaurus_folder_is_refused_on_both_routes() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("docs/guides")).unwrap();
    std::fs::write(project.join("docs/intro.md"), "# Intro\n").unwrap();
    std::fs::write(project.join("docs/guides/setup.mdx"), "# Setup\n").unwrap();
    std::fs::write(
        project.join("docusaurus.config.js"),
        "module.exports = {};\n",
    )
    .unwrap();
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_human_root_agents_md_gets_the_template_and_keeps_its_text() {
    for bridge in [false, true] {
        let h = harness().await;
        let project = h.project.path();
        std::fs::write(project.join("AGENTS.md"), "# Team rules\n\nUse tabs.\n").unwrap();
        audited_via_route(&h, &DocsRoot::default_root(), bridge).await;
        installed(project, "docs");
        let root = std::fs::read_to_string(project.join("AGENTS.md")).unwrap();
        assert!(
            root.contains("# Team rules\n\nUse tabs.\n"),
            "bridge={bridge}: {root}"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_empty_docs_folder_gets_the_template() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("docs")).unwrap();
    assert_eq!(
        crate::core::scanner::detect_audit_status(&project.to_string_lossy()),
        crate::models::AiAuditStatus::TemplateInstalled
    );
    audited(&h, &DocsRoot::default_root()).await;
    installed(project, "docs");
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_doc_folder_project_is_audited_there_and_nothing_lands_in_docs() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("doc")).unwrap();
    let doc = DocsRoot::from_rel("doc").unwrap();
    assert_eq!(DocsRoot::for_project(project).unwrap(), doc);
    audited_via_route(&h, &doc, true).await;
    installed(project, "doc");
    assert!(!project.join("docs").exists(), "nothing may land in docs/");
    assert_eq!(
        crate::core::scanner::detect_audit_status(&project.to_string_lossy()),
        crate::models::AiAuditStatus::Audited
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_human_doc_folder_is_refused_on_both_routes() {
    let h = harness().await;
    std::fs::create_dir_all(h.project.path().join("doc")).unwrap();
    std::fs::write(h.project.path().join("doc/manual.md"), "# Manual\n").unwrap();
    refused_on_both_routes(&h, "docs_folder_choice_required", "doc/legacy/").await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_ai_folder_is_migrated_first_or_refused_as_human_docs() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("ai")).unwrap();
    std::fs::write(project.join("ai/index.md"), "# Legacy Kronn index\n").unwrap();
    refused_on_both_routes(&h, "docs_migration_required", "Migrate it to docs/").await;
    // The section refresh refuses the same layout.
    let response = crate::api::audit::drift::partial_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![2],
        }),
    )
    .await
    .into_response();
    let body = sse_body(response).await;
    let error = sse_events(&body, "error").pop().expect("refused");
    assert_eq!(error["error_code"], "docs_migration_required", "{body}");
    assert!(!project.join("docs").exists());

    // A human `ai/` folder is a folder like any other.
    std::fs::remove_file(project.join("ai/index.md")).unwrap();
    std::fs::write(project.join("ai/prompts.md"), "# Prompts\n").unwrap();
    refused_on_both_routes(&h, "docs_folder_choice_required", "ai/legacy/").await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn old_kronn_docs_get_the_files_they_lack_and_keep_the_others() {
    let h = harness().await;
    let project = h.project.path();
    audited(&h, &DocsRoot::default_root()).await;
    let spec = project.join("docs/conventions/agents-md-format-v1.md");
    std::fs::remove_file(&spec).unwrap();
    let glossary = project.join("docs/glossary.md");
    let mut edited = std::fs::read_to_string(&glossary).unwrap();
    edited.push_str("\nTeam term: kept as written.\n");
    std::fs::write(&glossary, &edited).unwrap();
    std::fs::write(project.join("docs/team-notes.md"), "# Ours\n").unwrap();
    // A top-up leaves the root agent files alone, even one the team deleted.
    std::fs::remove_file(project.join("CLAUDE.md")).unwrap();
    let root_files: Vec<_> = ["AGENTS.md", "CLAUDE.md"]
        .iter()
        .map(|f| (f, std::fs::read(project.join(f)).ok()))
        .collect();

    audited(&h, &DocsRoot::default_root()).await;
    assert!(spec.is_file(), "the missing spec is installed again");
    assert!(std::fs::read_to_string(&glossary)
        .unwrap()
        .contains("Team term: kept as written."));
    assert_eq!(
        std::fs::read_to_string(project.join("docs/team-notes.md")).unwrap(),
        "# Ours\n"
    );
    for (file, before) in root_files {
        assert_eq!(std::fs::read(project.join(file)).ok(), before, "{file}");
    }
    assert!(!project.join("docs/legacy").exists());
}

// ─── A first audit that stopped early stays Kronn's ────────────────────────

fn not_asked_about_the_folder(body: &str) {
    assert!(
        sse_events(body, "error")
            .iter()
            .all(|e| e["error_code"] != "docs_folder_choice_required"),
        "{body}"
    );
    assert!(!sse_events(body, "start").is_empty(), "{body}");
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_first_audit_stopped_in_step_1_relaunches_fresh_on_both_routes() {
    let h = harness().await;
    let project = h.project.path();
    let targets = targets_for(&DocsRoot::default_root());
    arm(h.tools.path(), "fill", &targets);
    fail_steps(h.tools.path(), &[1]);
    let body = run_full(&h.state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    assert_ne!(done["status"], "complete", "{body}");
    // The install recorded the folder as Kronn's before step 1 ran.
    let state = crate::core::kronn_state::read(project).expect(".kronn.json");
    assert!(state.template_installed_at.is_some(), "{state:?}");
    assert!(!state.has_any_audit(), "{state:?}");
    assert_eq!(
        crate::core::docs_folder::folder_state(project, "docs"),
        crate::core::docs_folder::DocsFolderState::KronnManaged
    );

    std::fs::write(project.join("NOTES.md"), "# Team notes\n").unwrap();
    for bridge in [false, true] {
        arm(h.tools.path(), "fill", &targets);
        let body = launch_route_fresh(&h, bridge, Some(true)).await;
        not_asked_about_the_folder(&body);
        let done = sse_events(&body, "done").pop().expect("terminal done");
        validation_settled(&h.state, &done).await;
    }
    assert_eq!(
        std::fs::read_to_string(project.join("NOTES.md")).unwrap(),
        "# Team notes\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("README.md")).unwrap(),
        "# Fixture\n"
    );
    assert!(!project.join("docs/legacy").exists());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_cancelled_first_run_without_state_is_kronns_only_with_a_recorded_run() {
    let h = harness().await;
    let project = h.project.path();
    // Step 1 of a cancelled first run wrote the entry; nothing recorded on disk.
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(
        project.join("docs/AGENTS.md"),
        "<!-- kronn:doc-version=\"1.0\" -->\n# Project\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" -->\nBody\n<!-- kronn:section:end -->\n",
    )
    .unwrap();
    // Without a run of this instance, markers alone are a human's docs.
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;

    h.state
        .db
        .with_conn(|conn| {
            crate::db::audit_runs::insert_running(
                conn,
                "cancelled-first",
                PROJECT_ID,
                "Full",
                "ClaudeCode",
                chrono::Utc::now(),
            )?;
            crate::db::audit_runs::mark_cancelled(conn, "cancelled-first")
        })
        .await
        .unwrap();
    // The briefing accepts the folder without a choice, then both routes launch.
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", false),
    )
    .await
    .unwrap();
    for bridge in [false, true] {
        arm(
            h.tools.path(),
            "fill",
            &targets_for(&DocsRoot::default_root()),
        );
        let body = launch_route_fresh(&h, bridge, Some(true)).await;
        not_asked_about_the_folder(&body);
        let done = sse_events(&body, "done").pop().expect("terminal done");
        validation_settled(&h.state, &done).await;
    }
    assert!(project
        .join("docs/conventions/agents-md-format-v1.md")
        .is_file());
    assert!(!project.join("docs/legacy").exists());

    // A run recorded for the project never makes an unmarked folder Kronn's.
    std::fs::create_dir_all(project.join("manual")).unwrap();
    std::fs::write(project.join("manual/guide.md"), "# Guide\n").unwrap();
    assert!(matches!(
        crate::core::docs_folder::folder_state_with_runs(project, "manual", true),
        crate::core::docs_folder::DocsFolderState::HumanDocs { .. }
    ));
}

// ─── Verification: an applied merge under `.ai/`, a golden for `docs/` ──────

fn git_in(dir: &Path, args: &[&str]) {
    let out = crate::core::cmd::git_cmd()
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The merge is APPLIED (worktree written, files accepted and committed), not
/// only previewed: every file lands under `.ai/`, none under `docs/`, in the
/// worktree and in the commit, and the checkout stays untouched.
#[test]
fn an_applied_template_merge_on_a_dot_ai_project_writes_and_commits_under_its_root() {
    use crate::api::projects::template_merge::{
        accept_merge, discard_merge, run_merge_in_worktree, MergeBase,
    };
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("repo");
    std::fs::create_dir_all(&project).unwrap();
    git_in(&project, &["init", "-q", "-b", "main"]);
    std::fs::write(
        project.join("AGENTS.md"),
        "# Agent guide\n\n## Running tests\n\nmake test\n",
    )
    .unwrap();
    std::fs::write(project.join("README.md"), "# Repo\n").unwrap();
    git_in(&project, &["add", "-A"]);
    git_in(&project, &["commit", "-q", "-m", "init"]);
    let _relocated = relocate(&project, ".ai");
    let templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");

    let view = run_merge_in_worktree(&project, "Relocated", MergeBase::Local, &templates).unwrap();
    assert_eq!(view.docs_dir, ".ai");
    let worktree = PathBuf::from(view.worktree_path.clone().expect("a merge worktree"));
    assert!(!view.files.is_empty());
    assert!(
        view.files.iter().all(|f| f.path.starts_with(".ai/")),
        "{:?}",
        view.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    // Written for real, relocated, and nothing under docs/ anywhere.
    let entry = std::fs::read_to_string(worktree.join(".ai/AGENTS.md")).unwrap();
    assert!(!entry.contains("`docs/"), "{entry}");
    assert!(worktree.join(".ai/conventions/README.md").is_file());
    assert!(
        !worktree.join("docs").exists(),
        "no docs/ in the merge worktree"
    );
    assert!(!project.join("docs").exists(), "no docs/ in the checkout");
    assert!(!project.join(".ai").exists(), "the checkout is not written");

    let accepted = accept_merge(
        &project,
        &[".ai/AGENTS.md".to_string(), ".ai/glossary.md".to_string()],
    )
    .unwrap();
    let shown = crate::core::cmd::git_cmd()
        .args(["show", "--name-only", "--format=", &accepted.commit])
        .current_dir(&worktree)
        .output()
        .unwrap();
    let committed: BTreeSet<String> = String::from_utf8_lossy(&shown.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(
        committed,
        [".ai/AGENTS.md", ".ai/glossary.md"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>()
    );
    // A path outside the root cannot be accepted into the commit.
    assert!(accept_merge(&project, &["docs/AGENTS.md".to_string()]).is_err());
    discard_merge(&project).unwrap();
}

/// What a default-root Full audit leaves in the project, one line per file:
/// the sha256 of its bytes (dates, the project location and name masked), or
/// `volatile` for state that embeds run ids and hashes of other files.
fn default_root_snapshot(project: &Path, prompts: &[String]) -> String {
    let date =
        regex_lite::Regex::new(r"\d{4}-\d{2}-\d{2}([T ][0-9:.]+(Z|[+-]\d{2}:?\d{2})?)?").unwrap();
    // Every step prompt opens with "(YYYYMMDD: <today>)": the compact form of
    // the same date, which the golden must not depend on either.
    let compact =
        regex_lite::Regex::new(r"\b20\d{2}(0[1-9]|1[0-2])(0[1-9]|[12]\d|3[01])\b").unwrap();
    let location = project.display().to_string();
    let canonical = project
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| location.clone());
    let name = project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mask = |text: &str| {
        let text = text
            .replace(&canonical, "<PROJECT>")
            .replace(&location, "<PROJECT>")
            .replace(&name, "<NAME>");
        let text = date.replace_all(&text, "<DATE>").into_owned();
        compact.replace_all(&text, "<DATE>").into_owned()
    };
    let mut lines: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(project)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry
            .path()
            .strip_prefix(project)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with(".git/") {
            continue;
        }
        let bytes = std::fs::read(entry.path()).unwrap();
        // A validated original is named by the sha256 of its bytes, which hold
        // the audit date (`audit="YYYY-MM-DD"`): the golden would change every
        // day. Name it by the sha of the masked bytes instead.
        let rel = match rel.strip_prefix(".kronn/audit/validated-originals/") {
            Some(_) => {
                let masked = mask(&String::from_utf8_lossy(&bytes));
                format!(
                    ".kronn/audit/validated-originals/{}.bak",
                    crate::core::docs_merge::sha256_hex(masked.as_bytes())
                )
            }
            None => rel,
        };
        let volatile = matches!(
            rel.as_str(),
            "docs/.kronn.json"
                // Carries the operator label and the run id (KT-1199).
                | "docs/.kronn-audit-run.json"
                | "docs/checksums.json"
                | "docs/citations.json"
                | "docs/provenance.json"
        ) || rel.starts_with(".kronn/audit/")
            // MCP host sync writes machine-specific paths into these.
            || matches!(
                rel.as_str(),
                ".mcp.json"
                    | ".ai/mcp/mcp.json"
                    | ".gemini/settings.json"
                    | ".kiro/settings/mcp.json"
                    | ".vibe/config.toml"
            );
        let digest = if volatile {
            "volatile".to_string()
        } else {
            let masked = mask(&String::from_utf8_lossy(&bytes));
            crate::core::docs_merge::sha256_hex(masked.as_bytes())
        };
        lines.push(format!("{digest}  {rel}"));
    }
    for (n, prompt) in prompts.iter().enumerate() {
        let digest = crate::core::docs_merge::sha256_hex(mask(prompt).as_bytes());
        lines.push(format!("{digest}  prompt.{}", n + 1));
    }
    lines.join("\n") + "\n"
}

/// With the default `docs/` root a Full audit produces, byte for byte, the
/// artefacts and step prompts recorded in `default_root.golden`. An intended
/// change to a template or a prompt updates the golden in the same commit:
/// the failure message prints the new content.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_default_docs_root_audit_is_byte_identical_to_its_golden() {
    let h = harness().await;
    let prompts = audited(&h, &DocsRoot::default_root()).await;
    let actual = default_root_snapshot(h.project.path(), &prompts);
    assert!(
        actual.lines().any(|l| l.ends_with("  docs/AGENTS.md")),
        "{actual}"
    );
    let golden = include_str!("default_root.golden");
    if actual != golden {
        let proposed = tempfile::Builder::new()
            .prefix("kronn-default-root-")
            .suffix(".golden")
            .tempfile()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        std::fs::write(&proposed, &actual).unwrap();
        let (a, g): (Vec<&str>, Vec<&str>) = (actual.lines().collect(), golden.lines().collect());
        let differing: Vec<String> = a
            .iter()
            .filter(|l| !g.contains(l))
            .map(|l| format!("+ {l}"))
            .chain(
                g.iter()
                    .filter(|l| !a.contains(l))
                    .map(|l| format!("- {l}")),
            )
            .collect();
        panic!(
            "the default-root audit output changed; if intended, replace default_root.golden with {}:\n{}",
            proposed.display(),
            differing.join("\n")
        );
    }
}

// ─── KT-1299 — Kronn's own template files are never "foreign" docs ─────────

async fn install_template_route(h: &Harness) {
    let response = crate::api::projects::install_template(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0;
    assert!(response.success, "{:?}", response.error);
}

fn assert_not_asked(body: &str) {
    assert!(
        sse_events(body, "error")
            .iter()
            .all(|e| e["error_code"] != "docs_folder_choice_required"),
        "{body}"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn installing_the_template_then_launching_the_full_audit_starts_on_both_routes() {
    let h = harness().await;
    let project = h.project.path();
    assert!(!project.join("docs").exists());
    install_template_route(&h).await;
    assert!(project.join("docs/AGENTS.md").is_file());
    // Everything the install wrote is Kronn's: the gate counts none of it.
    assert_eq!(
        crate::core::docs_folder::folder_state(project, "docs"),
        crate::core::docs_folder::DocsFolderState::KronnManaged
    );

    // The briefing keeps the folder; ticking "move to legacy" moves nothing of it.
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    assert!(!project.join("docs/legacy").exists());
    assert!(project.join("docs/AGENTS.md").is_file());

    audited_via_route(&h, &DocsRoot::default_root(), false).await;
    arm(
        h.tools.path(),
        "noop",
        &targets_for(&DocsRoot::default_root()),
    );
    assert_not_asked(&launch_route(&h, true).await);
    assert!(!project.join("docs/legacy").exists());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn the_template_installed_beside_human_docs_does_not_make_them_kronns() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    install_template_route(&h).await;
    // The human's files keep the folder theirs: the choice is still due.
    assert!(matches!(
        crate::core::docs_folder::folder_state(project, "docs"),
        crate::core::docs_folder::DocsFolderState::HumanDocs { .. }
    ));
    let state = crate::core::kronn_state::read(project).expect(".kronn.json");
    assert!(state.template_installed_at.is_none(), "{state:?}");
    // The install recorded exactly what it wrote: every file but the human's
    // two and Kronn's state file.
    let human = ["install.md", "guide/usage.md"];
    let template_files: BTreeSet<String> = files_under(&project.join("docs"))
        .into_iter()
        .filter(|f| !human.contains(&f.as_str()) && f != ".kronn.json" && !f.starts_with("var/"))
        .collect();
    assert!(template_files.contains("AGENTS.md"), "{template_files:?}");
    assert_eq!(
        state.kronn_written.keys().cloned().collect::<BTreeSet<_>>(),
        template_files
    );
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;
    let template_bytes: std::collections::BTreeMap<String, Vec<u8>> = template_files
        .iter()
        .map(|f| {
            (
                f.clone(),
                std::fs::read(project.join("docs").join(f)).unwrap(),
            )
        })
        .collect();

    // The choice unblocks the launch although the entry file already carries
    // Kronn's signature, and moves the human's files.
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    assert!(project.join("docs/legacy/install.md").is_file());
    assert!(project.join("docs/legacy/guide/usage.md").is_file());
    assert!(!project.join("docs/install.md").exists());
    // Only the human's files moved: every template file stays in its place,
    // byte for byte.
    for (file, bytes) in &template_bytes {
        assert_eq!(
            &std::fs::read(project.join("docs").join(file))
                .unwrap_or_else(|_| panic!("{file} moved")),
            bytes,
            "{file} changed"
        );
        assert!(!project.join("docs/legacy").join(file).exists(), "{file}");
    }
    let moved: BTreeSet<String> = files_under(&project.join("docs/legacy"))
        .into_iter()
        .filter(|f| f != "README.md")
        .collect();
    assert_eq!(
        moved,
        human.iter().map(|f| f.to_string()).collect::<BTreeSet<_>>()
    );
    for state_file in ["briefing.md", ".kronn.json"] {
        assert!(
            project.join("docs").join(state_file).is_file(),
            "{state_file}"
        );
        assert!(!project.join("docs/legacy").join(state_file).exists());
    }
    audited_via_route(&h, &DocsRoot::default_root(), false).await;
    assert_eq!(
        std::fs::read_to_string(project.join("docs/legacy/install.md")).unwrap(),
        "# Install guide\n"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn installing_never_turns_a_human_folder_with_an_earlier_audit_into_kronns() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    // An earlier audit is recorded, yet no file carries a Kronn marker.
    crate::core::kronn_state::record_audit(project, "full").unwrap();
    let is_human = |project: &Path| {
        matches!(
            crate::core::docs_folder::folder_state(project, "docs"),
            crate::core::docs_folder::DocsFolderState::HumanDocs { .. }
        )
    };
    assert!(is_human(project));

    install_template_route(&h).await;
    // Still the human's: the install's own marked files are no proof.
    assert!(project.join("docs/AGENTS.md").is_file());
    assert!(is_human(project));
    let state = crate::core::kronn_state::read(project).expect(".kronn.json");
    assert!(state.template_installed_at.is_none(), "{state:?}");
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;

    // Once the human chose, the folder is Kronn's and the audit runs.
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    assert!(project.join("docs/legacy/install.md").is_file());
    assert!(project.join("docs/AGENTS.md").is_file());
    audited_via_route(&h, &DocsRoot::default_root(), false).await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn template_files_the_human_overwrote_are_theirs_again_and_asked_about() {
    let h = harness().await;
    let project = h.project.path();
    install_template_route(&h).await;
    crate::core::kronn_state::record_audit(project, "full").unwrap();
    assert_eq!(
        crate::core::docs_folder::folder_state(project, "docs"),
        crate::core::docs_folder::DocsFolderState::KronnManaged
    );

    // The human replaces every recorded file with their own text: the record
    // names them, but they no longer hold the bytes Kronn wrote.
    let recorded: BTreeSet<String> = crate::core::kronn_state::read(project)
        .expect(".kronn.json")
        .kronn_written
        .into_keys()
        .collect();
    assert!(recorded.contains("AGENTS.md") && recorded.contains("glossary.md"));
    for rel in &recorded {
        std::fs::write(project.join("docs").join(rel), format!("# Mine: {rel}\n")).unwrap();
    }
    let state = crate::core::docs_folder::folder_state(project, "docs");
    assert!(
        matches!(
            state,
            crate::core::docs_folder::DocsFolderState::HumanDocs { files, .. }
                if files as usize == recorded.len()
        ),
        "{state:?}"
    );
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;

    // On the choice exactly those files move, with the human's text; the audit
    // restores the template in their place.
    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    let moved: BTreeSet<String> = files_under(&project.join("docs/legacy"))
        .into_iter()
        .filter(|f| f != "README.md")
        .collect();
    assert_eq!(moved, recorded);
    for rel in &recorded {
        assert_eq!(
            std::fs::read_to_string(project.join("docs/legacy").join(rel)).unwrap(),
            format!("# Mine: {rel}\n")
        );
    }
    audited_via_route(&h, &DocsRoot::default_root(), false).await;
    assert!(project.join("docs/AGENTS.md").is_file());
    assert_ne!(
        std::fs::read_to_string(project.join("docs/AGENTS.md")).unwrap(),
        "# Mine: AGENTS.md\n"
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_install_over_a_corrupt_state_file_writes_nothing_unrecorded() {
    let h = harness().await;
    let project = h.project.path();
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(project.join("docs/.kronn.json"), "{ torn").unwrap();
    human_docs(project);
    let before = snapshot(project);
    let response = crate::api::projects::install_template(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0;
    assert!(!response.success);
    assert_eq!(snapshot(project), before);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_kronn_signed_entry_without_a_record_still_moves_on_the_choice() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    std::fs::write(
        project.join("docs/AGENTS.md"),
        "# AI agent context — Entry point\n\nWritten by an old Kronn, never recorded.\n",
    )
    .unwrap();
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;

    let _cleanup = Relocated(project.to_string_lossy().into_owned());
    crate::api::audit::briefing::save_briefing_answers(
        &h.state,
        PROJECT_ID,
        briefing("docs", true),
    )
    .await
    .unwrap();
    for moved in ["install.md", "guide/usage.md", "AGENTS.md"] {
        assert!(project.join("docs/legacy").join(moved).is_file(), "{moved}");
    }
    let body = launch_route(&h, false).await;
    assert_not_asked(&body);
    assert!(!sse_events(&body, "start").is_empty(), "{body}");
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn partial_and_revalidation_wait_for_the_choice_on_human_docs() {
    let h = harness().await;
    let project = h.project.path();
    human_docs(project);
    arm(h.tools.path(), "fill", &["docs/glossary.md"]);
    let before = snapshot(project);

    let partial = crate::api::audit::drift::partial_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![1],
        }),
    )
    .await
    .into_response();
    let revalidation = crate::api::audit::revalidate_audit(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::RevalidateAuditRequest {
            deterministic_only: Some(true),
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
        }),
    )
    .await
    .into_response();
    for response in [partial, revalidation] {
        let body = sse_body(response).await;
        let error = sse_events(&body, "error").pop().expect("refused");
        assert_eq!(error["error_code"], "docs_folder_choice_required", "{body}");
        assert!(sse_events(&body, "start").is_empty(), "{body}");
    }
    assert_eq!(snapshot(project), before);
}
