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

/// A run of `porte` then an NVIDIA step, in a worktree whose `before_run`
/// hook leaves `marker`.
async fn run_with_before_run_hook(
    id: &str,
    nvidia_key: bool,
    marker: &std::path::Path,
) -> WorkflowRun {
    let (state, mut tokens, agents) = test_state_and_configs();
    let project = format!("proj-{id}");
    let _repo = git_project(&state, &project).await;
    if nvidia_key {
        tokens.keys.push(api_key(NVIDIA_CREDENTIAL_SLUG));
    }
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = format!("wf-{id}");
    wf.project_id = Some(project.clone());
    wf.workspace_config = Some(WorkspaceConfig {
        hooks: hooks(None, Some(&format!("touch '{}'", marker.display()))),
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
    run
}

#[tokio::test]
async fn http_step_preflight_a_refused_run_never_runs_the_before_run_hook() {
    let markers = tempfile::tempdir().unwrap();
    let refused_marker = markers.path().join("refused-before-run");
    let run = run_with_before_run_hook("hook-refused", false, &refused_marker).await;
    refused_before_any_step(&run, "jeu");
    assert!(
        !refused_marker.exists(),
        "the before_run hook ran for a refused run"
    );

    // The same run with its key starts, and its hook runs.
    let started_marker = markers.path().join("started-before-run");
    let run = run_with_before_run_hook("hook-started", true, &started_marker).await;
    passed_preflight(&run);
    assert!(
        started_marker.exists(),
        "the hook runs once the preflight passed"
    );
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
