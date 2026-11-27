//! KT-1998 — a model that starts with `-` or holds a control character never
//! reaches an agent command line. A plain `#[cfg(test)]` module, so the
//! `model_id_guard` filter runs it with or without the `full-tests` feature.

use super::*;
use crate::acp::{
    AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const CLI_FAMILIES: [AgentType; 7] = [
    AgentType::ClaudeCode,
    AgentType::Codex,
    AgentType::OpenCode,
    AgentType::GeminiCli,
    AgentType::Kiro,
    AgentType::Vibe,
    AgentType::CopilotCli,
];

fn tier_slot<'a>(
    tiers: &'a mut ModelTiersConfig,
    agent: &AgentType,
) -> &'a mut crate::models::setup::ModelTierConfig {
    match agent {
        AgentType::ClaudeCode => &mut tiers.claude_code,
        AgentType::Codex => &mut tiers.codex,
        AgentType::OpenCode => &mut tiers.open_code,
        AgentType::GeminiCli => &mut tiers.gemini_cli,
        AgentType::Kiro => &mut tiers.kiro,
        AgentType::Vibe => &mut tiers.vibe,
        AgentType::CopilotCli => &mut tiers.copilot_cli,
        other => panic!("{other:?} is not a CLI family"),
    }
}

/// The argv a family's builder gives for `model`: the token after
/// `--model`, or `None` when the family takes its model through the ACP
/// session instead of argv.
fn model_after_flag(agent: &AgentType, model: &str) -> Option<String> {
    let (_, _, args, _, _, _) = agent_command(agent, "prompt", false, "", Some(model));
    let at = args.iter().position(|a| a == "--model")?;
    Some(args[at + 1].clone())
}

/// One family: a pinned and a tier model starting with `-` are refused with
/// an error naming the value and its source; a plain id passes and, when the
/// family's builder emits `--model`, comes right after it.
fn model_id_guard_family(agent: AgentType, emits_model_flag: bool) {
    let pinned = launch_model_flag(Some("--help"), &agent, ModelTier::Default, None)
        .expect_err("a pinned dash-led model is refused");
    assert!(pinned.contains("\"--help\""), "{agent:?}: {pinned}");
    assert!(pinned.contains("pinned on this run"), "{agent:?}: {pinned}");

    let mut tiers = ModelTiersConfig::default();
    tier_slot(&mut tiers, &agent).economy = Some(" -c".into());
    let tier = launch_model_flag(None, &agent, ModelTier::Economy, Some(&tiers))
        .expect_err("a dash-led tier model is refused");
    assert!(tier.contains("economy tier model"), "{agent:?}: {tier}");
    assert!(tier.contains("in Settings → Agents"), "{agent:?}: {tier}");
    assert!(tier.contains("starts with '-'"), "{agent:?}: {tier}");

    let model = launch_model_flag(Some("model-x"), &agent, ModelTier::Default, None)
        .expect("a plain id passes");
    assert_eq!(model.as_deref(), Some("model-x"));
    let after = model_after_flag(&agent, "model-x");
    if emits_model_flag {
        assert_eq!(after.as_deref(), Some("model-x"), "{agent:?}");
    } else {
        assert_eq!(
            after, None,
            "{agent:?}: the model goes through the ACP session"
        );
    }
}

#[test]
fn model_id_guard_runner_claude_code() {
    model_id_guard_family(AgentType::ClaudeCode, true);
}

#[test]
fn model_id_guard_runner_codex() {
    model_id_guard_family(AgentType::Codex, true);
}

#[test]
fn model_id_guard_runner_open_code() {
    model_id_guard_family(AgentType::OpenCode, false);
}

#[test]
fn model_id_guard_runner_gemini_cli() {
    model_id_guard_family(AgentType::GeminiCli, true);
}

#[test]
fn model_id_guard_runner_kiro() {
    model_id_guard_family(AgentType::Kiro, false);
}

#[test]
fn model_id_guard_runner_vibe() {
    model_id_guard_family(AgentType::Vibe, true);
}

#[test]
fn model_id_guard_runner_copilot_cli() {
    model_id_guard_family(AgentType::CopilotCli, true);
}

/// A control character is refused at launch too (a row saved before the rule,
/// a hand-edited config), with the value shown escaped; ids with brackets,
/// a namespace or an inner space pass unchanged. No length rule at launch.
#[test]
fn model_id_guard_runner_refuses_a_control_character() {
    let agent = AgentType::ClaudeCode;
    for bad in ["sonnet\u{0}--flag", "gpt-5\n--flag"] {
        let error = launch_model_flag(Some(bad), &agent, ModelTier::Default, None)
            .expect_err("a control character is refused");
        assert!(error.contains(&format!("{bad:?}")), "{error}");
        assert!(error.contains("control character"), "{error}");
        assert!(error.contains("pinned on this run"), "{error}");
    }
    let long = "m".repeat(crate::models::setup::MAX_MODEL_ID_CHARS * 4);
    for good in [
        "opus[1m]",
        "vendor/model:tag@v1",
        "model with space",
        long.as_str(),
    ] {
        let model = launch_model_flag(Some(good), &agent, ModelTier::Default, None)
            .expect("a model id passes");
        assert_eq!(model.as_deref(), Some(good));
        assert_eq!(model_after_flag(&agent, good).as_deref(), Some(good));
    }
}

/// The three sources a refusal can name: the run's pin, a tier slot of the
/// settings, and the catalogue's tier assignment.
#[test]
fn model_id_guard_runner_names_each_source() {
    let agent = AgentType::GeminiCli;
    assert_eq!(
        LaunchModelSource::of(Some("x"), &agent, ModelTier::Default, None),
        LaunchModelSource::Pinned
    );
    let mut tiers = ModelTiersConfig::default();
    tier_slot(&mut tiers, &agent).reasoning = Some("pro".into());
    assert_eq!(
        LaunchModelSource::of(None, &agent, ModelTier::Reasoning, Some(&tiers)),
        LaunchModelSource::Configured {
            agent: agent.clone(),
            tier: ModelTier::Reasoning
        }
    );
    let source = LaunchModelSource::of(None, &agent, ModelTier::Economy, Some(&tiers));
    assert_eq!(
        source,
        LaunchModelSource::Catalogue {
            agent: agent.clone(),
            tier: ModelTier::Economy
        }
    );
    let error = check_launch_model(Some("-x".into()), || source)
        .expect_err("a dash-led catalogue model is refused");
    assert!(
        error.contains("the economy tier model the catalogue assigns to GeminiCli"),
        "{error}"
    );
    assert!(error.contains("starts with '-'"), "{error}");
    assert_eq!(
        check_launch_model(None, || unreachable!("no model, no refusal")).unwrap(),
        None
    );
}

/// A transport that counts what reaches it: nothing should.
struct CountingTransport {
    created: AtomicUsize,
    prompts: AtomicUsize,
}

#[async_trait::async_trait]
impl AcpTransport for CountingTransport {
    async fn initialize(&self, _: AcpInitialize) -> Result<AcpNegotiatedCapabilities, AcpError> {
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: [AcpCapability::Sessions, AcpCapability::Streaming]
                .into_iter()
                .collect(),
        })
    }

    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        self.created.fetch_add(1, Ordering::SeqCst);
        AcpSessionTarget::new(crate::acp::AcpAgent::OpenCode, "guard-session")
    }

    async fn config_options(&self) -> Vec<AcpConfigOption> {
        Vec::new()
    }

    async fn set_config_option(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }

    async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }

    async fn prompt(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        events: tokio::sync::mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        let _ = events.send(AcpSessionEvent::Completed).await;
        Ok(())
    }

    async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), AcpError> {
        Ok(())
    }
}

fn no_tokens() -> crate::models::setup::TokensConfig {
    crate::models::setup::TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: Vec::new(),
        disabled_overrides: Vec::new(),
    }
}

/// The guard sits where every CLI route takes its model: a launch with a
/// dash-led model is refused before any transport is touched, on every
/// family, whatever route (native ACP, adapted ACP, direct CLI) it would take.
#[tokio::test]
async fn model_id_guard_start_agent_refuses_before_any_transport() {
    let project = tempfile::tempdir().unwrap();
    let tokens = no_tokens();
    for agent in CLI_FAMILIES {
        let _saved = crate::core::config::test_saved_access::set(&agent, true);
        let fixture = Arc::new(CountingTransport {
            created: AtomicUsize::new(0),
            prompts: AtomicUsize::new(0),
        });
        let result = start_agent_with_config(AgentStartConfig {
            full_access: true,
            model_override: Some("--help"),
            test_acp_transport: Some(fixture.clone()),
            ..AgentStartConfig::new(&agent, project.path().to_str().unwrap(), "hello", &tokens)
        })
        .await;
        let error = result.err().expect("a dash-led model is refused");
        assert!(error.contains("starts with '-'"), "{agent:?}: {error}");
        assert_eq!(fixture.created.load(Ordering::SeqCst), 0, "{agent:?}");
        assert_eq!(fixture.prompts.load(Ordering::SeqCst), 0, "{agent:?}");
    }
}

/// Without any test transport (the production wiring), the launch error is
/// the guard's, not a missing binary's: nothing before the guard looks up or
/// spawns the CLI.
#[tokio::test]
async fn model_id_guard_start_agent_without_a_test_transport_refuses_at_the_guard() {
    let project = tempfile::tempdir().unwrap();
    let tokens = no_tokens();
    for agent in CLI_FAMILIES {
        let _saved = crate::core::config::test_saved_access::set(&agent, true);
        for bad in ["--help", "gpt-5\n--flag"] {
            let result = start_agent_with_config(AgentStartConfig {
                full_access: true,
                model_override: Some(bad),
                test_acp_transport: None,
                ..AgentStartConfig::new(&agent, project.path().to_str().unwrap(), "hello", &tokens)
            })
            .await;
            let error = result.err().expect("refused");
            assert!(
                error.starts_with("Agent launch refused: the model "),
                "{agent:?} {bad:?}: {error}"
            );
        }
    }
}
