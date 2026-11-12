use super::*;
use crate::models::AgentFallback;

fn agents(chains: &[(AgentType, Vec<AgentFallback>)]) -> AgentsConfig {
    let mut agents = crate::core::config::default_config().agents;
    for (from, chain) in chains {
        agents.agent_config_mut(from).unwrap().fallback_chain = chain.clone();
    }
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
        default_chain(config, origin),
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
        allow_paid: false,
    }
}

fn paid_ok(agent: AgentType) -> AgentFallback {
    AgentFallback {
        allow_paid: true,
        ..link(agent)
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
        agents_of(default_chain(&config, &AgentType::ClaudeCode)),
        vec![AgentType::Codex, AgentType::Ollama]
    );
    assert!(default_chain(&config, &AgentType::Codex).is_empty());
    validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
        default_chain(&config, &AgentType::ClaudeCode),
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
            Some(&own)
        )),
        vec![AgentType::GeminiCli]
    );
    assert!(effective_chain(&config, &AgentType::ClaudeCode, Some(&[])).is_empty());
    assert_eq!(
        agents_of(&effective_chain(&config, &AgentType::ClaudeCode, None)),
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
        &[link(AgentType::Codex), link(AgentType::Codex)],
    )
    .expect_err("Codex twice");
    assert!(matches!(error, ChainError::Cycle(_)), "{error}");
    assert!(error.to_string().contains("Claude Code → Codex → Codex"));
    assert_eq!(
        validate_chain(
            &config,
            Some(&AgentType::Codex),
            &[link(AgentType::GeminiCli), link(AgentType::Codex)]
        ),
        Err(ChainError::SelfFallback(AgentType::Codex)),
        "a chain that comes back to its own agent loops"
    );
    // A written-by-hand loop is not walked either.
    let mut hand = agents(&[]);
    hand.claude_code.fallback_chain = vec![link(AgentType::Codex), link(AgentType::Codex)];
    match pick(
        &hand,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &everyone(),
        &TurnNeeds::default(),
        &never_cooling,
    ) {
        Selection::Stop { why } => assert!(why.contains("loop"), "{why}"),
        other => panic!("{other:?}"),
    }
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
    validate_chain(&config, Some(&AgentType::ClaudeCode), &four).unwrap();
    let five: Vec<AgentFallback> = four
        .iter()
        .cloned()
        .chain([link(AgentType::CopilotCli)])
        .collect();
    let error = validate_chain(&config, Some(&AgentType::ClaudeCode), &five)
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
        &[link(AgentType::Codex)],
    )
    .unwrap_err();
    assert!(matches!(error, ChainError::BroaderAccess { .. }), "{error}");
    assert!(error.to_string().contains("more access"));
    config.claude_code.full_access = true;
    validate_chain(
        &config,
        Some(&AgentType::ClaudeCode),
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
            &[link(AgentType::Custom)]
        ),
        Err(ChainError::NoCardFor(AgentType::Custom))
    );
    let blank = AgentFallback {
        model: Some("   ".into()),
        ..link(AgentType::Codex)
    };
    assert!(matches!(
        validate_chain(&config, Some(&AgentType::ClaudeCode), &[blank]),
        Err(ChainError::BadModel(_))
    ));
    let named = AgentFallback {
        model: Some("sonnet".into()),
        ..link(AgentType::Codex)
    };
    validate_chain(&config, Some(&AgentType::ClaudeCode), &[named]).unwrap();
}

#[test]
fn the_chain_view_shows_the_gaps_of_each_link_as_warnings() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![
            link(AgentType::Codex),
            paid_ok(AgentType::LiteLlm),
            link(AgentType::Ollama),
        ],
    )]);
    let view = chain_view(&config, &AgentType::ClaudeCode);
    assert_eq!(view.error, None);
    assert_eq!(view.links.len(), 3);
    assert!(view.links[0].gaps.is_empty(), "a CLI agent has everything");
    assert!(view.links[1].paid && view.links[1].allow_paid);
    assert!(view.links[1].gaps.contains(&"no MCP tools".to_string()));
    assert!(view.links[2]
        .gaps
        .contains(&"capped context window".to_string()));
    // A link lacks only what the agent it replaces had: Ollama after Ollama-class
    // agents is no gap, and Claude Code → itself is flagged as an error.
    let looped = agents(&[(
        AgentType::ClaudeCode,
        vec![link(AgentType::Codex), link(AgentType::Codex)],
    )]);
    assert!(chain_view(&looped, &AgentType::ClaudeCode).error.is_some());
    let own = chain_view_of(&config, None, &[link(AgentType::Ollama)]);
    assert!(own.links[0].gaps.contains(&"no MCP tools".to_string()));
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
fn a_paid_fallback_is_used_only_with_the_owners_opt_in() {
    let usable = everyone();
    let without = agents(&[(AgentType::ClaudeCode, vec![link(AgentType::LiteLlm)])]);
    match pick(
        &without,
        &AgentType::ClaudeCode,
        &AgentType::ClaudeCode,
        &usable,
        &TurnNeeds::default(),
        &never_cooling,
    ) {
        Selection::Stop { why } => assert!(why.contains("bills by API key"), "{why}"),
        other => panic!("a paid link without opt-in must not be used: {other:?}"),
    }
    let with = agents(&[(AgentType::ClaudeCode, vec![paid_ok(AgentType::LiteLlm)])]);
    assert_eq!(
        pick(
            &with,
            &AgentType::ClaudeCode,
            &AgentType::ClaudeCode,
            &usable,
            &TurnNeeds::default(),
            &never_cooling
        ),
        Selection::Switch(paid_ok(AgentType::LiteLlm))
    );
    assert!(
        is_paid(&AgentType::Nvidia) && !is_paid(&AgentType::Codex) && !is_paid(&AgentType::Ollama)
    );
}

#[test]
fn a_link_that_lacks_what_the_turn_needs_is_skipped() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![paid_ok(AgentType::LiteLlm), link(AgentType::Codex)],
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
        "Codex (gpt-5.6-sol): quota exhausted until 14:00 UTC -> continued with Claude Code (sonnet)"
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
        "Codex (economy tier): rate limited -> continued with Claude Code (economy tier)"
    );
    assert!(
        stop_notice(&AgentType::Codex, Trigger::Outage, "nothing usable")
            .starts_with("Codex: provider unavailable -> no fallback")
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
    let chain = default_chain(&config, &AgentType::Codex).to_vec();
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
fn the_overview_lists_the_chains_and_the_cooldowns_for_the_agent_cards() {
    let connection = connection();
    set_cooldown(
        &connection,
        &AgentType::Codex,
        Trigger::QuotaExhausted,
        Some(at(14)),
        at(10),
    )
    .unwrap();
    let config = agents(&[(AgentType::Codex, vec![link(AgentType::ClaudeCode)])]);
    let overview = overview(&config, list_cooldowns(&connection, at(11)).unwrap());
    assert_eq!(overview.chains.len(), 1);
    assert_eq!(overview.chains[0].agent, AgentType::Codex);
    assert_eq!(overview.cooldowns[0].reason, "quota_exhausted");
}

#[test]
fn the_chain_is_stored_in_config_toml_and_read_back_and_old_configs_still_load() {
    let config = agents(&[(
        AgentType::ClaudeCode,
        vec![
            AgentFallback {
                agent: AgentType::Codex,
                model: Some("gpt-5.6-sol".into()),
                allow_paid: false,
            },
            link(AgentType::Ollama),
        ],
    )]);
    let text = toml::to_string(&config.claude_code).unwrap();
    assert!(text.contains("[[fallback_chain]]"), "{text}");
    let back: crate::models::AgentConfig = toml::from_str(&text).unwrap();
    assert_eq!(back.fallback_chain, config.claude_code.fallback_chain);
    // An agent with no chain writes nothing, and a config written before the
    // feature existed loads with none.
    assert!(!toml::to_string(&config.codex)
        .unwrap()
        .contains("fallback_chain"));
    let old: crate::models::AgentConfig = toml::from_str("installed = true").unwrap();
    assert!(old.fallback_chain.is_empty());
    // The opt-in defaults to refused when a hand-written link omits it.
    let hand: crate::models::AgentConfig =
        toml::from_str("[[fallback_chain]]\nagent = \"LiteLlm\"").unwrap();
    assert!(!hand.fallback_chain[0].allow_paid);
}

#[test]
fn a_discussion_override_is_stored_replaced_and_removed() {
    let connection = connection();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    assert_eq!(get_discussion_override(&connection, "d1").unwrap(), None);
    set_discussion_override(&connection, "d1", Some(&[link(AgentType::Codex)])).unwrap();
    assert_eq!(
        get_discussion_override(&connection, "d1").unwrap(),
        Some(vec![link(AgentType::Codex)])
    );
    // An empty list is "no fallback here", distinct from no override.
    set_discussion_override(&connection, "d1", Some(&[])).unwrap();
    assert_eq!(
        get_discussion_override(&connection, "d1").unwrap(),
        Some(vec![])
    );
    set_discussion_override(&connection, "d1", None).unwrap();
    assert_eq!(get_discussion_override(&connection, "d1").unwrap(), None);
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
