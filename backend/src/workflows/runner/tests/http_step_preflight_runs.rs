//! KT-2045 — an Agent step on a native HTTP provider (`LiteLlm`, `Nvidia`) is
//! refused by the run preflight, before any step runs, when the run would
//! refuse it for its named connection (gone, endpoint-less, keyless where the
//! preset needs a key) or, without a connection, for its provider key. A
//! valid pair still starts; a CLI step keeps today's check.
//!
//! No socket and no mock server (the gate runs in a sandbox that refuses
//! loopback): a refused run never reaches HTTP, and the "still starts" run
//! pauses on a Gate placed before its HTTP steps. Every key value is a fake.

use super::*;
use crate::db::external_api_connections::NVIDIA_CREDENTIAL_SLUG;
use crate::models::{ExternalApiConnection, ExternalApiConnectionPreset};

const FAKE_KEY: &str = "fake-http-step-preflight-key";

fn connection(
    id: &str,
    endpoint: Option<&str>,
    preset: ExternalApiConnectionPreset,
) -> ExternalApiConnection {
    let now = chrono::Utc::now();
    ExternalApiConnection {
        id: id.into(),
        display_name: format!("{id} display"),
        mention_alias: id.into(),
        endpoint: endpoint.map(str::to_string),
        credential_slug: format!("credential-{id}"),
        origin_preset: preset,
        economy_model: None,
        default_model: Some("vendor/model-fixture".into()),
        reasoning_model: None,
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        decision_model: None,
        tools_enabled: Some(false),
        media_endpoint: None,
    }
}

async fn insert_connection(state: &crate::AppState, connection: ExternalApiConnection) {
    let target = crate::db::model_catalog::http_runtime_target_id(&connection.id);
    let agent = crate::db::external_api_connections::target_for_connection(&connection).agent_type;
    let model = connection.default_model.clone().unwrap();
    state
        .db
        .with_conn(move |conn| {
            crate::db::external_api_connections::insert(conn, &connection)?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &agent,
                &[crate::db::model_catalog::DiscoveredModel {
                    model_id: model.clone(),
                    display_name: model,
                    resolved_model: None,
                    description: None,
                    capabilities: vec!["chat".into()],
                    reasoning_modes: Vec::new(),
                    default_reasoning_mode: None,
                }],
            )
        })
        .await
        .unwrap();
}

fn api_key(provider: &str) -> crate::models::ApiKey {
    crate::models::ApiKey {
        id: format!("key-{provider}"),
        name: "fixture key".into(),
        provider: provider.into(),
        value: FAKE_KEY.into(),
        active: true,
    }
}

fn no_tokens() -> TokensConfig {
    TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    }
}

fn http_step(name: &str, agent: AgentType, connection_id: Option<&str>) -> WorkflowStep {
    let mut step = fake_step(name);
    step.agent = agent;
    step.prompt_template = "Answer".into();
    step.agent_settings = Some(crate::models::AgentSettings {
        tier: Some(crate::models::ModelTier::Default),
        connection_id: connection_id.map(str::to_string),
        ..Default::default()
    });
    step
}

fn gate(name: &str) -> WorkflowStep {
    let mut gate = fake_step(name);
    gate.step_type = StepType::Gate;
    gate
}

struct Setup {
    connections: Vec<ExternalApiConnection>,
    /// Providers (credential slugs) holding a fake active key.
    keys: Vec<String>,
}

/// Runs `steps` after a Gate named `porte`: a step that ran before the
/// preflight refused would leave the Gate's pause, never `__preflight__` alone.
async fn run_after_gate(id: &str, steps: Vec<WorkflowStep>, setup: Setup) -> WorkflowRun {
    run_after_gate_with(
        id,
        steps,
        setup,
        Extras {
            declared_proxy: true,
            ..Extras::default()
        },
    )
    .await
}

/// What a KT-2055 run adds to its setup.
#[derive(Default)]
struct Extras {
    /// A declared LiteLLM proxy (`lite_llm.base_url`).
    declared_proxy: bool,
    quick_prompts: Vec<crate::models::QuickPrompt>,
    /// Workflows the run's SubWorkflow steps start.
    children: Vec<Workflow>,
}

async fn run_after_gate_with(
    id: &str,
    steps: Vec<WorkflowStep>,
    setup: Setup,
    extras: Extras,
) -> WorkflowRun {
    let (state, mut tokens, mut agents) = test_state_and_configs();
    // A declared LiteLLM proxy makes `LiteLlm` launchable without a local
    // binary, so the refusals below are about the connection or the key only.
    agents.lite_llm.base_url = extras
        .declared_proxy
        .then(|| "https://litellm-proxy.invalid".into());
    for connection in setup.connections {
        insert_connection(&state, connection).await;
    }
    for prompt in extras.quick_prompts {
        // Deleting a connection clears it from a live Quick Prompt; only a
        // revision pinned with a run still names it. Written as such a stale
        // reference, with the foreign key off for this one insert.
        state
            .db
            .with_conn(move |conn| {
                conn.execute_batch("PRAGMA foreign_keys = OFF")?;
                let inserted = crate::db::quick_prompts::insert_quick_prompt(conn, &prompt);
                conn.execute_batch("PRAGMA foreign_keys = ON")?;
                inserted
            })
            .await
            .unwrap();
    }
    for child in extras.children {
        state
            .db
            .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &child))
            .await
            .unwrap();
    }
    for provider in &setup.keys {
        tokens.keys.push(api_key(provider));
        state
            .config
            .write()
            .await
            .tokens
            .keys
            .push(api_key(provider));
    }
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.steps = std::iter::once(gate("porte")).chain(steps).collect();
    let mut run = pending_run(&format!("run-{id}"), &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    run
}

/// The refusal, asserting it came before any step ran and names `step`.
fn refused_before_any_step<'a>(run: &'a WorkflowRun, step: &str) -> &'a str {
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert_eq!(
        run.step_results.len(),
        1,
        "no step may run before the refusal: {:?}",
        run.step_results
    );
    let refusal = &run.step_results[0];
    assert_eq!(refusal.step_name, "__preflight__", "{:?}", run.step_results);
    assert!(
        !refusal.output.contains("Install or enable"),
        "never asks to install an HTTP provider: {}",
        refusal.output
    );
    assert!(
        refusal.output.contains(&format!("'{step}'")),
        "names the step: {}",
        refusal.output
    );
    assert!(
        !refusal.output.contains(FAKE_KEY),
        "never echoes a key: {}",
        refusal.output
    );
    &refusal.output
}

#[tokio::test]
async fn http_step_preflight_litellm_step_on_a_deleted_connection_is_refused_before_any_step_runs()
{
    let run = run_after_gate(
        "litellm-gone",
        vec![http_step("jeu", AgentType::LiteLlm, Some("conn-gone"))],
        Setup {
            connections: Vec::new(),
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-gone"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("not found"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_litellm_step_on_an_endpoint_less_connection_is_refused_before_any_step_runs(
) {
    let run = run_after_gate(
        "litellm-no-endpoint",
        vec![http_step("jeu", AgentType::LiteLlm, Some("conn-noend"))],
        Setup {
            connections: vec![connection(
                "conn-noend",
                None,
                ExternalApiConnectionPreset::LiteLlm,
            )],
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-noend"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("no HTTP endpoint"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_nvidia_step_on_a_keyless_nvidia_connection_is_refused_before_any_step_runs(
) {
    // The NVIDIA provider slot holds a key: it is not this connection's.
    let run = run_after_gate(
        "nvidia-keyless-connection",
        vec![http_step("jeu", AgentType::Nvidia, Some("conn-nv"))],
        Setup {
            connections: vec![connection(
                "conn-nv",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-nv display"),
        "names the connection: {refusal}"
    );
    assert!(refusal.contains("no API key"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_nvidia_step_without_any_key_is_refused_before_any_step_runs() {
    let run = run_after_gate(
        "nvidia-no-key",
        vec![http_step("jeu", AgentType::Nvidia, None)],
        Setup {
            connections: Vec::new(),
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(refusal.contains("NVIDIA"), "names the provider: {refusal}");
    assert!(refusal.contains("no NVIDIA API key"), "{refusal}");
}

#[tokio::test]
async fn http_step_preflight_valid_http_steps_still_start() {
    let run = run_after_gate(
        "valid-pairs",
        vec![
            // A LiteLLM proxy may run keyless.
            http_step("proxy", AgentType::LiteLlm, Some("conn-proxy")),
            http_step("nv-named", AgentType::Nvidia, Some("conn-nv-ok")),
            http_step("nv-slot", AgentType::Nvidia, None),
        ],
        Setup {
            connections: vec![
                connection(
                    "conn-proxy",
                    Some("https://proxy.invalid/v1"),
                    ExternalApiConnectionPreset::LiteLlm,
                ),
                connection(
                    "conn-nv-ok",
                    Some("https://nvidia.invalid/v1"),
                    ExternalApiConnectionPreset::Nvidia,
                ),
            ],
            keys: vec![
                "credential-conn-nv-ok".into(),
                NVIDIA_CREDENTIAL_SLUG.into(),
            ],
        },
    )
    .await;
    assert!(
        run.step_results
            .iter()
            .all(|result| result.step_name != "__preflight__"),
        "a valid pair passes the preflight: {:?}",
        run.step_results
    );
    // It started: the first step (the Gate) ran and paused the run.
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );
    assert_eq!(run.step_results[0].step_name, "porte");
}

#[tokio::test]
async fn http_step_preflight_reason_names_step_connection_and_what_is_missing() {
    let state = test_state_and_configs().0;
    insert_connection(
        &state,
        connection(
            "conn-nv-reason",
            Some("https://nvidia.invalid/v1"),
            ExternalApiConnectionPreset::Nvidia,
        ),
    )
    .await;
    let tokens = no_tokens();
    let named = http_step("jeu", AgentType::Nvidia, Some("conn-nv-reason"));
    let refusal =
        crate::workflows::steps::step_connection_refusal(&named, Some(&state.db), &tokens)
            .await
            .expect("a keyless NVIDIA connection is refused");
    for part in ["'jeu'", "conn-nv-reason display", "no API key"] {
        assert!(refusal.contains(part), "{part} missing from: {refusal}");
    }
    let bare = http_step("jeu", AgentType::Nvidia, None);
    let refusal = crate::workflows::steps::step_connection_refusal(&bare, Some(&state.db), &tokens)
        .await
        .expect("an NVIDIA step without a key is refused");
    for part in ["'jeu'", "NVIDIA", "API key"] {
        assert!(refusal.contains(part), "{part} missing from: {refusal}");
    }
}

#[tokio::test]
async fn http_step_preflight_blank_provider_key_counts_as_missing() {
    let state = test_state_and_configs().0;
    let mut tokens = no_tokens();
    tokens.keys.push(crate::models::ApiKey {
        value: "   ".into(),
        ..api_key(NVIDIA_CREDENTIAL_SLUG)
    });
    let step = http_step("jeu", AgentType::Nvidia, None);
    assert!(
        crate::workflows::steps::step_connection_refusal(&step, Some(&state.db), &tokens)
            .await
            .is_some(),
        "a whitespace-only NVIDIA key is no key"
    );
}

#[tokio::test]
async fn http_step_preflight_keyless_litellm_without_connection_is_not_refused() {
    // A LiteLLM proxy may run without a master key (KT-2031): its readiness
    // stays true, so the preflight does not refuse it on its key.
    let state = test_state_and_configs().0;
    let step = http_step("jeu", AgentType::LiteLlm, None);
    assert!(
        crate::workflows::steps::step_connection_refusal(&step, Some(&state.db), &no_tokens())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn http_step_preflight_cli_steps_keep_todays_check() {
    // A CLI owns its login: no key, no connection, still never refused here;
    // its install/enable check stays `agent_can_launch`.
    let state = test_state_and_configs().0;
    for agent in [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::OpenCode,
        AgentType::Vibe,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::Ollama,
    ] {
        let step = http_step("jeu", agent.clone(), None);
        assert!(
            crate::workflows::steps::step_connection_refusal(&step, Some(&state.db), &no_tokens())
                .await
                .is_none(),
            "{agent:?} must not be refused on a key"
        );
    }
}

// ─── KT-2055: the steps as they run, children, hook, resume ─────────────

fn quick_prompt(id: &str, connection_id: Option<&str>) -> crate::models::QuickPrompt {
    crate::models::QuickPrompt {
        id: id.into(),
        pinned: false,
        name: format!("{id} name"),
        icon: "P".into(),
        prompt_template: "Answer from the Quick Prompt".into(),
        variables: Vec::new(),
        agent: AgentType::Nvidia,
        connection_id: connection_id.map(str::to_string),
        project_id: None,
        skill_ids: Vec::new(),
        profile_ids: Vec::new(),
        directive_ids: Vec::new(),
        tier: crate::models::ModelTier::Default,
        agent_settings: None,
        description: String::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// An NVIDIA step naming no connection, run from Quick Prompt `qp`.
fn qp_step(name: &str, qp: &str) -> WorkflowStep {
    let mut step = http_step(name, AgentType::Nvidia, None);
    step.prompt_template = String::new();
    step.quick_prompt_id = Some(qp.into());
    step
}

fn passed_preflight(run: &WorkflowRun) {
    assert!(
        run.step_results
            .iter()
            .all(|result| result.step_name != "__preflight__"),
        "the preflight passes: {:?}",
        run.step_results
    );
    assert_eq!(
        run.status,
        RunStatus::WaitingApproval,
        "{:?}",
        run.step_results
    );
    assert_eq!(run.step_results[0].step_name, "porte");
}

#[tokio::test]
async fn http_step_preflight_nvidia_step_keyed_by_its_quick_prompts_connection_runs_with_an_empty_slot(
) {
    // No NVIDIA slot key: the step runs on the connection its QP names.
    let run = run_after_gate_with(
        "qp-keyed",
        vec![qp_step("jeu", "qp-keyed")],
        Setup {
            connections: vec![connection(
                "conn-qp-nv",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec!["credential-conn-qp-nv".into()],
        },
        Extras {
            quick_prompts: vec![quick_prompt("qp-keyed", Some("conn-qp-nv"))],
            ..Extras::default()
        },
    )
    .await;
    passed_preflight(&run);
}

#[tokio::test]
async fn http_step_preflight_quick_prompt_naming_a_gone_or_keyless_connection_is_refused_before_any_step_runs(
) {
    // The NVIDIA slot holds a key: the raw step would pass on it.
    let run = run_after_gate_with(
        "qp-gone",
        vec![qp_step("jeu", "qp-gone")],
        Setup {
            connections: Vec::new(),
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
        Extras {
            quick_prompts: vec![quick_prompt("qp-gone", Some("conn-qp-gone"))],
            ..Extras::default()
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-qp-gone") && refusal.contains("not found"),
        "{refusal}"
    );

    let run = run_after_gate_with(
        "qp-keyless",
        vec![qp_step("jeu", "qp-keyless")],
        Setup {
            connections: vec![connection(
                "conn-qp-keyless",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
        Extras {
            quick_prompts: vec![quick_prompt("qp-keyless", Some("conn-qp-keyless"))],
            ..Extras::default()
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("conn-qp-keyless display") && refusal.contains("no API key"),
        "{refusal}"
    );
}

#[tokio::test]
async fn http_step_preflight_provider_step_on_a_connection_needs_no_binary_nor_base_url() {
    // No declared proxy, and the host's `litellm` binary does not count:
    // both steps run on their named connection.
    let run = run_after_gate_with(
        "no-binary",
        vec![
            http_step("proxy", AgentType::LiteLlm, Some("conn-proxy-2")),
            http_step("nv", AgentType::Nvidia, Some("conn-nv-2")),
        ],
        Setup {
            connections: vec![
                connection(
                    "conn-proxy-2",
                    Some("https://proxy-2.invalid/v1"),
                    ExternalApiConnectionPreset::LiteLlm,
                ),
                connection(
                    "conn-nv-2",
                    Some("https://nvidia.invalid/v1"),
                    ExternalApiConnectionPreset::Nvidia,
                ),
            ],
            keys: vec!["credential-conn-nv-2".into()],
        },
        Extras::default(),
    )
    .await;
    passed_preflight(&run);
}

#[tokio::test]
async fn http_step_preflight_blank_connection_id_runs_on_the_provider_slot() {
    let run = run_after_gate(
        "blank-id",
        vec![http_step("jeu", AgentType::Nvidia, Some("   "))],
        Setup {
            connections: Vec::new(),
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
    )
    .await;
    passed_preflight(&run);
}

#[tokio::test]
async fn http_step_preflight_a_keyless_reviewer_is_refused_before_any_step_runs() {
    let mut author = http_step("auteur", AgentType::LiteLlm, Some("conn-proxy-r"));
    author.multi_agent_review = Some(crate::models::MultiAgentReviewConfig {
        reviewer_agent: AgentType::Nvidia,
        reviewer_tier: None,
        debate_prompt: "Review it".into(),
        max_rounds: Some(1),
    });
    let run = run_after_gate(
        "reviewer-keyless",
        vec![author],
        Setup {
            connections: vec![connection(
                "conn-proxy-r",
                Some("https://proxy.invalid/v1"),
                ExternalApiConnectionPreset::LiteLlm,
            )],
            keys: Vec::new(),
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "auteur");
    assert!(
        refusal.contains("multi_agent_review reviewer (NVIDIA)")
            && refusal.contains("no NVIDIA API key"),
        "{refusal}"
    );
}

#[tokio::test]
async fn http_step_preflight_a_childs_keyless_step_is_refused_before_the_parents_first_step() {
    let mut start_child = fake_step("lancer");
    start_child.step_type = StepType::SubWorkflow;
    start_child.sub_workflow_id = Some("wf-child-keyless".into());
    // The child starts its parent again: the walk must still end.
    let mut back = fake_step("retour");
    back.step_type = StepType::SubWorkflow;
    back.sub_workflow_id = Some("wf-child-cycle".into());
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-child-keyless".into();
    child.name = "enfant".into();
    child.steps = vec![http_step("jeu-enfant", AgentType::Nvidia, None), back];
    let run = run_after_gate_with(
        "child-cycle",
        vec![start_child],
        Setup {
            connections: Vec::new(),
            keys: Vec::new(),
        },
        Extras {
            declared_proxy: true,
            children: vec![child],
            ..Extras::default()
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu-enfant");
    assert!(
        refusal.contains("sub-workflow `enfant` (step 'lancer')")
            && refusal.contains("no NVIDIA API key"),
        "names where the step lives and why: {refusal}"
    );
}

/// A run of `porte` then an NVIDIA step, in a worktree whose `after_create`
/// and `before_run` hooks leave `after-create` and `before-run` in `markers`.
/// Returns the run, how many worktrees its repository has after it, and the
/// statuses its run updates broadcast.
async fn run_with_hooks(
    id: &str,
    nvidia_key: bool,
    markers: &std::path::Path,
) -> (WorkflowRun, usize, Vec<String>) {
    let (state, mut tokens, agents) = test_state_and_configs();
    let mut updates = state.ws_broadcast.subscribe();
    let project = format!("proj-{id}");
    let repo = git_project(&state, &project).await;
    if nvidia_key {
        tokens.keys.push(api_key(NVIDIA_CREDENTIAL_SLUG));
    }
    let touch = |name: &str| format!("touch '{}'", markers.join(name).display());
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.project_id = Some(project.clone());
    wf.workspace_config = Some(WorkspaceConfig {
        hooks: hooks(Some(&touch("after-create")), Some(&touch("before-run"))),
        require_isolation: true,
        main_tree_read_only: false,
        base_ref: None,
    });
    wf.steps = vec![gate("porte"), http_step("jeu", AgentType::Nvidia, None)];
    let mut run = pending_run(&format!("run-{id}"), &wf.id);
    run.project_id = wf.project_id.clone();
    insert_wf_and_run(&state, &wf, &run).await;
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let worktrees = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["worktree", "list", "--porcelain"])
        .output()
        .unwrap();
    let count = String::from_utf8_lossy(&worktrees.stdout)
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count();
    let mut statuses = Vec::new();
    while let Ok(message) = updates.try_recv() {
        if let crate::models::WsMessage::WorkflowRunUpdated { run_id, status, .. } = message {
            if run_id == run.id {
                statuses.push(status);
            }
        }
    }
    (run, count, statuses)
}

#[tokio::test]
async fn http_step_preflight_a_refused_run_creates_no_worktree_and_runs_no_hook() {
    // KT-2057 — refused before the worktree: no checkout, no `after_create`.
    let refused = tempfile::tempdir().unwrap();
    let (run, worktrees, statuses) = run_with_hooks("hook-refused", false, refused.path()).await;
    refused_before_any_step(&run, "jeu");
    assert!(run.finished_at.is_some(), "a refused run is finished");
    assert_eq!(
        statuses,
        vec!["Failed".to_string()],
        "its state is broadcast"
    );
    assert_eq!(run.workspace_path, None);
    assert_eq!(worktrees, 1, "only the main checkout");
    assert!(
        !refused.path().join("after-create").exists(),
        "the after_create hook ran for a refused run"
    );
    assert!(
        !refused.path().join("before-run").exists(),
        "the before_run hook ran for a refused run"
    );

    // The same run with its key starts in its worktree, and its hooks run.
    let started = tempfile::tempdir().unwrap();
    let (run, worktrees, _) = run_with_hooks("hook-started", true, started.path()).await;
    passed_preflight(&run);
    assert!(run.workspace_path.is_some());
    assert_eq!(worktrees, 2);
    assert!(started.path().join("after-create").exists());
    assert!(started.path().join("before-run").exists());
}

#[tokio::test]
async fn http_step_preflight_a_step_whose_quick_prompt_cannot_load_is_refused_before_any_step_runs()
{
    // KT-2057 — it would fail at dispatch in these words, after `porte`.
    let run = run_after_gate(
        "qp-missing",
        vec![qp_step("jeu", "qp-does-not-exist")],
        Setup {
            connections: Vec::new(),
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu");
    assert!(
        refusal.contains("Quick Prompt(s) that cannot load")
            && refusal.contains("qp-does-not-exist"),
        "{refusal}"
    );
}

/// A `BatchQuickPrompt` step running Quick Prompt `qp` over two items.
fn batch_step(name: &str, qp: &str) -> WorkflowStep {
    let mut step = fake_step(name);
    step.step_type = StepType::BatchQuickPrompt;
    step.batch_quick_prompt_id = Some(qp.into());
    step.batch_items_from = Some(r#"["a","b"]"#.into());
    step
}

#[tokio::test]
async fn http_step_preflight_a_batch_whose_items_run_on_a_keyless_connection_is_refused_before_any_step_runs(
) {
    // KT-2057 — the items run the QP's agent on the QP's connection.
    let run = run_after_gate_with(
        "batch-keyless",
        vec![batch_step("lot", "qp-batch-keyless")],
        Setup {
            connections: vec![connection(
                "conn-batch-keyless",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
        Extras {
            quick_prompts: vec![quick_prompt("qp-batch-keyless", Some("conn-batch-keyless"))],
            ..Extras::default()
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "lot");
    assert!(
        refusal.contains("conn-batch-keyless display") && refusal.contains("no API key"),
        "{refusal}"
    );

    // Keyed, the batch passes the preflight.
    let run = run_after_gate_with(
        "batch-keyed",
        vec![batch_step("lot", "qp-batch-keyed")],
        Setup {
            connections: vec![connection(
                "conn-batch-keyed",
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            )],
            keys: vec!["credential-conn-batch-keyed".into()],
        },
        Extras {
            quick_prompts: vec![quick_prompt("qp-batch-keyed", Some("conn-batch-keyed"))],
            ..Extras::default()
        },
    )
    .await;
    passed_preflight(&run);
}

#[tokio::test]
async fn http_step_dispatch_a_resumed_runs_keyless_step_is_refused_before_any_request() {
    // A resumed run skips the preflight (`is_resume`): the key was removed
    // while it waited. Dispatch refuses the step in the preflight's words.
    let (state, tokens, agents) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-resumed-keyless".into();
    wf.steps = vec![
        json_data_step("prep", serde_json::json!({ "ok": true })),
        http_step("jeu", AgentType::Nvidia, None),
    ];
    let mut run = pending_run("run-resumed-keyless", &wf.id);
    run.step_results = vec![fake_result("prep")];
    insert_wf_and_run(&state, &wf, &run).await;
    execute_run(
        state.clone(),
        &wf,
        &mut run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(run.status, RunStatus::Failed, "{:?}", run.step_results);
    assert!(
        run.step_results
            .iter()
            .all(|result| result.step_name != "__preflight__"),
        "a resume has no preflight: {:?}",
        run.step_results
    );
    let refused = run
        .step_results
        .iter()
        .find(|result| result.step_name == "jeu")
        .expect("the step was dispatched");
    assert_eq!(refused.status, RunStatus::Failed);
    assert!(
        refused
            .output
            .starts_with("preflight_failed:'jeu' runs on NVIDIA but no NVIDIA API key is set"),
        "{}",
        refused.output
    );
    assert!(
        !refused.output.contains("nvidia.invalid"),
        "{}",
        refused.output
    );
}

/// A workflow whose only step starts `child`.
fn starting(id: &str, child: &str) -> Workflow {
    let mut start_child = fake_step("lancer");
    start_child.step_type = StepType::SubWorkflow;
    start_child.sub_workflow_id = Some(child.into());
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = id.into();
    wf.steps = vec![start_child];
    wf
}

async fn insert_workflow(state: &crate::AppState, workflow: Workflow) {
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &workflow))
        .await
        .unwrap();
}

#[tokio::test]
async fn http_step_preflight_counts_the_sub_workflow_depth_from_the_runs_own() {
    // KT-2057 — a run that is itself a child at depth 4 may start one more
    // level; at depth 5 its child is refused by its step, so not judged here.
    let (state, _, _) = test_state_and_configs();
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-depth-child".into();
    child.steps = vec![http_step("jeu-enfant", AgentType::Nvidia, None)];
    insert_workflow(&state, child).await;
    let wf = starting("wf-depth", "wf-depth-child");
    for (depth, judged) in [(0, true), (4, true), (5, false), (u64::MAX, false)] {
        let mut run = pending_run(&format!("run-depth-{depth}"), &wf.id);
        run.trigger_context = Some(serde_json::json!({ "__subwf_depth__": depth }));
        let planned = crate::workflows::run_preflight::planned_agent_steps(&state, &wf, &run).await;
        let names: Vec<_> = planned.iter().map(|p| p.step.name.as_str()).collect();
        assert_eq!(
            names.contains(&"jeu-enfant"),
            judged,
            "depth {depth}: {names:?}"
        );
    }
}

#[tokio::test]
async fn http_step_preflight_reads_a_childs_references_in_the_childs_project() {
    // KT-2057 — a child pinned to its own project reads its `ref:prompt:` in
    // that project, not the parent's namesake; a child with no project runs
    // in its parent's and reads it there.
    let (state, _, _) = test_state_and_configs();
    let projects = tempfile::tempdir().unwrap();
    for project in ["proj-ref-parent", "proj-ref-child"] {
        insert_project_at(&state, project, &projects.path().join(project)).await;
    }
    for (connection_id, project) in [
        ("conn-ref-parent", "proj-ref-parent"),
        ("conn-ref-child", "proj-ref-child"),
    ] {
        insert_connection(
            &state,
            connection(
                connection_id,
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            ),
        )
        .await;
        let mut prompt = quick_prompt(&format!("qp-{project}"), Some(connection_id));
        prompt.name = "Shared Prompt".into();
        prompt.project_id = Some(project.into());
        state
            .db
            .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &prompt))
            .await
            .unwrap();
    }
    for (child_id, child_project, expected) in [
        ("wf-ref-pinned", Some("proj-ref-child"), "conn-ref-child"),
        ("wf-ref-inherits", None, "conn-ref-parent"),
    ] {
        let mut child = make_workflow_with_artifacts(Default::default());
        child.id = child_id.into();
        child.project_id = child_project.map(str::to_string);
        child.steps = vec![qp_step("jeu-enfant", "ref:prompt:shared-prompt")];
        insert_workflow(&state, child).await;
        let mut wf = starting(&format!("wf-parent-of-{child_id}"), child_id);
        wf.project_id = Some("proj-ref-parent".into());
        let mut run = pending_run(&format!("run-parent-of-{child_id}"), &wf.id);
        run.project_id = wf.project_id.clone();

        let planned = crate::workflows::run_preflight::planned_agent_steps(&state, &wf, &run).await;
        let step = planned
            .iter()
            .find(|p| p.step.name == "jeu-enfant")
            .expect("the child's step is judged");
        assert_eq!(step.unloaded, None, "{child_id}");
        assert_eq!(
            crate::workflows::steps::step_connection_id(&step.step),
            Some(expected),
            "{child_id}"
        );
    }
}

#[tokio::test]
async fn http_step_preflight_reads_a_childs_references_as_the_run_pinned_them() {
    // KT-2057 — the child run inherits its parent's pins: a `ref:prompt:`
    // repointed after the run was pinned is judged on the pinned prompt.
    let (state, _, _) = test_state_and_configs();
    let projects = tempfile::tempdir().unwrap();
    insert_project_at(&state, "proj-pin", &projects.path().join("proj-pin")).await;
    for connection_id in ["conn-pin-old", "conn-pin-new"] {
        insert_connection(
            &state,
            connection(
                connection_id,
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            ),
        )
        .await;
    }
    let prompt_on = |id: &str, connection_id: &str| {
        let mut prompt = quick_prompt(id, Some(connection_id));
        prompt.name = "Pinned Prompt".into();
        prompt.project_id = Some("proj-pin".into());
        prompt
    };
    let old = prompt_on("qp-pin-old", "conn-pin-old");
    let mut retired = old.clone();
    state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &old))
        .await
        .unwrap();
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-pin-child".into();
    child.steps = vec![qp_step("jeu-enfant", "ref:prompt:pinned-prompt")];
    insert_workflow(&state, child).await;
    let mut wf = starting("wf-pin-parent", "wf-pin-child");
    wf.project_id = Some("proj-pin".into());
    let mut run = pending_run("run-pin-parent", &wf.id);
    run.project_id = wf.project_id.clone();
    insert_wf_and_run(&state, &wf, &run).await;
    let (pinned_wf, pinned_run) = (wf.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::workflows::run_pins::pin_or_load(conn, &pinned_wf, &pinned_run)
        })
        .await
        .unwrap()
        .expect("pinned");

    // The slug moves to a new prompt on another connection.
    retired.name = "Retired Prompt".into();
    let new = prompt_on("qp-pin-new", "conn-pin-new");
    state
        .db
        .with_conn(move |conn| {
            crate::db::quick_prompts::update_quick_prompt(conn, &retired)?;
            crate::db::quick_prompts::insert_quick_prompt(conn, &new)
        })
        .await
        .unwrap();

    let mut live = pending_run("run-pin-live", &wf.id);
    live.project_id = wf.project_id.clone();
    for (run, expected) in [(&run, "conn-pin-old"), (&live, "conn-pin-new")] {
        let planned = crate::workflows::run_preflight::planned_agent_steps(&state, &wf, run).await;
        let step = planned
            .iter()
            .find(|p| p.step.name == "jeu-enfant")
            .expect("the child's step is judged");
        assert_eq!(step.unloaded, None, "{}", run.id);
        assert_eq!(
            crate::workflows::steps::step_connection_id(&step.step),
            Some(expected),
            "{}",
            run.id
        );
    }
}

#[tokio::test]
async fn http_step_preflight_judges_a_child_reached_from_two_projects_on_each_projects_prompt() {
    // KT-2059 — the run pins a multi-project child's references once per
    // project it runs in: reached under A (p1) then B (p2), it is judged on
    // each project's prompt and connection, not twice on the first one's.
    let (state, _, _) = test_state_and_configs();
    let projects = tempfile::tempdir().unwrap();
    for (project, connection_id) in [("proj-two-1", "conn-two-1"), ("proj-two-2", "conn-two-2")] {
        insert_project_at(&state, project, &projects.path().join(project)).await;
        insert_connection(
            &state,
            connection(
                connection_id,
                Some("https://nvidia.invalid/v1"),
                ExternalApiConnectionPreset::Nvidia,
            ),
        )
        .await;
        let mut prompt = quick_prompt(&format!("qp-{project}"), Some(connection_id));
        prompt.name = "Two Projects".into();
        prompt.project_id = Some(project.into());
        state
            .db
            .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &prompt))
            .await
            .unwrap();
    }
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-two-child".into();
    child.name = "enfant".into();
    child.steps = vec![qp_step("jeu-enfant", "ref:prompt:two-projects")];
    insert_workflow(&state, child).await;
    for (parent, project) in [("wf-two-a", "proj-two-1"), ("wf-two-b", "proj-two-2")] {
        let mut wf = starting(parent, "wf-two-child");
        wf.name = parent.into();
        wf.project_id = Some(project.into());
        insert_workflow(&state, wf).await;
    }
    let mut root = starting("wf-two-root", "wf-two-a");
    let mut second = fake_step("second");
    second.step_type = StepType::SubWorkflow;
    second.sub_workflow_id = Some("wf-two-b".into());
    root.steps.push(second);
    let run = pending_run("run-two-root", &root.id);
    insert_wf_and_run(&state, &root, &run).await;
    let (pinned_wf, pinned_run) = (root.clone(), run.clone());
    state
        .db
        .with_conn(move |conn| {
            crate::workflows::run_pins::pin_or_load(conn, &pinned_wf, &pinned_run)
        })
        .await
        .unwrap()
        .expect("pinned");

    let planned = crate::workflows::run_preflight::planned_agent_steps(&state, &root, &run).await;
    let mut judged: Vec<(String, Option<String>)> = planned
        .iter()
        .filter(|p| p.step.name == "jeu-enfant")
        .map(|p| {
            assert_eq!(p.unloaded, None, "{:?}", p.within);
            (
                p.within.clone().unwrap_or_default(),
                crate::workflows::steps::step_connection_id(&p.step).map(str::to_string),
            )
        })
        .collect();
    judged.sort();
    assert_eq!(judged.len(), 2, "{judged:?}");
    assert!(judged[0].0.contains("`wf-two-a`"), "{judged:?}");
    assert_eq!(judged[0].1.as_deref(), Some("conn-two-1"), "{judged:?}");
    assert!(judged[1].0.contains("`wf-two-b`"), "{judged:?}");
    assert_eq!(judged[1].1.as_deref(), Some("conn-two-2"), "{judged:?}");
}

#[tokio::test]
async fn http_step_preflight_a_childs_step_whose_quick_prompt_cannot_load_is_refused_before_the_parents_first_step(
) {
    // KT-2057 — read with this run's pins, which the child run inherits: it
    // would fail mid-run, after the parent's first steps.
    let mut start_child = fake_step("lancer");
    start_child.step_type = StepType::SubWorkflow;
    start_child.sub_workflow_id = Some("wf-enfant-qp-gone".into());
    let mut child = make_workflow_with_artifacts(Default::default());
    child.id = "wf-enfant-qp-gone".into();
    child.name = "enfant".into();
    child.steps = vec![qp_step("jeu-enfant", "qp-child-gone")];
    let run = run_after_gate_with(
        "child-qp-gone",
        vec![start_child],
        Setup {
            connections: Vec::new(),
            keys: vec![NVIDIA_CREDENTIAL_SLUG.into()],
        },
        Extras {
            children: vec![child],
            ..Extras::default()
        },
    )
    .await;
    let refusal = refused_before_any_step(&run, "jeu-enfant");
    assert!(
        refusal.contains("sub-workflow `enfant` (step 'lancer')")
            && refusal.contains("qp-child-gone"),
        "{refusal}"
    );
}
