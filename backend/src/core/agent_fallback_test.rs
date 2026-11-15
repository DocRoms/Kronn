use super::*;
use crate::models::{AgentFallback, FallbackGap, FallbackMode, FallbackTransport};

fn agents(chains: &[(AgentType, Vec<AgentFallback>)]) -> AgentsConfig {
    let mut agents = crate::core::config::default_config().agents;
    for (from, chain) in chains {
        agents.agent_config_mut(from).unwrap().fallback_chain = chain.clone();
    }
    // The way a 0.15.3 chain is read now: carried onto the rows of the agents.
    migrate_legacy_chains(&mut agents);
    agents
}

/// The first link `failing` is offered when the turn started on `origin`, with
/// the origin's default chain.
fn pick(
    config: &AgentsConfig,
    origin: &AgentType,
    failing: &AgentType,
    usable: &[AgentType],
    needs: &TurnNeeds,
    cooling: &dyn Fn(&AgentType) -> Option<DateTime<Utc>>,
) -> Selection {
    select(
        config,
        &default_chain(config, origin, FallbackMode::Standard),
        origin,
        failing,
        usable,
        needs,
        cooling,
    )
}

fn link(agent: AgentType) -> AgentFallback {
    AgentFallback {
        agent,
        model: None,
        mode: None,
    }
}

fn everyone() -> Vec<AgentType> {
    all_agents().to_vec()
}

fn never_cooling(_: &AgentType) -> Option<DateTime<Utc>> {
    None
}

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    crate::db::migrations::run(&connection).unwrap();
    connection
}

fn at(hour: u32) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&format!("2026-11-10T{hour:02}:00:00Z"))
        .unwrap()
        .with_timezone(&Utc)
}

// ─── Triggers ────────────────────────────────────────────────────────────

#[test]
fn every_provider_side_failure_is_a_trigger() {
    let cases = [
        (
            "ERROR: You've hit your usage limit. Upgrade to Pro or try again in 2 hours.",
            Trigger::QuotaExhausted,
        ),
        (
            "Your credit balance is too low to access the Anthropic API",
            Trigger::QuotaExhausted,
        ),
        (
            "HTTP 429 Too Many Requests: rate_limit_exceeded",
            Trigger::RateLimit,
        ),
        (
            "[Agent provider error] HTTP 503 Service Unavailable",
            Trigger::Outage,
        ),
        ("API Error: 500 Internal Server Error", Trigger::Outage),
        (
            "Invalid authentication credentials (api error: 401)",
            Trigger::NotAuthenticated,
        ),
        (
            "Not logged in · Please run /login",
            Trigger::NotAuthenticated,
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(classify_trigger(text), Some(expected), "{text}");
    }
}

#[test]
fn a_task_failure_a_refusal_a_test_failure_or_a_work_timeout_is_never_a_trigger() {
    let not_the_providers = [
        "error[E0308]: mismatched types — the build failed",
        "test result: FAILED. 3 passed; 2 failed; 0 ignored",
        "AssertionError: expected 4 to equal 5",
        "I can't help with that request.",
        "The task could not be completed: the repository has no Cargo.toml",
        "Agent stalled (no output for 300s)",
        // The work's own timeout stays the work's, even when its last output
        // happens to quote a rate limit the agent was reading about.
        "Agent stalled (no output for 300s) — last line: HTTP 429 Too Many Requests",
        "workflow global timeout reached while the step was running",
        "",
    ];
    for text in not_the_providers {
        assert_eq!(classify_trigger(text), None, "{text}");
    }
}

#[test]
fn the_reset_the_provider_announces_is_read() {
    let now = at(9);
    let reset = announced_reset("You've hit your limit · resets 4pm (Europe/Paris)", now);
    assert_eq!(reset, Some(at(15)), "4pm in Paris in November is 15:00 UTC");
    assert_eq!(announced_reset("HTTP 429 Too Many Requests", now), None);
}

// ─── The chain ───────────────────────────────────────────────────────────

fn agents_of(chain: &[AgentFallback]) -> Vec<AgentType> {
    chain.iter().map(|link| link.agent.clone()).collect()
}

#[test]
fn the_chain_is_the_ordered_list_of_the_agent_the_turn_starts_on() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![link(AgentType::Codex), link(AgentType::Ollama)],
    )]);
    assert_eq!(
        agents_of(&default_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Standard
        )),
        vec![AgentType::Codex, AgentType::Ollama]
    );
    assert_eq!(
        agents_of(&default_chain(
            &config,
            &AgentType::Codex,
            FallbackMode::Eco
        )),
        vec![AgentType::Ollama],
        "the fallback of a fallback is the one set on ITS own row"
    );
    assert!(default_chain(&config, &AgentType::Ollama, FallbackMode::Eco).is_empty());
    validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
        Some(FallbackMode::Standard),
        &default_chain(&config, &AgentType::ClaudeCode, FallbackMode::Standard),
    )
    .unwrap();
}

#[test]
fn an_override_replaces_the_default_chain_and_an_empty_one_cuts_it() {
    let config = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::Codex)])]);
    let own = [link(AgentType::GeminiCli)];
    assert_eq!(
        agents_of(&effective_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Standard,
            Some(&own)
        )),
        vec![AgentType::GeminiCli]
    );
    assert!(effective_chain(
        &config,
        &AgentType::ClaudeCode,
        FallbackMode::Standard,
        Some(&[])
    )
    .is_empty());
    assert_eq!(
        agents_of(&effective_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Standard,
            None
        )),
        vec![AgentType::Codex]
    );
}

#[test]
fn the_walk_goes_on_in_the_same_list_after_a_hop() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![link(AgentType::Codex), link(AgentType::Ollama)],
    )]);
    let needs = TurnNeeds::default();
    // The origin fails: the first link.
    assert_eq!(
        pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &everyone(),
            &needs,
            &never_cooling
        ),
        Selection::Switch(link(AgentType::Codex))
    );
    // Codex fails in its turn: the walk goes on to Ollama, not back to Claude Code
    // and not into Codex's own chain.
    assert_eq!(
        pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::Codex,
            &everyone(),
            &needs,
            &never_cooling
        ),
        Selection::Switch(link(AgentType::Ollama))
    );
    // The last link fails: nothing left, said clearly.
    match pick(
        &config,
        &AgentType::ClaudeCode,
        &AgentType::Ollama,
        &everyone(),
        &needs,
        &never_cooling,
    ) {
        Selection::Stop { why } => assert!(why.contains("no link left"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_loop_is_refused_the_starting_agent_or_one_agent_twice() {
    let config = agents(&[]);
    let error = validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
        None,
        &[link(AgentType::Codex), link(AgentType::Codex)],
    )
    .expect_err("Codex twice");
    assert!(matches!(error, ChainError::Cycle(_)), "{error}");
    assert!(error.to_string().contains("Claude Code → Codex → Codex"));
    assert_eq!(
        validate_chain(
            &config,
            Some(&AgentType::Codex),
            None,
            &[link(AgentType::GeminiCli), link(AgentType::Codex)]
        ),
        Err(ChainError::SelfFallback(AgentType::Codex)),
        "a chain that comes back to its own agent loops"
    );
    // A loop written by hand across two rows is not walked: the chain stops
    // where it would come back to an agent already on it.
    let mut hand = agents(&[]);
    set_row(
        &mut hand,
        &AgentType::ClaudeCode,
        FallbackMode::Standard,
        Some(link(AgentType::Codex)),
    );
    set_row(
        &mut hand,
        &AgentType::Codex,
        FallbackMode::Standard,
        Some(link(AgentType::ClaudeCode)),
    );
    assert_eq!(
        agents_of(&default_chain(
            &hand,
            &AgentType::ClaudeCode,
            FallbackMode::Standard
        )),
        vec![AgentType::Codex]
    );
    // And one that would be written through the API is refused with the path.
    let error = validate_row(
        &hand,
        &AgentType::Codex,
        FallbackMode::Standard,
        &link(AgentType::ClaudeCode),
    )
    .expect_err("Claude Code → Codex → Claude Code");
    assert!(matches!(
        error,
        ChainError::SelfFallback(_) | ChainError::Cycle(_)
    ));
    let mut rows = agents(&[]);
    set_row(
        &mut rows,
        &AgentType::Codex,
        FallbackMode::Standard,
        Some(link(AgentType::GeminiCli)),
    );
    set_row(
        &mut rows,
        &AgentType::GeminiCli,
        FallbackMode::Standard,
        Some(link(AgentType::ClaudeCode)),
    );
    let error = validate_row(
        &rows,
        &AgentType::ClaudeCode,
        FallbackMode::Standard,
        &link(AgentType::Codex),
    )
    .expect_err("the loop passes through two other rows");
    assert!(matches!(error, ChainError::Cycle(_)), "{error}");
    assert!(error
        .to_string()
        .contains("Claude Code → Codex → Gemini CLI → Claude Code"));
}

#[test]
fn a_chain_past_the_depth_limit_is_refused() {
    let config = agents(&[]);
    let four = [
        link(AgentType::Codex),
        link(AgentType::GeminiCli),
        link(AgentType::Kiro),
        link(AgentType::Vibe),
    ];
    assert_eq!(four.len(), MAX_CHAIN_DEPTH);
    validate_chain(&config, Some(&AgentType::ClaudeCode), None, &four).unwrap();
    let five: Vec<AgentFallback> = four
        .iter()
        .cloned()
        .chain([link(AgentType::CopilotCli)])
        .collect();
    let error = validate_chain(&config, Some(&AgentType::ClaudeCode), None, &five)
        .expect_err("a fifth link is over the limit");
    assert!(matches!(error, ChainError::TooDeep { .. }), "{error}");
    assert!(error.to_string().contains("at most 4"));
}

#[test]
fn a_fallback_never_gets_more_access_than_the_agent_it_replaces() {
    let mut config = agents(&[]);
    config.codex.full_access = true;
    let error = validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
        None,
        &[link(AgentType::Codex)],
    )
    .unwrap_err();
    assert!(matches!(error, ChainError::BroaderAccess { .. }), "{error}");
    assert!(error.to_string().contains("more access"));
    config.claude_code.full_access = true;
    validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
        None,
        &[link(AgentType::Codex)],
    )
    .unwrap();
}

#[test]
fn a_named_connection_cannot_take_part_and_a_blank_model_is_refused() {
    let config = agents(&[]);
    assert_eq!(
        validate_chain(
            &config,
            Some(&AgentType::ClaudeCode),
            None,
            &[link(AgentType::Custom)]
        ),
        Err(ChainError::NoCardFor(AgentType::Custom))
    );
    let blank = AgentFallback {
        model: Some("   ".into()),
        ..link(AgentType::Codex)
    };
    assert!(matches!(
        validate_chain(&config, Some(&AgentType::ClaudeCode), None, &[blank]),
        Err(ChainError::BadModel(_))
    ));
    let named = AgentFallback {
        model: Some("sonnet".into()),
        ..link(AgentType::Codex)
    };
    validate_chain(&config, Some(&AgentType::ClaudeCode), None, &[named]).unwrap();
}

#[test]
fn the_chain_view_shows_the_gaps_of_each_link_as_warnings() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![
            link(AgentType::Codex),
            link(AgentType::LiteLlm),
            link(AgentType::Ollama),
        ],
    )]);
    let view = chain_view(&config, &AgentType::ClaudeCode, FallbackMode::Standard);
    assert_eq!(view.error, None);
    assert_eq!(view.links.len(), 1, "one fallback is set on the row itself");
    assert!(view.links[0].gaps.is_empty(), "a CLI agent has everything");
    // The next ones are inherited: set on the rows of the fallbacks.
    assert_eq!(view.inherited.len(), 2);
    assert_eq!(view.inherited[0].source, AgentType::Codex);
    assert_eq!(view.inherited[0].link.agent, AgentType::LiteLlm);
    assert!(view.inherited[0]
        .link
        .gaps
        .contains(&FallbackGap::NoMcpTools));
    assert_eq!(view.inherited[1].source, AgentType::LiteLlm);
    assert!(view.inherited[1]
        .link
        .gaps
        .contains(&FallbackGap::CappedContext));
    let own = chain_view_of(
        &config,
        None,
        FallbackMode::Standard,
        &[link(AgentType::Ollama)],
    );
    assert!(own.links[0].gaps.contains(&FallbackGap::NoMcpTools));
}

#[test]
fn every_link_carries_its_transport() {
    assert_eq!(transport(&AgentType::ClaudeCode), FallbackTransport::Cli);
    assert_eq!(transport(&AgentType::OpenCode), FallbackTransport::Cli);
    assert_eq!(transport(&AgentType::LiteLlm), FallbackTransport::Http);
    assert_eq!(transport(&AgentType::Nvidia), FallbackTransport::Http);
    assert_eq!(transport(&AgentType::Ollama), FallbackTransport::Local);
    let config = agents(&[(AgentType::OpenCode, vec![link(AgentType::Ollama)])]);
    let view = chain_view(&config, &AgentType::OpenCode, FallbackMode::Eco);
    assert_eq!(view.links[0].transport, FallbackTransport::Local);
}

#[test]
fn a_fallback_goes_to_the_same_kind_of_mode_only() {
    let config = agents(&[]);
    let to = |mode: FallbackMode| AgentFallback {
        mode: Some(mode),
        ..link(AgentType::Codex)
    };
    // Text goes to any text mode.
    for from in FallbackMode::TEXT {
        for target in FallbackMode::TEXT {
            validate_row(&config, &AgentType::ClaudeCode, from, &to(target)).unwrap();
        }
    }
    // The other kinds stay with their own.
    for mode in [
        FallbackMode::Image,
        FallbackMode::Video,
        FallbackMode::Decision,
    ] {
        validate_row(&config, &AgentType::ClaudeCode, mode, &to(mode)).unwrap();
    }
    for from in FallbackMode::ALL {
        for target in FallbackMode::ALL {
            if from.kind() == target.kind() {
                continue;
            }
            let error = validate_row(&config, &AgentType::ClaudeCode, from, &to(target))
                .expect_err("incompatible kinds are never accepted");
            assert!(
                matches!(error, ChainError::WrongKind { .. }),
                "{from:?} → {target:?}: {error}"
            );
        }
    }
    assert_eq!(
        validate_row(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Image,
            &link(AgentType::ClaudeCode)
        ),
        Err(ChainError::SelfFallback(AgentType::ClaudeCode))
    );
}

#[test]
fn rows_are_per_mode_and_every_agent_opencode_included_has_them() {
    let mut config = agents(&[]);
    assert!(!is_configured(&config));
    // Every agent can have a row on every mode, and OpenCode is a target like
    // the others (each row is checked on its own, from an empty configuration).
    for agent in all_agents() {
        for mode in FallbackMode::ALL {
            let target = if agent == AgentType::OpenCode {
                AgentType::Codex
            } else {
                AgentType::OpenCode
            };
            let fallback = AgentFallback {
                mode: Some(mode),
                ..link(target)
            };
            validate_row(&agents(&[]), &agent, mode, &fallback)
                .unwrap_or_else(|error| panic!("{agent:?} {mode:?}: {error}"));
        }
    }
    // OpenCode has its own rows, one per mode, and Codex falls back to it.
    for mode in FallbackMode::ALL {
        let fallback = AgentFallback {
            mode: Some(mode),
            ..link(AgentType::Codex)
        };
        assert!(set_row(
            &mut config,
            &AgentType::OpenCode,
            mode,
            Some(fallback)
        ));
    }
    assert!(is_configured(&config));
    assert_eq!(
        config.open_code.mode_fallbacks.len(),
        FallbackMode::ALL.len()
    );
    assert_eq!(
        row(&config, &AgentType::OpenCode, FallbackMode::Video).map(|link| link.agent.clone()),
        Some(AgentType::Codex)
    );
    // A row is changed alone, and removed alone.
    assert!(set_row(
        &mut config,
        &AgentType::OpenCode,
        FallbackMode::Eco,
        None
    ));
    assert!(row(&config, &AgentType::OpenCode, FallbackMode::Eco).is_none());
    assert!(row(&config, &AgentType::OpenCode, FallbackMode::Standard).is_some());
    assert!(!set_row(
        &mut config,
        &AgentType::Custom,
        FallbackMode::Eco,
        None
    ));
}

#[test]
fn the_fallback_of_the_fallback_is_the_one_on_its_own_row_and_depth_is_bounded() {
    let mut config = agents(&[]);
    // Claude Code standard → Codex; Codex standard → Kiro, but Codex eco → Vibe.
    set_row(
        &mut config,
        &AgentType::ClaudeCode,
        FallbackMode::Standard,
        Some(link(AgentType::Codex)),
    );
    set_row(
        &mut config,
        &AgentType::Codex,
        FallbackMode::Standard,
        Some(link(AgentType::Kiro)),
    );
    set_row(
        &mut config,
        &AgentType::Codex,
        FallbackMode::Eco,
        Some(link(AgentType::Vibe)),
    );
    assert_eq!(
        agents_of(&default_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Standard
        )),
        vec![AgentType::Codex, AgentType::Kiro]
    );
    // A link running in another mode chains through THAT mode's row of the target.
    let advanced_to_eco = AgentFallback {
        mode: Some(FallbackMode::Eco),
        ..link(AgentType::Codex)
    };
    set_row(
        &mut config,
        &AgentType::ClaudeCode,
        FallbackMode::Advanced,
        Some(advanced_to_eco),
    );
    assert_eq!(
        agents_of(&default_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Advanced
        )),
        vec![AgentType::Codex, AgentType::Vibe]
    );
    // Depth: a straight line of rows is cut at MAX_CHAIN_DEPTH links, and a row
    // that would make it longer is refused when saved.
    let line = [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::Vibe,
        AgentType::CopilotCli,
    ];
    let mut deep = agents(&[]);
    for pair in line.windows(2) {
        set_row(
            &mut deep,
            &pair[0],
            FallbackMode::Eco,
            Some(link(pair[1].clone())),
        );
    }
    assert_eq!(
        default_chain(&deep, &AgentType::ClaudeCode, FallbackMode::Eco).len(),
        MAX_CHAIN_DEPTH
    );
    let mut shallow = agents(&[]);
    for pair in line[..5].windows(2) {
        set_row(
            &mut shallow,
            &pair[0],
            FallbackMode::Eco,
            Some(link(pair[1].clone())),
        );
    }
    let error = validate_row(
        &shallow,
        &AgentType::OpenCode,
        FallbackMode::Eco,
        &link(AgentType::ClaudeCode),
    )
    .expect_err("OpenCode → Claude Code → … is five links");
    assert!(matches!(error, ChainError::TooDeep { .. }), "{error}");
}

#[test]
fn a_0_15_3_agent_chain_is_carried_onto_the_text_rows() {
    let mut config = crate::core::config::default_config().agents;
    config.claude_code.fallback_chain = vec![
        AgentFallback {
            model: Some("gpt-5.6".into()),
            ..link(AgentType::Codex)
        },
        link(AgentType::Ollama),
    ];
    config.gemini_cli.fallback_chain = vec![link(AgentType::OpenCode)];
    assert!(migrate_legacy_chains(&mut config));
    assert!(config.claude_code.fallback_chain.is_empty());
    for mode in FallbackMode::TEXT {
        assert_eq!(
            row(&config, &AgentType::ClaudeCode, mode)
                .map(|link| (link.agent.clone(), link.model.clone())),
            Some((AgentType::Codex, Some("gpt-5.6".to_string())))
        );
        assert_eq!(
            row(&config, &AgentType::Codex, mode).map(|link| link.agent.clone()),
            Some(AgentType::Ollama)
        );
        assert_eq!(
            row(&config, &AgentType::GeminiCli, mode).map(|link| link.agent.clone()),
            Some(AgentType::OpenCode)
        );
    }
    // Decision, image and video were never part of it.
    for mode in [
        FallbackMode::Decision,
        FallbackMode::Image,
        FallbackMode::Video,
    ] {
        assert!(row(&config, &AgentType::ClaudeCode, mode).is_none());
    }
    // The walk is the one the owner had: Claude Code → Codex → Ollama.
    assert_eq!(
        agents_of(&default_chain(
            &config,
            &AgentType::ClaudeCode,
            FallbackMode::Advanced
        )),
        vec![AgentType::Codex, AgentType::Ollama]
    );
    assert!(
        !migrate_legacy_chains(&mut config),
        "nothing left to carry: a second start changes nothing"
    );
}

// ─── Choosing the next link ──────────────────────────────────────────────

#[test]
fn the_first_usable_link_is_chosen() {
    let config = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::Codex)])]);
    let selection = pick(
        &config,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &everyone(),
        &TurnNeeds::default(),
        &never_cooling,
    );
    assert_eq!(selection, Selection::Switch(link(AgentType::Codex)));
}

#[test]
fn without_a_configured_fallback_nothing_is_chosen_and_the_reason_is_clear() {
    let selection = pick(
        &agents(&[]),
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &everyone(),
        &TurnNeeds::default(),
        &never_cooling,
    );
    match selection {
        Selection::Stop { why } => assert!(why.contains("no fallback configured"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_link_on_cooldown_or_not_installed_is_skipped_for_the_next_one() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![link(AgentType::Codex), link(AgentType::GeminiCli)],
    )]);
    let cooling = |agent: &AgentType| (*agent == AgentType::Codex).then(|| at(14));
    assert_eq!(
        pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &everyone(),
            &TurnNeeds::default(),
            &cooling
        ),
        Selection::Switch(link(AgentType::GeminiCli))
    );
    let only_codex = vec![AgentType::ClaudeCode, AgentType::Codex];
    match pick(
        &config,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &only_codex,
        &TurnNeeds::default(),
        &cooling,
    ) {
        Selection::Stop { why } => {
            assert!(
                why.contains("Codex is itself unavailable until 14:00 UTC"),
                "{why}"
            );
            assert!(
                why.contains("Gemini CLI is not installed or not enabled"),
                "{why}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fallback_that_bills_by_api_key_needs_no_opt_in() {
    let config = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::LiteLlm)])]);
    assert_eq!(
        pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &everyone(),
            &TurnNeeds::default(),
            &never_cooling
        ),
        Selection::Switch(link(AgentType::LiteLlm)),
        "enabling the fallback is the consent"
    );
}

#[test]
fn a_link_that_lacks_what_the_turn_needs_is_skipped() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![link(AgentType::LiteLlm), link(AgentType::Codex)],
    )]);
    let needs_mcp = TurnNeeds {
        mcp_tools: true,
        ..Default::default()
    };
    assert_eq!(
        pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &everyone(),
            &needs_mcp,
            &never_cooling
        ),
        Selection::Switch(link(AgentType::Codex)),
        "LiteLLM has no MCP tools: the walk goes on to Codex"
    );
    let ollama_only = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::Ollama)])]);
    let long = TurnNeeds {
        long_context: true,
        ..Default::default()
    };
    match pick(
        &ollama_only,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &everyone(),
        &long,
        &never_cooling,
    ) {
        Selection::Stop { why } => assert!(why.contains("context window"), "{why}"),
        other => panic!("{other:?}"),
    }
    let images = TurnNeeds {
        image_input: true,
        ..Default::default()
    };
    assert_eq!(
        capability_gap(&AgentType::Ollama, &images),
        Some("it cannot be counted on to read images")
    );
    assert_eq!(capability_gap(&AgentType::Codex, &images), None);
}

#[test]
fn a_fallback_with_more_access_than_the_failing_agent_is_skipped_at_run_time() {
    // Written by hand in config.toml, past the save-time check.
    let mut config = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::Codex)])]);
    config.codex.full_access = true;
    match pick(
        &config,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &everyone(),
        &TurnNeeds::default(),
        &never_cooling,
    ) {
        Selection::Stop { why } => assert!(why.contains("more access"), "{why}"),
        other => panic!("{other:?}"),
    }
}

// ─── What the room says ──────────────────────────────────────────────────

#[test]
fn the_room_line_names_the_agent_the_reason_the_time_and_the_continuation() {
    let explicit = AgentFallback {
        model: Some("sonnet".into()),
        ..link(AgentType::ClaudeCode)
    };
    assert_eq!(
        notice(
            &AgentType::Codex,
            Some("gpt-5.6-sol"),
            Trigger::QuotaExhausted,
            Some(at(14)),
            &explicit,
            ModelTier::Default
        ),
        "Codex · gpt-5.6-sol: quota exhausted until 14:00 UTC → continued with Claude Code · sonnet [CLI]"
    );
    assert_eq!(
        notice(
            &AgentType::Codex,
            None,
            Trigger::RateLimit,
            None,
            &link(AgentType::ClaudeCode),
            ModelTier::Economy
        ),
        "Codex · eco mode: rate limited → continued with Claude Code · eco mode [CLI]"
    );
    let local = AgentFallback {
        model: Some("qwen3".into()),
        ..link(AgentType::Ollama)
    };
    assert_eq!(
        notice(
            &AgentType::OpenCode,
            Some("big-pickle"),
            Trigger::Outage,
            None,
            &local,
            ModelTier::Default
        ),
        "OpenCode · big-pickle: provider unavailable → continued with Ollama · qwen3 [Local]"
    );
    let http = link(AgentType::Nvidia);
    assert!(notice(
        &AgentType::Codex,
        None,
        Trigger::RateLimit,
        None,
        &http,
        ModelTier::Reasoning
    )
    .ends_with("NVIDIA · advanced mode [HTTP]"));
    assert!(
        stop_notice(&AgentType::Codex, Trigger::Outage, "nothing usable")
            .starts_with("Codex: provider unavailable → no fallback")
    );
}

// ─── Cooldown ────────────────────────────────────────────────────────────

#[test]
fn the_cooldown_lasts_until_the_announced_reset_else_a_retry_window() {
    let connection = connection();
    let now = at(10);
    let until = set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        now,
    )
    .unwrap();
    assert_eq!(until, at(14));
    assert_eq!(
        active_cooldown(&connection, &AgentType::Codex, now)
            .unwrap()
            .unwrap()
            .until,
        at(14)
    );
    // An announcement already in the past is not one: the window applies.
    let until = set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::RateLimit,
        Some(at(8)),
        now,
    )
    .unwrap();
    assert_eq!(until, now + Duration::minutes(5));
    // No announcement: the trigger's own re-try window.
    let until = set_cooldown(
        &connection,
        &AgentType::ClaudeCode,
        Trigger::NotAuthenticated,
        None,
        now,
    )
    .unwrap();
    assert_eq!(until, now + Duration::minutes(30));
}

#[test]
fn the_agent_returns_by_itself_once_its_cooldown_has_passed() {
    let connection = connection();
    set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        at(10),
    )
    .unwrap();
    assert!(active_cooldown(&connection, &AgentType::Codex, at(13))
        .unwrap()
        .is_some());
    assert!(active_cooldown(&connection, &AgentType::Codex, at(14))
        .unwrap()
        .is_none());
    assert!(list_cooldowns(&connection, at(15)).unwrap().is_empty());
}

#[test]
fn a_provider_that_answers_again_or_a_new_account_ends_the_cooldown_at_once() {
    let connection = connection();
    set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        at(10),
    )
    .unwrap();
    set_cooldown(
        &connection,
        &AgentType::ClaudeCode,
        Trigger::RateLimit,
        None,
        at(10),
    )
    .unwrap();
    clear_cooldown(&connection, &AgentType::Codex).unwrap();
    assert!(active_cooldown(&connection, &AgentType::Codex, at(11))
        .unwrap()
        .is_none());
    assert!(active_cooldown(&connection, &AgentType::ClaudeCode, at(10))
        .unwrap()
        .is_some());
    clear_all_cooldowns(&connection).unwrap();
    assert!(list_cooldowns(&connection, at(10)).unwrap().is_empty());
}

#[test]
fn new_turns_start_on_the_fallback_while_the_agent_cools_down_then_return() {
    let connection = connection();
    let config = agents(&[(AgentType::Codex, vec![link(AgentType::ClaudeCode)])]);
    let chain = default_chain(&config, &AgentType::Codex, FallbackMode::Standard);
    let needs = TurnNeeds::default();
    // Not cooling: the agent itself.
    assert!(preferred_start(
        &connection,
        &config,
        &chain,
        &AgentType::Codex,
        &everyone(),
        &needs,
        at(10)
    )
    .unwrap()
    .is_none());
    set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        at(10),
    )
    .unwrap();
    let (taken, cooldown) = preferred_start(
        &connection,
        &config,
        &chain,
        &AgentType::Codex,
        &everyone(),
        &needs,
        at(11),
    )
    .unwrap()
    .expect("straight to the fallback");
    assert_eq!(taken.agent, AgentType::ClaudeCode);
    assert_eq!(cooldown.until, at(14));
    // Past the reset the agent is back, with nothing to undo.
    assert!(preferred_start(
        &connection,
        &config,
        &chain,
        &AgentType::Codex,
        &everyone(),
        &needs,
        at(14)
    )
    .unwrap()
    .is_none());
    // The fallback cooling too: nothing to move to, the agent is simply tried.
    set_cooldown(
        &connection,
        &AgentType::ClaudeCode,
        Trigger::RateLimit,
        Some(at(13)),
        at(10),
    )
    .unwrap();
    assert!(preferred_start(
        &connection,
        &config,
        &chain,
        &AgentType::Codex,
        &everyone(),
        &needs,
        at(11)
    )
    .unwrap()
    .is_none());
}

#[test]
fn a_turn_continued_on_a_fallback_keeps_its_pin() {
    let connection = connection();
    // The dispatch row it hangs on is the surfaces' tests' business.
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    assert_eq!(get_pin(&connection, "job-1").unwrap(), None);
    let pin = Pin {
        from_agent: AgentType::Codex,
        to_agent: AgentType::ClaudeCode,
        model: Some("sonnet".into()),
        tier: ModelTier::Economy,
        hops: 1,
    };
    set_pin(&connection, "job-1", &pin, Trigger::QuotaExhausted).unwrap();
    assert_eq!(get_pin(&connection, "job-1").unwrap(), Some(pin.clone()));
    set_pin(
        &connection,
        "job-1",
        &Pin { hops: 2, ..pin },
        Trigger::RateLimit,
    )
    .unwrap();
    assert_eq!(get_pin(&connection, "job-1").unwrap().unwrap().hops, 2);
}

#[test]
fn the_overview_lists_the_rows_and_the_cooldowns_for_the_agent_cards() {
    let connection = connection();
    set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        at(10),
    )
    .unwrap();
    let mut config = agents(&[]);
    set_row(
        &mut config,
        &AgentType::Codex,
        FallbackMode::Image,
        Some(AgentFallback {
            mode: Some(FallbackMode::Image),
            ..link(AgentType::ClaudeCode)
        }),
    );
    let overview = overview(&config, list_cooldowns(&connection, at(11)).unwrap());
    assert_eq!(overview.chains.len(), 1);
    assert_eq!(overview.chains[0].agent, AgentType::Codex);
    assert_eq!(overview.chains[0].mode, FallbackMode::Image);
    assert_eq!(overview.cooldowns[0].reason, "quota_exhausted");
}

#[test]
fn the_rows_are_stored_in_config_toml_and_read_back_and_old_configs_still_load() {
    let mut config = agents(&[]);
    set_row(
        &mut config,
        &AgentType::ClaudeCode,
        FallbackMode::Advanced,
        Some(AgentFallback {
            model: Some("gpt-5.6-sol".into()),
            mode: Some(FallbackMode::Standard),
            ..link(AgentType::Codex)
        }),
    );
    let text = toml::to_string(&config.claude_code).unwrap();
    assert!(text.contains("[[mode_fallbacks]]"), "{text}");
    let back: crate::models::AgentConfig = toml::from_str(&text).unwrap();
    assert_eq!(back.mode_fallbacks, config.claude_code.mode_fallbacks);
    // An agent with no row writes nothing, and a config written before the
    // feature existed loads with none.
    assert!(!toml::to_string(&config.codex)
        .unwrap()
        .contains("mode_fallbacks"));
    let old: crate::models::AgentConfig = toml::from_str("installed = true").unwrap();
    assert!(old.mode_fallbacks.is_empty());
    // A 0.15.3 chain is read (with its `allow_paid`, now ignored) and never
    // written back: it is carried onto the rows at start-up.
    let mut legacy: crate::models::AgentConfig =
        toml::from_str("[[fallback_chain]]\nagent = \"LiteLlm\"\nallow_paid = true").unwrap();
    assert_eq!(legacy.fallback_chain.len(), 1);
    assert!(!toml::to_string(&legacy).unwrap().contains("fallback_chain"));
    legacy.fallback_chain.clear();
}

#[test]
fn a_discussion_override_is_stored_per_mode_replaced_and_removed() {
    let connection = connection();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    assert_eq!(get_discussion_override(&connection, "d1").unwrap(), None);
    set_discussion_override(
        &connection,
        "d1",
        FallbackMode::Eco,
        Some(&[link(AgentType::Codex)]),
    )
    .unwrap();
    let eco = ModeOverride {
        mode: FallbackMode::Eco,
        chain: vec![link(AgentType::Codex)],
    };
    assert_eq!(
        get_discussion_override(&connection, "d1").unwrap(),
        Some(vec![eco.clone()])
    );
    // An empty list is "no fallback here", distinct from no override, and the
    // other modes are untouched.
    set_discussion_override(&connection, "d1", FallbackMode::Advanced, Some(&[])).unwrap();
    let stored = get_discussion_override(&connection, "d1").unwrap();
    assert_eq!(
        override_for(stored.as_deref(), FallbackMode::Advanced),
        Some(&[][..])
    );
    assert_eq!(
        override_for(stored.as_deref(), FallbackMode::Eco),
        Some(&[link(AgentType::Codex)][..])
    );
    assert_eq!(
        override_for(stored.as_deref(), FallbackMode::Standard),
        None
    );
    // Back to the default, mode by mode: the last one removes the row.
    set_discussion_override(&connection, "d1", FallbackMode::Advanced, None).unwrap();
    assert_eq!(
        get_discussion_override(&connection, "d1").unwrap(),
        Some(vec![eco])
    );
    set_discussion_override(&connection, "d1", FallbackMode::Eco, None).unwrap();
    assert_eq!(get_discussion_override(&connection, "d1").unwrap(), None);
}

#[test]
fn a_0_15_3_discussion_override_applies_to_the_three_text_modes() {
    let connection = connection();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    for (id, json) in [
        ("d-chain", "[{\"agent\":\"Codex\",\"allow_paid\":true}]"),
        ("d-cut", "[]"),
    ] {
        connection
            .execute(
                "INSERT INTO discussion_fallback_overrides (discussion_id, chain_json, updated_at)
                 VALUES (?1, ?2, '2026-11-10T00:00:00Z')",
                params![id, json],
            )
            .unwrap();
    }
    let chain = get_discussion_override(&connection, "d-chain").unwrap();
    for mode in FallbackMode::TEXT {
        assert_eq!(
            override_for(chain.as_deref(), mode),
            Some(&[link(AgentType::Codex)][..])
        );
    }
    assert_eq!(override_for(chain.as_deref(), FallbackMode::Image), None);
    let cut = get_discussion_override(&connection, "d-cut").unwrap();
    for mode in FallbackMode::TEXT {
        assert_eq!(override_for(cut.as_deref(), mode), Some(&[][..]));
    }
}

#[test]
fn a_worker_remembers_the_agent_it_started_on() {
    let connection = connection();
    assert_eq!(get_execution_origin(&connection, "e1").unwrap(), None);
    note_execution_origin(&connection, "e1", &AgentType::Codex).unwrap();
    // The first call wins: a later hop does not make the link the origin.
    note_execution_origin(&connection, "e1", &AgentType::ClaudeCode).unwrap();
    assert_eq!(
        get_execution_origin(&connection, "e1").unwrap(),
        Some(AgentType::Codex)
    );
}

// ─── KT-1749 — a task worker is never offered what the launch refuses ─────

#[test]
fn a_task_worker_is_never_offered_an_agent_the_launch_would_refuse() {
    let worker = TurnNeeds {
        write_access: true,
        task_worker: true,
        ..Default::default()
    };
    for refused in [
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::CopilotCli,
        AgentType::OpenCode,
        AgentType::Vibe,
        AgentType::Codex,
    ] {
        let config = agents(&[(AgentType::ClaudeCode, vec![link(refused.clone())])]);
        match pick(
            &config,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &everyone(),
            &worker,
            &never_cooling,
        ) {
            Selection::Stop { why } => {
                assert!(why.contains("cannot take this turn"), "{refused:?}: {why}")
            }
            other => panic!("{refused:?}: {other:?}"),
        }
        // The same agent is fine for a plain room turn.
        assert!(matches!(
            pick(
                &config,
                &AgentType::ClaudeCode,
                &AgentType::ClaudeCode,
                &everyone(),
                &TurnNeeds::default(),
                &never_cooling,
            ),
            Selection::Switch(_)
        ));
    }
    for allowed in [AgentType::Ollama, AgentType::LiteLlm, AgentType::Nvidia] {
        let config = agents(&[(AgentType::ClaudeCode, vec![link(allowed.clone())])]);
        assert_eq!(
            pick(
                &config,
                &AgentType::ClaudeCode,
                &AgentType::ClaudeCode,
                &everyone(),
                &worker,
                &never_cooling,
            ),
            Selection::Switch(link(allowed)),
        );
    }
}

#[test]
fn the_worker_target_follows_the_transport_of_the_agent() {
    use crate::models::MessageTargetKind;
    for http in [AgentType::Ollama, AgentType::LiteLlm, AgentType::Nvidia] {
        let target = task_worker_target(&http);
        assert_eq!(target.kind, MessageTargetKind::DiscussionAgent);
        assert_eq!(task_worker_launch_refusal(&http), None, "{http:?}");
    }
    for cli in [AgentType::ClaudeCode, AgentType::Codex] {
        assert_eq!(task_worker_target(&cli).kind, MessageTargetKind::Agent);
    }
    assert!(task_worker_launch_refusal(&AgentType::GeminiCli)
        .is_some_and(|why| why.contains("native ACP")));
}

#[test]
fn the_stop_notice_reads_in_the_room_language() {
    use crate::core::room_lang::RoomLang;
    let fr = stop_notice_in(
        &AgentType::ClaudeCode,
        Trigger::RateLimit,
        "raison",
        RoomLang::Fr,
    );
    assert_eq!(
        fr,
        "Claude Code: limite de débit atteinte → aucun agent de repli (raison)"
    );
    assert_eq!(
        stop_notice_in(&AgentType::ClaudeCode, Trigger::Outage, "why", RoomLang::En),
        stop_notice(&AgentType::ClaudeCode, Trigger::Outage, "why")
    );
}
