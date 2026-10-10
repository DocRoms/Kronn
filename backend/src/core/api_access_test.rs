use super::*;
use crate::models::ApiEndpointAccess;

fn ep(method: &str, path: &str) -> ApiEndpoint {
    ApiEndpoint {
        method: method.into(),
        path: path.into(),
        description: String::new(),
    }
}

fn declared() -> Vec<ApiEndpoint> {
    vec![
        ep("GET", "/users/me"),
        ep("GET", "/pages/{page_id}"),
        ep("PATCH", "/pages/{page_id}"),
    ]
}

fn target(method: &str, path: &str) -> Target {
    Target {
        method: method.into(),
        path: path.into(),
    }
}

fn identity(agent: AgentType, model: Option<&str>) -> AgentIdentity {
    AgentIdentity {
        agent_type: agent,
        model: model.map(str::to_owned),
    }
}

fn local_member() -> Member {
    Member {
        identity: Some(identity(AgentType::Ollama, Some("qwen3:8b"))),
        local: true,
    }
}

fn remote_member(agent: AgentType) -> Member {
    Member {
        identity: Some(identity(agent, None)),
        local: false,
    }
}

fn agent(caller: Member, audience: Vec<Member>) -> Principal {
    Principal::Agent { caller, audience }
}

fn policy(access: ApiAccessRule) -> ApiAccessPolicy {
    ApiAccessPolicy {
        access,
        endpoints: vec![],
    }
}

fn run(p: &ApiAccessPolicy, t: &Target, who: &Principal) -> Result<(), String> {
    decide("notion", p, &declared(), t, &t.path, who)
}

#[test]
fn strict_mode_refuses_an_undeclared_path() {
    let p = policy(ApiAccessRule::All);
    let who = Principal::Human;
    let err = run(&p, &target("POST", "/pages"), &who).unwrap_err();
    assert!(err.contains("not a declared endpoint"), "{err}");
    // The bypass the ticket names: a sibling path reaching the same data.
    assert!(run(&p, &target("GET", "/blocks/abc/children"), &who).is_err());
    assert!(run(&p, &target("GET", "/pages/abc"), &who).is_ok());
}

#[test]
fn strict_mode_refuses_a_declared_path_with_another_method() {
    let p = policy(ApiAccessRule::All);
    let err = run(&p, &target("DELETE", "/pages/abc"), &Principal::Human).unwrap_err();
    assert!(err.contains("not a declared endpoint"), "{err}");
}

#[test]
fn a_placeholder_matches_exactly_one_segment() {
    let p = policy(ApiAccessRule::All);
    assert!(run(&p, &target("GET", "/pages/a/b"), &Principal::Human).is_err());
    assert!(run(&p, &target("GET", "/pages"), &Principal::Human).is_err());
    assert!(run(&p, &target("GET", "/users/me/"), &Principal::Human).is_ok());
}

#[test]
fn ambiguous_paths_are_refused() {
    let p = policy(ApiAccessRule::All);
    for path in [
        "/pages/a%2Fb",
        "/pages/a%2fb",
        "/pages/%2e%2e",
        "/pages/..",
        "/pages//x",
        "/pages/a%5Cb",
    ] {
        let err = run(&p, &target("GET", path), &Principal::Human).unwrap_err();
        assert!(err.contains("ambiguous"), "{path}: {err}");
    }
}

#[test]
fn an_endpoint_rule_overrides_the_plugin_rule() {
    let mut p = policy(ApiAccessRule::LocalOnly);
    p.endpoints.push(ApiEndpointAccess {
        method: "GET".into(),
        path: "/users/me".into(),
        access: ApiAccessRule::All,
    });
    p.endpoints.push(ApiEndpointAccess {
        method: "PATCH".into(),
        path: "/pages/{page_id}".into(),
        access: ApiAccessRule::Blocked,
    });
    let remote = agent(remote_member(AgentType::ClaudeCode), vec![]);
    assert!(run(&p, &target("GET", "/users/me"), &remote).is_ok());
    assert!(run(&p, &target("GET", "/pages/x"), &remote).is_err());
    let local = agent(local_member(), vec![]);
    assert!(run(&p, &target("GET", "/pages/x"), &local).is_ok());
    let err = run(&p, &target("PATCH", "/pages/x"), &local).unwrap_err();
    assert!(err.contains("blocked"), "{err}");
}

#[test]
fn a_policy_endpoint_is_declared_too() {
    let mut p = policy(ApiAccessRule::All);
    p.endpoints.push(ApiEndpointAccess {
        method: "POST".into(),
        path: "/search".into(),
        access: ApiAccessRule::All,
    });
    assert!(run(&p, &target("POST", "/search"), &Principal::Human).is_ok());
}

#[test]
fn an_agents_rule_admits_only_the_listed_agents() {
    let p = policy(ApiAccessRule::Agents {
        agents: vec![ApiAccessSubject {
            agent: AgentType::Codex,
            model: None,
        }],
    });
    let t = target("GET", "/users/me");
    assert!(run(&p, &t, &agent(remote_member(AgentType::Codex), vec![])).is_ok());
    let err = run(&p, &t, &agent(remote_member(AgentType::ClaudeCode), vec![])).unwrap_err();
    assert!(err.contains("reserved to Codex"), "{err}");
    let nobody = agent(
        Member {
            identity: None,
            local: false,
        },
        vec![],
    );
    let err = run(&p, &t, &nobody).unwrap_err();
    assert!(err.contains("no Kronn-issued identity"), "{err}");
    assert!(run(&p, &t, &Principal::Human).is_ok());
}

#[test]
fn an_agents_rule_can_pin_a_model() {
    let p = policy(ApiAccessRule::Agents {
        agents: vec![ApiAccessSubject {
            agent: AgentType::Ollama,
            model: Some("qwen3:8b".into()),
        }],
    });
    let t = target("GET", "/users/me");
    assert!(run(&p, &t, &agent(local_member(), vec![])).is_ok());
    let other = Member {
        identity: Some(identity(AgentType::Ollama, Some("llama3:70b"))),
        local: true,
    };
    assert!(run(&p, &t, &agent(other, vec![])).is_err());
}

#[test]
fn an_agents_rule_on_a_workflow_needs_every_agent_step_listed() {
    let p = policy(ApiAccessRule::Agents {
        agents: vec![ApiAccessSubject {
            agent: AgentType::Ollama,
            model: None,
        }],
    });
    let t = target("GET", "/users/me");
    let ok = Principal::Workflow {
        audience: vec![local_member()],
    };
    assert!(run(&p, &t, &ok).is_ok());
    let mixed = Principal::Workflow {
        audience: vec![local_member(), remote_member(AgentType::Codex)],
    };
    assert!(run(&p, &t, &mixed).is_err());
}

#[test]
fn local_only_refuses_a_remote_agent() {
    let p = policy(ApiAccessRule::LocalOnly);
    let t = target("GET", "/users/me");
    let err = run(&p, &t, &agent(remote_member(AgentType::ClaudeCode), vec![])).unwrap_err();
    assert!(err.contains("local models"), "{err}");
    assert!(run(&p, &t, &agent(local_member(), vec![local_member()])).is_ok());
}

#[test]
fn local_only_refuses_a_discussion_with_a_remote_agent() {
    let p = policy(ApiAccessRule::LocalOnly);
    let t = target("GET", "/users/me");
    let who = agent(
        local_member(),
        vec![local_member(), remote_member(AgentType::ClaudeCode)],
    );
    let err = run(&p, &t, &who).unwrap_err();
    assert!(err.contains("remote agent takes part"), "{err}");
}

#[test]
fn local_only_refuses_a_workflow_with_a_remote_agent_step() {
    let p = policy(ApiAccessRule::LocalOnly);
    let t = target("GET", "/users/me");
    let remote = Principal::Workflow {
        audience: vec![local_member(), remote_member(AgentType::Codex)],
    };
    assert!(run(&p, &t, &remote).is_err());
    let unknown = Principal::Workflow {
        audience: vec![Member {
            identity: None,
            local: false,
        }],
    };
    assert!(run(&p, &t, &unknown).is_err());
    let local = Principal::Workflow {
        audience: vec![local_member()],
    };
    assert!(run(&p, &t, &local).is_ok());
    assert!(run(&p, &t, &Principal::Workflow { audience: vec![] }).is_ok());
}

#[test]
fn blocked_refuses_everyone() {
    let p = policy(ApiAccessRule::Blocked);
    let t = target("GET", "/users/me");
    assert!(run(&p, &t, &Principal::Human).is_err());
    assert!(run(&p, &t, &agent(local_member(), vec![])).is_err());
}

#[test]
fn local_means_ollama_on_loopback_with_a_known_non_cloud_model() {
    let loopback = "http://localhost:11434";
    assert!(is_local(
        &identity(AgentType::Ollama, Some("qwen3:8b")),
        loopback
    ));
    assert!(is_local(
        &identity(AgentType::Ollama, Some("qwen3:8b")),
        "http://127.0.0.1:11434"
    ));
    assert!(is_local(
        &identity(AgentType::Ollama, Some("qwen3:8b")),
        "http://[::1]:11434"
    ));
    assert!(!is_local(
        &identity(AgentType::Ollama, Some("gpt-oss:120b-cloud")),
        loopback
    ));
    assert!(!is_local(
        &identity(AgentType::Ollama, Some("kimi:cloud")),
        loopback
    ));
    assert!(!is_local(&identity(AgentType::Ollama, None), loopback));
    assert!(!is_local(
        &identity(AgentType::Ollama, Some("qwen3:8b")),
        "http://192.168.1.20:11434"
    ));
    assert!(!is_local(
        &identity(AgentType::LiteLlm, Some("qwen3:8b")),
        loopback
    ));
    assert!(!is_local(&identity(AgentType::ClaudeCode, None), loopback));
}

#[test]
fn a_policy_is_normalised_and_checked_before_it_is_stored() {
    let mut p = policy(ApiAccessRule::All);
    p.endpoints.push(ApiEndpointAccess {
        method: " get ".into(),
        path: "pages/{id}/".into(),
        access: ApiAccessRule::Blocked,
    });
    let stored = normalize_policy(p.clone()).unwrap();
    assert_eq!(stored.endpoints[0].method, "GET");
    assert_eq!(stored.endpoints[0].path, "/pages/{id}");
    for (method, path) in [
        ("BREW", "/x"),
        ("GET", ""),
        ("GET", "/x?y=1"),
        ("GET", "https://evil/x"),
        ("GET", "/a/../b"),
    ] {
        let mut bad = policy(ApiAccessRule::All);
        bad.endpoints.push(ApiEndpointAccess {
            method: method.into(),
            path: path.into(),
            access: ApiAccessRule::All,
        });
        assert!(normalize_policy(bad).is_err(), "{method} {path}");
    }
    p.endpoints.push(p.endpoints[0].clone());
    assert!(normalize_policy(p).is_err(), "duplicate rule accepted");
}

#[test]
fn discussion_agents_lists_the_agent_and_every_participant() {
    let now = chrono::Utc::now();
    let disc = crate::models::Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: "d".into(),
        project_id: None,
        title: "d".into(),
        agent: AgentType::Ollama,
        language: "en".into(),
        participants: vec![AgentType::Ollama, AgentType::ClaudeCode],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Default,
        model: Some("qwen3:8b".into()),
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::OnDemand,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let agents = discussion_agents(&disc, None);
    assert_eq!(agents.len(), 2);
    assert_eq!(
        agents[0],
        Some(identity(AgentType::Ollama, Some("qwen3:8b")))
    );
    assert_eq!(
        agents[1].as_ref().map(|a| a.agent_type.clone()),
        Some(AgentType::ClaudeCode)
    );
}

#[tokio::test]
async fn agent_steps_cover_rollbacks_and_flag_unnamed_runners() {
    let db = crate::db::Database::open_in_memory().unwrap();
    let mut workflow = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflows(id, name, trigger_json, steps_json, created_at, updated_at) \
                 VALUES ('wf', 'wf', '{\"type\":\"Manual\"}', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            crate::db::workflows::get_workflow(conn, "wf")
        })
        .await
        .unwrap()
        .unwrap();
    workflow.steps = vec![
        WorkflowStep {
            step_type: StepType::ApiCall,
            ..WorkflowStep::default()
        },
        WorkflowStep {
            name: "main".into(),
            step_type: StepType::Agent,
            agent: AgentType::Ollama,
            ..WorkflowStep::default()
        },
        WorkflowStep {
            step_type: StepType::BatchQuickPrompt,
            ..WorkflowStep::default()
        },
    ];
    workflow.on_failure = vec![WorkflowStep {
        name: "rollback".into(),
        step_type: StepType::Agent,
        agent: AgentType::Codex,
        ..WorkflowStep::default()
    }];
    let steps: Vec<Option<&str>> = agent_steps(&workflow)
        .into_iter()
        .map(|step| step.map(|s| s.name.as_str()))
        .collect();
    assert_eq!(steps, vec![Some("main"), None, Some("rollback")]);
}

#[test]
fn a_mixed_segment_keeps_its_literal_prefix_and_suffix() {
    let mut p = policy(ApiAccessRule::All);
    p.endpoints.push(ApiEndpointAccess {
        method: "GET".into(),
        path: "/files/report-{id}.json".into(),
        access: ApiAccessRule::All,
    });
    let who = Principal::Human;
    assert!(run(&p, &target("GET", "/files/report-42.json"), &who).is_ok());
    for neighbour in [
        "/files/private.csv",
        "/files/report-42.csv",
        "/files/summary-42.json",
        "/files/report-.json",
    ] {
        let err = run(&p, &target("GET", neighbour), &who).unwrap_err();
        assert!(
            err.contains("not a declared endpoint"),
            "{neighbour}: {err}"
        );
    }
}

#[test]
fn malformed_braces_never_match_and_are_refused_on_save() {
    for template in [
        "/files/{id",
        "/files/id}",
        "/files/{a}{b}",
        "/files/{}",
        "/files/{{x}",
    ] {
        assert!(
            !template_matches(template, &["files", "anything"]),
            "{template} matched"
        );
        assert!(
            normalize_endpoint("GET", template).is_err(),
            "{template} accepted"
        );
    }
    // Every placeholder form, with literal text around it.
    assert!(template_matches("/a/v{{n}}", &["a", "v2"]));
    assert!(template_matches("/a/${ENV.ORG}-x", &["a", "acme-x"]));
    assert!(template_matches(
        "/odata/$metadata",
        &["odata", "$metadata"]
    ));
}

#[test]
fn encoded_unreserved_characters_match_as_their_plain_form() {
    assert_eq!(
        request_segments("/users/%6De"),
        Some(vec!["users".into(), "me".into()])
    );
    assert_eq!(
        request_segments("/a/%7e%5F"),
        Some(vec!["a".into(), "~_".into()])
    );
    // Reserved and non-ASCII escapes stay escaped, with uppercase hex.
    assert_eq!(
        request_segments("/a/b%3ac"),
        Some(vec!["a".into(), "b%3Ac".into()])
    );
    for bad in ["/a/%2F", "/a/%5c", "/a/%2E%2E", "/a/%zz", "/a/%6"] {
        assert_eq!(request_segments(bad), None, "{bad}");
    }
}

#[test]
fn every_agent_running_step_type_is_an_unknown_audience() {
    // Step types that may hand data to an agent Kronn cannot name. A new
    // step type falls in this group by default (the match's catch-all).
    for kind in [
        StepType::BatchQuickPrompt,
        StepType::SubWorkflow,
        StepType::TriggerWorkflow,
    ] {
        assert!(
            matches!(
                audience_slot(&WorkflowStep {
                    step_type: kind.clone(),
                    ..WorkflowStep::default()
                }),
                Some(None)
            ),
            "{kind:?}"
        );
    }
    assert!(audience_slot(&WorkflowStep {
        step_type: StepType::ApiCall,
        ..WorkflowStep::default()
    })
    .is_none());
}

#[test]
fn a_task_board_step_is_an_agentless_audience() {
    // The default Todo's board steps must not fall in the unknown, remote slot.
    assert!(audience_slot(&WorkflowStep {
        step_type: StepType::TaskBoard,
        ..WorkflowStep::default()
    })
    .is_none());
}

#[test]
fn rules_and_requests_share_one_canonical_form() {
    assert_eq!(canonical_segment("caf\u{e9}").as_deref(), Some("caf%C3%A9"));
    assert_eq!(canonical_segment("caf%c3%a9").as_deref(), Some("caf%C3%A9"));
    assert_ne!(canonical_segment("caf\u{e9}").as_deref(), Some("caf%E9"));
    assert_eq!(canonical_segment("b%3ac").as_deref(), Some("b%3Ac"));
    assert_eq!(canonical_segment("%6De").as_deref(), Some("me"));
    assert_eq!(
        normalize_endpoint("GET", "/users/%6De").unwrap().1,
        "/users/me"
    );
    assert_eq!(
        normalize_endpoint("GET", "/files/report-%7B{id}.json")
            .unwrap()
            .1,
        "/files/report-%7B{id}.json"
    );
    assert!(normalize_endpoint("GET", "/a/%2E%2E").is_err());
    assert!(normalize_endpoint("GET", "/a/%2f").is_err());
}

#[test]
fn rules_equal_after_canonicalisation_are_duplicates() {
    for (a, b) in [
        ("/users/me", "/users/%6De"),
        ("/pages/a%3ab", "/pages/a%3Ab"),
        ("/pages/caf\u{e9}", "/pages/caf%c3%a9"),
    ] {
        let mut p = policy(ApiAccessRule::All);
        for path in [a, b] {
            p.endpoints.push(ApiEndpointAccess {
                method: "GET".into(),
                path: path.into(),
                access: ApiAccessRule::Blocked,
            });
        }
        let err = normalize_policy(p).unwrap_err();
        assert!(err.contains("two rules"), "{a} / {b}: {err}");
    }
}

#[test]
fn the_canonical_form_is_the_one_the_url_serializer_sends() {
    // Every printable ASCII character but the ones a segment cannot hold
    // or that carry path structure, written raw in a URL path, comes out of
    // `url` exactly as the canonical form spells it.
    for byte in 0x00u8..=0x7f {
        let c = byte as char;
        if matches!(c, '/' | '\\' | '?' | '#' | '%' | '.') {
            continue;
        }
        let raw = format!("a{c}b");
        let parsed = reqwest::Url::parse(&format!("http://h/{raw}")).unwrap();
        let sent = parsed.path().trim_start_matches('/').to_string();
        if c.is_ascii_control() {
            // `url` drops TAB, CR and LF and escapes the other controls: a
            // rule spelling one raw would name another destination, so it
            // has no canonical form at all.
            assert_eq!(canonical_segment(&raw), None, "{c:?}: url sends {sent}");
            continue;
        }
        assert_eq!(
            canonical_segment(&raw),
            canonical_segment(&sent),
            "{c:?}: url sends {sent}"
        );
    }
    assert_eq!(
        canonical_segment("Annual Report.pdf").as_deref(),
        Some("Annual%20Report.pdf")
    );
}

#[test]
fn a_rule_with_a_control_character_is_refused_at_save() {
    for c in ['\t', '\r', '\n', '\u{0}', '\u{1f}', '\u{7f}'] {
        let path = format!("/files/sec{c}ret");
        assert!(normalize_endpoint("GET", &path).is_err(), "{c:?} accepted");
    }
}

#[test]
fn only_ascii_spaces_are_trimmed_and_unicode_spaces_are_kept() {
    assert_eq!(normalize_endpoint("GET", "  /a/b ").unwrap().1, "/a/b");
    for (c, escaped) in [
        ('\u{a0}', "%C2%A0"),
        ('\u{2000}', "%E2%80%80"),
        ('\u{3000}', "%E3%80%80"),
    ] {
        assert_eq!(
            normalize_endpoint("GET", &format!("/files/report{c}"))
                .unwrap()
                .1,
            format!("/files/report{escaped}")
        );
        assert_eq!(
            normalize_endpoint("GET", &format!("/files/{c}report"))
                .unwrap()
                .1,
            format!("/files/{escaped}report")
        );
    }
}

// ─── Through the executor, counting outbound requests ───────────────────

mod broker {
    use super::*;
    use crate::models::*;
    use crate::workflows::api_call_executor::{execute_api_call_step_with_db, SecurityPolicy};
    use crate::workflows::template::TemplateContext;
    use std::collections::HashMap;
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "kt1026-secret-token-never-in-output";

    async fn state_with_plugin(base_url: &str) -> crate::AppState {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let secret = crate::core::crypto::generate_secret();
        state.config.write().await.encryption_secret = Some(secret.clone());
        let plugin = McpServer {
            id: "custom-notion".into(),
            name: "Notion".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(ApiSpec {
                base_url: format!("{base_url}/v1"),
                auth: ApiAuthKind::Bearer {
                    env_key: "NOTION_TOKEN".into(),
                },
                endpoints: vec![ep("GET", "/users/me"), ep("GET", "/pages/{page_id}")],
                docs_url: None,
                config_keys: vec![],
                default_headers: vec![],
                test_endpoint: None,
            }),
        };
        let env = HashMap::from([("NOTION_TOKEN".to_string(), TOKEN.to_string())]);
        let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
        state
            .db
            .with_conn(move |conn| -> anyhow::Result<()> {
                crate::db::mcps::upsert_server(conn, &plugin)?;
                crate::db::mcps::insert_config(
                    conn,
                    &McpConfig {
                        id: "cfg-notion".into(),
                        server_id: "custom-notion".into(),
                        label: "Notion".into(),
                        env_keys: vec!["NOTION_TOKEN".into()],
                        env_encrypted: encrypted,
                        args_override: None,
                        is_global: true,
                        include_general: true,
                        config_hash: "kt1026".into(),
                        project_ids: Vec::new(),
                        host_sync: HostSyncMode::None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        state
    }

    async fn set_policy(state: &crate::AppState, policy: ApiAccessPolicy) {
        state
            .db
            .with_conn(move |conn| {
                crate::db::api_access_policies::set(conn, "custom-notion", &policy)
            })
            .await
            .unwrap();
    }

    async fn server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;
        server
    }

    fn step(method: &str, path: &str) -> WorkflowStep {
        WorkflowStep {
            name: "call".into(),
            step_type: StepType::ApiCall,
            api_plugin_slug: Some("custom-notion".into()),
            api_config_id: Some("cfg-notion".into()),
            api_endpoint_path: Some(path.into()),
            api_method: Some(method.into()),
            api_max_retries: Some(0),
            ..WorkflowStep::default()
        }
    }

    async fn call(
        state: &crate::AppState,
        step: &WorkflowStep,
        caller: &ApiCaller,
    ) -> crate::models::StepResult {
        execute_api_call_step_with_db(
            step,
            None,
            state,
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
            caller,
        )
        .await
        .result
    }

    async fn sent(server: &MockServer) -> usize {
        server.received_requests().await.unwrap_or_default().len()
    }

    fn agent_caller(agent: AgentType) -> ApiCaller {
        ApiCaller::Agent {
            identity: Some(AgentIdentity {
                agent_type: agent,
                model: None,
            }),
            discussion_ids: vec![],
            workflow_run_id: None,
        }
    }

    fn codex_only() -> ApiAccessPolicy {
        ApiAccessPolicy {
            access: ApiAccessRule::Agents {
                agents: vec![ApiAccessSubject {
                    agent: AgentType::Codex,
                    model: None,
                }],
            },
            endpoints: vec![],
        }
    }

    #[tokio::test]
    async fn without_a_policy_any_agent_and_any_path_go_through_as_before() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        let result = call(
            &state,
            &step("POST", "/pages"),
            &ApiCaller::unidentified_agent(),
        )
        .await;
        assert_eq!(result.status, RunStatus::Success, "{}", result.output);
        assert_eq!(sent(&server).await, 1);
    }

    #[tokio::test]
    async fn an_allowed_agent_reaches_its_endpoint() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, codex_only()).await;
        let result = call(
            &state,
            &step("GET", "/users/me"),
            &agent_caller(AgentType::Codex),
        )
        .await;
        assert_eq!(result.status, RunStatus::Success, "{}", result.output);
        assert_eq!(sent(&server).await, 1);
    }

    #[tokio::test]
    async fn a_refused_agent_sends_nothing() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, codex_only()).await;
        for caller in [
            agent_caller(AgentType::ClaudeCode),
            ApiCaller::unidentified_agent(),
        ] {
            let result = call(&state, &step("GET", "/users/me"), &caller).await;
            assert_eq!(result.status, RunStatus::Failed);
            assert!(result.output.contains("Access policy"), "{}", result.output);
            assert!(!result.output.contains(TOKEN));
        }
        assert_eq!(sent(&server).await, 0, "a refused call reached the API");
    }

    #[tokio::test]
    async fn an_undeclared_path_or_method_sends_nothing() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, codex_only()).await;
        let codex = agent_caller(AgentType::Codex);
        for (method, path) in [
            ("POST", "/pages"),
            ("GET", "/blocks/abc/children"),
            ("DELETE", "/pages/abc"),
            ("GET", "/users/me/../../v1/pages"),
            ("GET", "/../other"),
        ] {
            let result = call(&state, &step(method, path), &codex).await;
            assert_eq!(result.status, RunStatus::Failed, "{method} {path}");
            assert!(
                result.output.contains("Access policy"),
                "{method} {path}: {}",
                result.output
            );
        }
        assert_eq!(sent(&server).await, 0, "an undeclared call reached the API");
    }

    #[tokio::test]
    async fn a_path_param_cannot_smuggle_another_path() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, codex_only()).await;
        let mut smuggle = step("GET", "/pages/{page_id}");
        smuggle.api_path_params = Some(HashMap::from([(
            "page_id".to_string(),
            "x/../../blocks".to_string(),
        )]));
        let result = call(&state, &smuggle, &agent_caller(AgentType::Codex)).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert_eq!(sent(&server).await, 0);
    }

    #[tokio::test]
    async fn local_only_refuses_a_workflow_with_a_remote_agent_step() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(
            &state,
            ApiAccessPolicy {
                access: ApiAccessRule::LocalOnly,
                endpoints: vec![],
            },
        )
        .await;
        let workflow = ApiCaller::Workflow {
            agents: vec![Some(AgentIdentity {
                agent_type: AgentType::ClaudeCode,
                model: None,
            })],
        };
        let result = call(&state, &step("GET", "/users/me"), &workflow).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert_eq!(sent(&server).await, 0);
        let human = call(&state, &step("GET", "/users/me"), &ApiCaller::Human).await;
        assert_eq!(human.status, RunStatus::Success, "{}", human.output);
        assert_eq!(sent(&server).await, 1);
    }

    #[tokio::test]
    async fn local_only_refuses_a_discussion_with_a_remote_participant() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(
            &state,
            ApiAccessPolicy {
                access: ApiAccessRule::LocalOnly,
                endpoints: vec![],
            },
        )
        .await;
        let now = chrono::Utc::now();
        for (id, participants) in [
            ("room-local", vec![AgentType::Ollama]),
            ("room-mixed", vec![AgentType::Ollama, AgentType::ClaudeCode]),
        ] {
            let disc = crate::models::Discussion {
                connection_id: None,
                awaiting_agent: false,
                agent_running: false,
                id: id.into(),
                project_id: None,
                title: id.into(),
                agent: AgentType::Ollama,
                language: "en".into(),
                participants,
                messages: vec![],
                message_count: 0,
                non_system_message_count: 0,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                archived: false,
                pinned: false,
                workspace_mode: "Direct".into(),
                workspace_path: None,
                worktree_branch: None,
                tier: ModelTier::Default,
                model: Some("qwen3:8b".into()),
                pin_first_message: false,
                summary_cache: None,
                summary_up_to_msg_idx: None,
                summary_strategy: crate::models::SummaryStrategy::OnDemand,
                introspection_call_count: 0,
                shared_id: None,
                shared_with: vec![],
                workflow_run_id: None,
                test_mode_restore_branch: None,
                test_mode_stash_ref: None,
                created_at: now,
                updated_at: now,
            };
            state
                .db
                .with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &disc))
                .await
                .unwrap();
        }
        let ollama = |room: &str| ApiCaller::Agent {
            identity: Some(AgentIdentity {
                agent_type: AgentType::Ollama,
                model: Some("qwen3:8b".into()),
            }),
            discussion_ids: vec![room.into()],
            workflow_run_id: None,
        };
        let mixed = call(&state, &step("GET", "/users/me"), &ollama("room-mixed")).await;
        assert_eq!(mixed.status, RunStatus::Failed, "{}", mixed.output);
        assert_eq!(sent(&server).await, 0);
        // Only meaningful when this machine's Ollama host is loopback.
        let base = crate::api::ollama::resolve_base_url_pub(None);
        if is_local(
            &AgentIdentity {
                agent_type: AgentType::Ollama,
                model: Some("qwen3:8b".into()),
            },
            &base,
        ) {
            let local = call(&state, &step("GET", "/users/me"), &ollama("room-local")).await;
            assert_eq!(local.status, RunStatus::Success, "{}", local.output);
            assert_eq!(sent(&server).await, 1);
        }
    }

    #[tokio::test]
    async fn blocked_refuses_even_a_person() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(
            &state,
            ApiAccessPolicy {
                access: ApiAccessRule::Blocked,
                endpoints: vec![],
            },
        )
        .await;
        let result = call(&state, &step("GET", "/users/me"), &ApiCaller::Human).await;
        assert_eq!(result.status, RunStatus::Failed);
        assert_eq!(sent(&server).await, 0);
    }

    // ─── Every request the call sends, not only the entry URL ───

    fn pages_blocked() -> ApiAccessPolicy {
        ApiAccessPolicy {
            access: ApiAccessRule::All,
            endpoints: vec![ApiEndpointAccess {
                method: "GET".into(),
                path: "/pages/{page_id}".into(),
                access: ApiAccessRule::Blocked,
            }],
        }
    }

    async fn requests_to(server: &MockServer, path: &str) -> usize {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path() == path)
            .count()
    }

    #[tokio::test]
    async fn a_redirect_to_a_blocked_or_undeclared_endpoint_is_never_followed() {
        use wiremock::matchers::path;
        for target_path in ["/v1/pages/secret", "/v1/blocks/x/children"] {
            let server = MockServer::start().await;
            Mock::given(path("/v1/users/me"))
                .respond_with(ResponseTemplate::new(302).insert_header("Location", target_path))
                .mount(&server)
                .await;
            Mock::given(path(target_path))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                .mount(&server)
                .await;
            let state = state_with_plugin(&server.uri()).await;
            set_policy(&state, pages_blocked()).await;
            let result = call(&state, &step("GET", "/users/me"), &ApiCaller::Human).await;
            assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
            assert!(
                result.output.contains("redirect refused"),
                "{}",
                result.output
            );
            assert_eq!(requests_to(&server, "/v1/users/me").await, 1);
            assert_eq!(
                requests_to(&server, target_path).await,
                0,
                "{target_path} reached"
            );
        }
    }

    #[tokio::test]
    async fn a_redirect_that_turns_a_post_into_a_get_is_re_decided() {
        use wiremock::matchers::path;
        let server = MockServer::start().await;
        Mock::given(path("/v1/search"))
            .and(wiremock::matchers::method("POST"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/v1/search"))
            .mount(&server)
            .await;
        Mock::given(path("/v1/search"))
            .and(wiremock::matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let state = state_with_plugin(&server.uri()).await;
        let mut p = pages_blocked();
        p.endpoints.push(ApiEndpointAccess {
            method: "POST".into(),
            path: "/search".into(),
            access: ApiAccessRule::All,
        });
        set_policy(&state, p).await;
        let result = call(&state, &step("POST", "/search"), &ApiCaller::Human).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        let gets = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "GET")
            .count();
        assert_eq!(gets, 0, "the GET the redirect asked for was sent");
    }

    #[tokio::test]
    async fn a_next_page_link_to_a_blocked_endpoint_is_never_fetched() {
        use wiremock::matchers::path;
        let server = MockServer::start().await;
        let next = format!("<{}/v1/pages/secret>; rel=\"next\"", server.uri());
        Mock::given(path("/v1/users/me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", next.as_str())
                    .set_body_json(serde_json::json!([{"id": 1}])),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v1/pages/secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"id": 2}])))
            .mount(&server)
            .await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, pages_blocked()).await;
        let mut paged = step("GET", "/users/me");
        paged.api_pagination = Some(PaginationSpec::LinkHeader {
            page_size_param: None,
            page_size: None,
            max_pages: Some(5),
        });
        let result = call(&state, &paged, &ApiCaller::Human).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert!(result.output.contains("Access policy"), "{}", result.output);
        assert_eq!(requests_to(&server, "/v1/pages/secret").await, 0);
    }

    // ─── Every agent that can read the result ───

    async fn local_only(state: &crate::AppState) {
        set_policy(
            state,
            ApiAccessPolicy {
                access: ApiAccessRule::LocalOnly,
                endpoints: vec![],
            },
        )
        .await;
    }

    fn local_ollama(discussion_ids: Vec<String>, run: Option<&str>) -> ApiCaller {
        ApiCaller::Agent {
            identity: Some(AgentIdentity {
                agent_type: AgentType::Ollama,
                model: Some("qwen3:8b".into()),
            }),
            discussion_ids,
            workflow_run_id: run.map(str::to_owned),
        }
    }

    fn agent_step(name: &str, agent: AgentType) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: StepType::Agent,
            agent: agent.clone(),
            agent_settings: (agent == AgentType::Ollama).then(|| AgentSettings {
                model: Some("qwen3:8b".into()),
                ..Default::default()
            }),
            ..WorkflowStep::default()
        }
    }

    /// Workflow `wf` with these steps, and run `run` pinned on it.
    async fn pinned_run(
        state: &crate::AppState,
        steps: Vec<WorkflowStep>,
        on_failure: Vec<WorkflowStep>,
    ) -> crate::models::Workflow {
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO workflows(id, name, trigger_json, steps_json, created_at, updated_at) \
                     VALUES ('wf', 'wf', '{\"type\":\"Manual\"}', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                let mut wf = crate::db::workflows::get_workflow(conn, "wf")?
                    .ok_or_else(|| anyhow::anyhow!("workflow missing"))?;
                wf.steps = steps;
                wf.on_failure = on_failure;
                crate::db::workflows::update_workflow(conn, &wf)?;
                let started = (chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339();
                conn.execute(
                    "INSERT INTO workflow_runs(id, workflow_id, started_at) VALUES ('run', 'wf', ?1)",
                    [&started],
                )?;
                let run = crate::db::workflows::get_run(conn, "run")?
                    .ok_or_else(|| anyhow::anyhow!("run missing"))?;
                let pinned = crate::workflows::run_pins::pin_or_load(conn, &wf, &run)?
                    .map_err(|reason| anyhow::anyhow!(reason))?;
                Ok(pinned)
            })
            .await
            .unwrap()
    }

    fn ollama_is_local_here() -> bool {
        is_local(
            &AgentIdentity {
                agent_type: AgentType::Ollama,
                model: Some("qwen3:8b".into()),
            },
            &crate::api::ollama::resolve_base_url_pub(None),
        )
    }

    #[tokio::test]
    async fn local_only_refuses_a_workflow_whose_rollback_runs_a_remote_agent() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        local_only(&state).await;
        let workflow = pinned_run(
            &state,
            vec![agent_step("main", AgentType::Ollama)],
            vec![agent_step("rollback", AgentType::ClaudeCode)],
        )
        .await;
        let in_run = call(
            &state,
            &step("GET", "/users/me"),
            &local_ollama(vec![], Some("run")),
        )
        .await;
        assert_eq!(in_run.status, RunStatus::Failed, "{}", in_run.output);
        assert!(in_run.output.contains("remote agent"), "{}", in_run.output);
        let own_step = call(
            &state,
            &step("GET", "/users/me"),
            &workflow_caller(&state, &workflow).await,
        )
        .await;
        assert_eq!(own_step.status, RunStatus::Failed, "{}", own_step.output);
        assert_eq!(sent(&server).await, 0);
    }

    #[tokio::test]
    async fn local_only_reads_the_run_s_pin_not_the_edited_workflow() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        local_only(&state).await;
        let pinned = pinned_run(
            &state,
            vec![
                agent_step("fetch", AgentType::Ollama),
                agent_step("review", AgentType::ClaudeCode),
            ],
            vec![],
        )
        .await;
        // The operator drops the remote step from the editable definition.
        let mut edited = pinned.clone();
        edited.steps.truncate(1);
        state
            .db
            .with_conn(move |conn| crate::db::workflows::update_workflow(conn, &edited))
            .await
            .unwrap();
        let fresh = crate::AppState::new_defaults(
            state.config.clone(),
            state.db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let result = call(
            &fresh,
            &step("GET", "/users/me"),
            &local_ollama(vec![], Some("run")),
        )
        .await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert!(result.output.contains("remote agent"), "{}", result.output);
        assert_eq!(sent(&server).await, 0);
    }

    #[tokio::test]
    async fn a_run_without_a_pin_is_an_unknown_audience() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        local_only(&state).await;
        let result = call(
            &state,
            &step("GET", "/users/me"),
            &local_ollama(vec![], Some("never-pinned")),
        )
        .await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert_eq!(sent(&server).await, 0);
        // The pin of an all-local run does let the call through.
        pinned_run(&state, vec![agent_step("fetch", AgentType::Ollama)], vec![]).await;
        if ollama_is_local_here() {
            let local = call(
                &state,
                &step("GET", "/users/me"),
                &local_ollama(vec![], Some("run")),
            )
            .await;
            assert_eq!(local.status, RunStatus::Success, "{}", local.output);
        }
    }

    // ─── Percent-encoded spellings of a blocked path ───

    fn me_blocked() -> ApiAccessPolicy {
        blocked_under("/users/{id}", "/users/me")
    }

    /// `open` allowed, `blocked` (written as given) blocked, the rest open.
    fn blocked_under(open: &str, blocked: &str) -> ApiAccessPolicy {
        ApiAccessPolicy {
            access: ApiAccessRule::All,
            endpoints: vec![
                ApiEndpointAccess {
                    method: "GET".into(),
                    path: open.into(),
                    access: ApiAccessRule::All,
                },
                ApiEndpointAccess {
                    method: "GET".into(),
                    path: blocked.into(),
                    access: ApiAccessRule::Blocked,
                },
            ],
        }
    }

    async fn refused_with_no_request(policy: &ApiAccessPolicy, step: WorkflowStep) {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, policy.clone()).await;
        let label = step.api_endpoint_path.clone().unwrap_or_default();
        let result = call(&state, &step, &ApiCaller::Human).await;
        assert_eq!(
            result.status,
            RunStatus::Failed,
            "{label}: {}",
            result.output
        );
        assert!(
            result.output.contains("blocked"),
            "{label}: {}",
            result.output
        );
        assert_eq!(sent(&server).await, 0, "{label} reached the API");
    }

    #[tokio::test]
    async fn a_rule_with_an_encoded_literal_blocks_every_spelling_and_route() {
        use wiremock::matchers::path;
        // Stored as written, and as the settings route would store it.
        let raw = blocked_under("/users/{id}", "/users/%6De");
        let saved = normalize_policy(raw.clone()).unwrap();
        assert_eq!(saved.endpoints[1].path, "/users/me");
        for policy in [&raw, &saved] {
            for spelling in ["/users/me", "/users/%6De", "/users/%6d%65"] {
                refused_with_no_request(policy, step("GET", spelling)).await;
            }
            // Through Location and through Link: only the entry request.
            for via_link in [false, true] {
                let server = MockServer::start().await;
                let target = format!("{}/v1/users/%6De", server.uri());
                let response = if via_link {
                    ResponseTemplate::new(200)
                        .insert_header("Link", format!("<{target}>; rel=\"next\"").as_str())
                        .set_body_json(serde_json::json!([{"id": 1}]))
                } else {
                    ResponseTemplate::new(302).insert_header("Location", target.as_str())
                };
                Mock::given(path("/v1/users/42"))
                    .respond_with(response)
                    .mount(&server)
                    .await;
                Mock::given(any())
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
                    .with_priority(10)
                    .mount(&server)
                    .await;
                let state = state_with_plugin(&server.uri()).await;
                set_policy(&state, policy.clone()).await;
                let mut entry = step("GET", "/users/42");
                if via_link {
                    entry.api_pagination = Some(PaginationSpec::LinkHeader {
                        page_size_param: None,
                        page_size: None,
                        max_pages: Some(3),
                    });
                }
                let result = call(&state, &entry, &ApiCaller::Human).await;
                assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
                assert_eq!(sent(&server).await, 1, "the blocked endpoint was reached");
            }
        }
    }

    #[tokio::test]
    async fn escapes_differing_only_in_case_are_the_same_endpoint() {
        let policy = blocked_under("/pages/{id}", "/pages/a%3ab");
        for spelling in ["/pages/a%3Ab", "/pages/a%3ab"] {
            refused_with_no_request(&policy, step("GET", spelling)).await;
        }
    }

    #[tokio::test]
    async fn ascii_characters_the_url_escapes_are_matched_escaped() {
        // Raw in the rule, raw or escaped in the request.
        for (raw, escaped) in [
            (" ", "%20"),
            ("\"", "%22"),
            ("<", "%3C"),
            (">", "%3E"),
            ("`", "%60"),
        ] {
            let rule = blocked_under("/files/{name}", &format!("/files/Annual{raw}Report.pdf"));
            let saved = normalize_policy(rule.clone()).unwrap();
            assert_eq!(
                saved.endpoints[1].path,
                format!("/files/Annual{escaped}Report.pdf")
            );
            for policy in [&rule, &saved] {
                for spelling in [raw, escaped, &escaped.to_ascii_lowercase()] {
                    let path = format!("/files/Annual{spelling}Report.pdf");
                    refused_with_no_request(policy, step("GET", &path)).await;
                }
            }
        }
        // Braces cannot be literal in a template; escaped, they are.
        let rule = blocked_under("/files/{name}", "/files/a%7bb%7d");
        let saved = normalize_policy(rule.clone()).unwrap();
        assert_eq!(saved.endpoints[1].path, "/files/a%7Bb%7D");
        for policy in [&rule, &saved] {
            for spelling in ["/files/a%7Bb%7D", "/files/a%7bb%7d"] {
                refused_with_no_request(policy, step("GET", spelling)).await;
            }
        }
    }

    #[tokio::test]
    async fn a_stored_rule_with_a_control_character_fails_closed() {
        for c in ['\t', '\r', '\n'] {
            // Stored before the save-time check: never normalised.
            let legacy = blocked_under("/files/{name}", &format!("/files/sec{c}ret"));
            for path in ["/files/secret", "/files/other"] {
                let server = server().await;
                let state = state_with_plugin(&server.uri()).await;
                set_policy(&state, legacy.clone()).await;
                let result = call(&state, &step("GET", path), &ApiCaller::Human).await;
                assert_eq!(
                    result.status,
                    RunStatus::Failed,
                    "{c:?} {path}: {}",
                    result.output
                );
                assert!(
                    result.output.contains("cannot compare"),
                    "{}",
                    result.output
                );
                assert_eq!(sent(&server).await, 0, "{c:?} {path} reached the API");
            }
        }
    }

    #[tokio::test]
    async fn unicode_spaces_at_a_segment_end_are_kept_and_matched() {
        for (c, escaped) in [
            ('\u{a0}', "%C2%A0"),
            ('\u{2000}', "%E2%80%80"),
            ('\u{3000}', "%E3%80%80"),
        ] {
            let rule = blocked_under("/files/{name}", &format!("/files/report{c}"));
            let saved = normalize_policy(rule.clone()).unwrap();
            for policy in [&rule, &saved] {
                for spelling in [
                    c.to_string(),
                    escaped.to_string(),
                    escaped.to_ascii_lowercase(),
                ] {
                    refused_with_no_request(
                        policy,
                        step("GET", &format!("/files/report{spelling}")),
                    )
                    .await;
                }
            }
        }
    }

    #[tokio::test]
    async fn a_utf8_segment_is_matched_intact() {
        let policy = blocked_under("/pages/{id}", "/pages/café");
        for spelling in ["/pages/café", "/pages/caf%C3%A9", "/pages/caf%c3%a9"] {
            refused_with_no_request(&policy, step("GET", spelling)).await;
        }
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, policy).await;
        let other = call(&state, &step("GET", "/pages/cafe"), &ApiCaller::Human).await;
        assert_eq!(other.status, RunStatus::Success, "{}", other.output);
    }

    #[tokio::test]
    async fn an_encoded_spelling_of_a_blocked_path_is_refused() {
        for path in ["/users/me", "/users/%6De", "/users/%6d%65", "/users/m%65"] {
            let server = server().await;
            let state = state_with_plugin(&server.uri()).await;
            set_policy(&state, me_blocked()).await;
            let result = call(&state, &step("GET", path), &ApiCaller::Human).await;
            assert_eq!(
                result.status,
                RunStatus::Failed,
                "{path}: {}",
                result.output
            );
            assert!(
                result.output.contains("blocked"),
                "{path}: {}",
                result.output
            );
            assert_eq!(sent(&server).await, 0, "{path} reached the API");
        }
        // Another user id still goes through.
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, me_blocked()).await;
        let ok = call(&state, &step("GET", "/users/42"), &ApiCaller::Human).await;
        assert_eq!(ok.status, RunStatus::Success, "{}", ok.output);
    }

    #[tokio::test]
    async fn an_encoded_blocked_path_behind_a_redirect_or_next_link_is_never_sent() {
        use wiremock::matchers::path;
        // Redirect from an allowed user to the encoded blocked one.
        let server = MockServer::start().await;
        Mock::given(path("/v1/users/42"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/v1/users/%6De"))
            .mount(&server)
            .await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .with_priority(10)
            .mount(&server)
            .await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, me_blocked()).await;
        let result = call(&state, &step("GET", "/users/42"), &ApiCaller::Human).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert_eq!(sent(&server).await, 1, "only the entry request may be sent");

        // A next-page link to the encoded blocked one.
        let server = MockServer::start().await;
        let next = format!("<{}/v1/users/%6De>; rel=\"next\"", server.uri());
        Mock::given(path("/v1/users/42"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Link", next.as_str())
                    .set_body_json(serde_json::json!([{"id": 1}])),
            )
            .mount(&server)
            .await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .with_priority(10)
            .mount(&server)
            .await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, me_blocked()).await;
        let mut paged = step("GET", "/users/42");
        paged.api_pagination = Some(PaginationSpec::LinkHeader {
            page_size_param: None,
            page_size: None,
            max_pages: Some(3),
        });
        let result = call(&state, &paged, &ApiCaller::Human).await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert_eq!(sent(&server).await, 1, "only the entry request may be sent");
    }

    #[tokio::test]
    async fn local_only_refuses_a_room_with_a_joined_cli() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        local_only(&state).await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO discussions(id, title, agent, created_at, updated_at) \
                     VALUES ('room', 'room', 'Ollama', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    [],
                )?;
                conn.execute("UPDATE discussions SET model = 'qwen3:8b', participants_json = '[\"Ollama\"]' WHERE id = 'room'", [])?;
                crate::db::discussion_sessions::create_session(
                    conn,
                    "room",
                    "Ollama",
                    Some("host-cli"),
                    "peer",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        // Even a session declaring Ollama is unattested: unknown, so remote.
        let result = call(
            &state,
            &step("GET", "/users/me"),
            &local_ollama(vec!["room".into()], None),
        )
        .await;
        assert_eq!(result.status, RunStatus::Failed, "{}", result.output);
        assert!(result.output.contains("remote agent"), "{}", result.output);
        assert_eq!(sent(&server).await, 0);
    }

    // ─── The model a native launch really runs ───

    async fn native_api_call(
        state: &crate::AppState,
        bound: Option<&str>,
    ) -> crate::agents::tools::ToolOutcome {
        use crate::agents::tools::ToolCall;
        let executor = crate::api::agent_tools::KronnToolExecutor::arc(
            state.clone(),
            None,
            AgentType::Ollama,
            None,
            None,
        );
        if let Some(model) = bound {
            executor.bind_launch_identity(AgentIdentity {
                agent_type: AgentType::Ollama,
                model: Some(model.into()),
            });
        }
        // Defaults moving after launch must not change the decision.
        state.config.write().await.agents.model_tiers.ollama.default = Some("model-a".into());
        executor
            .execute(&ToolCall {
                id: "c".into(),
                name: "api_call".into(),
                arguments: serde_json::json!({
                    "api_plugin_slug": "custom-notion",
                    "api_config_id": "cfg-notion",
                    "endpoint_path": "/users/me",
                }),
            })
            .await
    }

    #[tokio::test]
    async fn a_native_agent_is_decided_on_the_model_bound_at_launch() {
        // A loopback base URL: the allowed call stops at the public-IP guard,
        // never on the network.
        let state = state_with_plugin("http://127.0.0.1:9").await;
        set_policy(
            &state,
            ApiAccessPolicy {
                access: ApiAccessRule::Agents {
                    agents: vec![ApiAccessSubject {
                        agent: AgentType::Ollama,
                        model: Some("model-a".into()),
                    }],
                },
                endpoints: vec![],
            },
        )
        .await;
        let text = |outcome: crate::agents::tools::ToolOutcome| {
            serde_json::to_string(&outcome.content).unwrap_or_default()
        };
        let ran_b = text(native_api_call(&state, Some("model-b")).await);
        assert!(ran_b.contains("reserved to Ollama (model-a)"), "{ran_b}");
        let unbound = text(native_api_call(&state, None).await);
        assert!(unbound.contains("Access policy"), "{unbound}");
        let ran_a = text(native_api_call(&state, Some("model-a")).await);
        assert!(!ran_a.contains("Access policy"), "{ran_a}");
    }

    // ─── A Watch poll passes the same gate (KT-1026 × KT-1099) ───

    async fn watch(
        state: &crate::AppState,
        workflow: &crate::models::Workflow,
    ) -> Result<crate::workflows::api_call_executor::WatchPollResponse, String> {
        crate::workflows::api_call_executor::execute_watch_poll(
            state,
            SecurityPolicy::allow_loopback_for_tests(),
            crate::workflows::api_call_executor::WatchPollRequest {
                source: &step("GET", "/users/me"),
                workflow,
                project_id: None,
                if_none_match: None,
                if_modified_since: None,
                max_body_bytes: 1 << 20,
                timeout: std::time::Duration::from_secs(5),
            },
        )
        .await
    }

    #[tokio::test]
    async fn a_watch_whose_audience_may_not_call_the_api_sends_nothing() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, codex_only()).await;
        let workflow = pinned_run(
            &state,
            vec![agent_step("main", AgentType::ClaudeCode)],
            vec![],
        )
        .await;
        let refused = watch(&state, &workflow).await.expect_err("must be refused");
        assert!(refused.contains("Access policy"), "{refused}");
        assert!(!refused.contains(TOKEN), "{refused}");
        assert_eq!(sent(&server).await, 0);
    }

    #[tokio::test]
    async fn a_watch_redirect_to_a_blocked_endpoint_is_refused_at_the_hop() {
        use wiremock::matchers::path;
        let server = MockServer::start().await;
        Mock::given(path("/v1/users/me"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/v1/pages/secret"))
            .mount(&server)
            .await;
        Mock::given(path("/v1/pages/secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, pages_blocked()).await;
        let workflow = pinned_run(&state, vec![], vec![]).await;
        let refused = watch(&state, &workflow)
            .await
            .expect_err("hop must be refused");
        assert!(refused.contains("redirect refused"), "{refused}");
        assert_eq!(requests_to(&server, "/v1/users/me").await, 1);
        assert_eq!(requests_to(&server, "/v1/pages/secret").await, 0);
    }

    #[tokio::test]
    async fn an_allowed_watch_still_polls() {
        let server = server().await;
        let state = state_with_plugin(&server.uri()).await;
        set_policy(&state, pages_blocked()).await;
        let workflow = pinned_run(&state, vec![], vec![]).await;
        let response = watch(&state, &workflow).await.expect("allowed poll");
        assert_eq!(response.status, 200);
        assert_eq!(sent(&server).await, 1);
    }
}
