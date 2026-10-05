use super::*;
use serde_json::json;

/// Routes that return or move secrets, or run commands (design note §3 rows
/// 3 and 4): a bridge token is never accepted there.
pub(crate) const SECRET_CLASS: &[(&str, &str)] = &[
    ("POST", "/api/mcps/configs/{id}/reveal"),
    ("POST", "/api/external-api/connections/{id}/reveal"),
    ("POST", "/api/execution-context/{run_kind}/{run_id}/reveal"),
    ("POST", "/api/mcps/bundles/export"),
    ("GET", "/api/config/export"),
    ("POST", "/api/config/import"),
    ("POST", "/api/config/recovery/set"),
    ("POST", "/api/config/sync-agent-tokens"),
    ("POST", "/api/config/discover-keys"),
    ("POST", "/api/config/auth-token/regenerate"),
    ("POST", "/api/projects/{id}/exec"),
    ("POST", "/api/discussions/{id}/exec"),
    ("POST", "/api/human-credentials/enrol"),
    ("POST", "/api/human-credentials/admin/rotate"),
];

fn scope(discussions: &[&str]) -> BridgeScope {
    BridgeScope {
        discussion_ids: discussions.iter().map(|id| id.to_string()).collect(),
        ..Default::default()
    }
}

fn grant(discussions: &[&str]) -> BridgeGrant {
    BridgeGrant {
        id: "test".into(),
        scope: scope(discussions),
        adopted: Mutex::new(Vec::new()),
        bound: Mutex::new(None),
        expires_at: Instant::now() + MAX_LIFETIME,
    }
}

fn in_project(project: &str) -> Residence {
    Residence::Projects(HashSet::from([project.to_string()]))
}

/// Fixture world: `a*` live in `p1`, `b*` in `p2`, `m*` in both, `g*` are
/// project-less, `all*` serve every project, `gen*` are General-only.
fn world(_: Kind, id: &str) -> Result<Option<Residence>, Refusal> {
    Ok(if id.starts_with("all") {
        Some(Residence::AllProjects)
    } else if id.starts_with("gen") {
        Some(Residence::General)
    } else {
        match id.chars().next() {
            Some('a') => Some(in_project("p1")),
            Some('b') => Some(in_project("p2")),
            Some('m') => Some(Residence::Projects(HashSet::from([
                "p1".to_string(),
                "p2".to_string(),
            ]))),
            Some('g') => Some(Residence::Global),
            _ => None,
        }
    })
}

fn target(kind: Kind, id: &str) -> NamedId {
    NamedId::new(kind, id, Role::Target)
}

fn reference(kind: Kind, id: &str) -> NamedId {
    NamedId::new(kind, id, Role::Ref)
}

fn route(method: &str, pattern: &str) -> &'static BridgeRoute {
    route_for(method, pattern).unwrap_or_else(|| panic!("{method} {pattern} is not listed"))
}

fn has(ids: &[NamedId], kind: Kind, id: &str) -> bool {
    ids.iter().any(|named| named.kind == kind && named.id == id)
}

// ─── Token lifecycle ────────────────────────────────────────────────────────

#[test]
fn a_minted_token_is_random_prefixed_and_dies_with_its_guard() {
    let first = mint(scope(&["d1"])).unwrap();
    let second = mint(scope(&["d1"])).unwrap();
    assert_ne!(first.value(), second.value());
    assert!(first.value().starts_with(TOKEN_PREFIX));
    assert_eq!(
        first.value().len(),
        TOKEN_PREFIX.len() + 64,
        "256 bits in hex"
    );
    assert!(
        !format!("{first:?}").contains(first.value()),
        "Debug never shows it"
    );
    let value = first.value().to_owned();
    let grant = lookup(&value).expect("live while the guard is held");
    assert_eq!(grant.scope.discussion_ids, vec!["d1".to_string()]);
    drop(first);
    assert!(lookup(&value).is_none(), "revoked when the launch ends");
    assert!(
        lookup(second.value()).is_some(),
        "another launch is unaffected"
    );
    assert!(lookup("not-a-bridge-token").is_none());
}

/// E-01 — a launch that owns nothing still gets a token.
#[test]
fn a_launch_that_owns_nothing_still_gets_a_token() {
    let guard = mint(BridgeScope::default()).expect("always minted");
    let grant = lookup(guard.value()).unwrap();
    assert!(grant.scope.owns_nothing());
}

/// E-02 — a grant past its deadline is refused.
#[test]
fn a_token_past_its_lifetime_is_dead() {
    let guard = mint_with_lifetime(scope(&["d1"]), Duration::ZERO).unwrap();
    assert!(lookup(guard.value()).is_none());
    let live = mint_with_lifetime(scope(&["d1"]), Duration::from_secs(60)).unwrap();
    assert!(lookup(live.value()).is_some());
}

/// B-17 / E-04 — the bound project is frozen on first use.
#[test]
fn the_bound_project_is_frozen_on_first_use() {
    let grant = grant(&["a-room"]);
    assert_eq!(grant.freeze(Some("p1".into())), Ok(Some("p1".into())));
    assert_eq!(grant.freeze(Some("p1".into())), Ok(Some("p1".into())));
    assert!(grant.freeze(None).is_err(), "moving to no project kills it");
    assert!(grant.freeze(Some("p2".into())).is_err());
}

/// E-03 — the operator token comparison.
#[test]
fn the_operator_token_comparison_is_exact() {
    assert!(operator_token_matches(Some("secret-token"), "secret-token"));
    assert!(!operator_token_matches(
        Some("secret-token"),
        "secret-tokeN"
    ));
    assert!(!operator_token_matches(Some("secret-token"), "secret"));
    assert!(!operator_token_matches(Some("secret-token"), ""));
    assert!(!operator_token_matches(None, "anything"));
}

/// E-03 — both the HTTP and the WebSocket paths go through the helper.
#[test]
fn both_auth_paths_compare_the_operator_token_in_constant_time() {
    for (name, source) in [
        ("lib.rs", include_str!("../lib.rs")),
        ("api/ws.rs", include_str!("../api/ws.rs")),
    ] {
        assert!(
            source.contains("operator_token_matches("),
            "{name} must compare with operator_token_matches"
        );
        for raw in [
            "bearer == expected",
            "== Some(expected)",
            "Some(credential.as_str())",
        ] {
            assert!(!source.contains(raw), "{name} still compares with `{raw}`");
        }
    }
}

// ─── Route list ─────────────────────────────────────────────────────────────

#[test]
fn the_positive_list_has_no_secret_class_route_and_no_duplicate() {
    for (method, pattern) in SECRET_CLASS {
        assert!(
            route_for(method, pattern).is_none(),
            "{method} {pattern} must never accept a bridge token"
        );
    }
    let mut seen = HashSet::new();
    for route in BRIDGE_ROUTES {
        assert!(
            seen.insert((route.method, route.pattern)),
            "duplicate {} {}",
            route.method,
            route.pattern
        );
        for (param, _) in route.params {
            assert!(
                route.pattern.contains(&format!("{{{param}}}")),
                "{} names a parameter it does not have: {param}",
                route.pattern
            );
        }
    }
}

/// Each listed pattern is a real route: a typo would silently refuse a call.
#[test]
fn every_listed_pattern_is_registered_in_the_router() {
    let router = include_str!("../lib.rs");
    for route in BRIDGE_ROUTES {
        assert!(
            router.contains(&format!("\"{}\"", route.pattern)),
            "{} is not a route in lib.rs",
            route.pattern
        );
    }
}

/// B-09 — the shared agent library is read, never written, by a token.
#[test]
fn the_agent_library_is_read_only_for_a_token() {
    for library in ["/api/skills", "/api/profiles", "/api/directives"] {
        assert_eq!(route("GET", library).rule, Rule::Read);
        assert_eq!(route("POST", library).rule, Rule::Denied);
        let item = format!("{library}/{{id}}");
        assert_eq!(route("PUT", &item).rule, Rule::Denied);
        assert_eq!(route("DELETE", &item).rule, Rule::Denied);
    }
    let grant = grant(&["a-room"]);
    assert!(authorize(
        &grant,
        route("PUT", "/api/skills/{id}"),
        Some("p1"),
        &[],
        world
    )
    .is_err());
}

// ─── Id collection ──────────────────────────────────────────────────────────

#[test]
fn ids_are_read_from_path_query_and_body_at_any_depth() {
    let route = route("GET", "/api/workflows/{id}/runs/{run_id}");
    let ids = collect_ids(
        route,
        &[
            ("id".into(), "wf-1".into()),
            ("run_id".into(), "run-1".into()),
        ],
        Some("project_id=p%2D1&unrelated=x&disc_id=d+2"),
        Some(&json!({
            "discussion_id": "d-body",
            "project_ids": ["p-a", "p-b", 7],
            "session_id": "ignored",
            "nested": {"project_id": "p-nested", "deeper": [{"qp_id": "qp-deep"}]}
        })),
    )
    .unwrap();
    for (kind, id) in [
        (Kind::Workflow, "wf-1"),
        (Kind::Run, "run-1"),
        (Kind::Project, "p-1"),
        (Kind::Discussion, "d 2"),
        (Kind::Discussion, "d-body"),
        (Kind::Project, "p-a"),
        (Kind::Project, "p-b"),
        (Kind::Project, "p-nested"),
        (Kind::QuickPrompt, "qp-deep"),
    ] {
        assert!(has(&ids, kind, id), "{kind:?} {id} missing from {ids:?}");
    }
    assert_eq!(ids.len(), 9, "{ids:?}");
}

/// B-01 / B-02 / F-01 — nested step refs and a workflow scope are collected.
#[test]
fn nested_step_references_and_scopes_are_collected() {
    let route = route("PUT", "/api/workflows/{id}");
    let ids = collect_ids(
        route,
        &[("id".into(), "a-wf".into())],
        None,
        Some(&json!({
            "steps": [
                {"sub_workflow_id": "X", "room_id": "room-x",
                 "batch_quick_prompt_id": "qp-x", "batch_chain_prompt_ids": ["qp-y"],
                 "quick_api_id": "qa-x", "api_config_id": "cfg-x", "page_id": "page-x",
                 "connection_id": "conn-x", "mcp_config_ids": ["mcp-x"],
                 "on_failure": [{"quick_prompt_id": "qp-z"}]}
            ],
            "project_scope": {"type": "Projects", "project_ids": ["other"]}
        })),
    )
    .unwrap();
    for (kind, id) in [
        (Kind::Workflow, "X"),
        (Kind::Discussion, "room-x"),
        (Kind::QuickPrompt, "qp-x"),
        (Kind::QuickPrompt, "qp-y"),
        (Kind::QuickApi, "qa-x"),
        (Kind::McpConfig, "cfg-x"),
        (Kind::Page, "page-x"),
        (Kind::Connection, "conn-x"),
        (Kind::McpConfig, "mcp-x"),
        (Kind::QuickPrompt, "qp-z"),
        (Kind::Project, "other"),
    ] {
        assert!(has(&ids, kind, id), "{kind:?} {id} missing from {ids:?}");
    }
}

/// B-16 — an import's workflow is parsed and walked; its own project fields
/// (replaced at import) and the resources it bundles are not looked up.
#[test]
fn an_import_s_content_is_walked() {
    let route = route("POST", "/api/workflows/import");
    let envelope = json!({
        "kind": "kronn.workflow",
        "workflow": {"id": "w-old", "project_id": "exporter-project",
            "steps": [{"sub_workflow_id": "b-child"}, {"quick_prompt_id": "qp-bundled"}]},
        "referenced_quick_prompts": [{"id": "qp-bundled", "project_id": "exporter-project"}]
    });
    let body = json!({"content": envelope.to_string(), "project_id": "p1"});
    let ids = collect_ids(route, &[], None, Some(&body)).unwrap();
    assert!(has(&ids, Kind::Workflow, "b-child"));
    assert!(
        !has(&ids, Kind::QuickPrompt, "qp-bundled"),
        "bundled: {ids:?}"
    );
    assert!(!has(&ids, Kind::Project, "exporter-project"), "{ids:?}");
    assert!(has(&ids, Kind::Project, "p1"));
    let not_json = json!({"content": "not json"});
    assert!(collect_ids(route, &[], None, Some(&not_json)).is_err());
}

/// B-08 / B-18 / B-11 — task parents, task references and offers.
#[test]
fn task_parents_references_and_offers_are_collected() {
    let create = route("POST", "/api/planning/tasks");
    let ids = collect_ids(
        create,
        &[],
        None,
        Some(&json!({"title": "t", "parent_id": "b-task"})),
    )
    .unwrap();
    assert!(ids.contains(&target(Kind::Task, "b-task")), "{ids:?}");
    let disc = route("POST", "/api/disc/create");
    let ids = collect_ids(disc, &[], None, Some(&json!({"parent_id": "msg-1"}))).unwrap();
    assert!(
        ids.is_empty(),
        "parent_id is a task only on planning routes: {ids:?}"
    );
    let prepare = route("POST", "/api/orchestration/tool/prepare");
    let ids = collect_ids(
        prepare,
        &[],
        None,
        Some(&json!({"task_reference": "KT-12"})),
    )
    .unwrap();
    assert!(has(&ids, Kind::Task, "KT-12"));
    let offer = route("POST", "/api/orchestration/accept-offer");
    let ids = collect_ids(offer, &[], None, Some(&json!({"offer_id": "o-1"}))).unwrap();
    assert!(ids.contains(&target(Kind::Offer, "o-1")));
}

// ─── Body preparation ───────────────────────────────────────────────────────

/// B-07 — a created resource lands in the token's project.
#[test]
fn creations_are_forced_into_the_bound_project() {
    for (pattern, field) in CREATE_ROUTES {
        let route = route("POST", pattern);
        let body = |extra: serde_json::Value| {
            let mut body = json!({"name": "x", "content": "{}"});
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            body
        };
        let prepared = prepare_body(route, Some("p1"), Some(&body(json!({}))))
            .unwrap()
            .expect("project added");
        if *field == "project_ids" {
            assert_eq!(prepared[field], json!(["p1"]), "{pattern}");
        } else {
            assert_eq!(prepared[field], "p1", "{pattern}");
        }
        let null = prepare_body(route, Some("p1"), Some(&body(json!({*field: null}))))
            .unwrap()
            .expect("null replaced");
        assert_ne!(null[field], serde_json::Value::Null, "{pattern}");
    }
    let qa = route("POST", "/api/quick-apis");
    assert!(prepare_body(qa, None, Some(&json!({"name": "x"}))).is_err());
    let disc = route("POST", "/api/disc/create");
    assert!(prepare_body(disc, None, Some(&json!({"title": "x"}))).is_ok());
}

/// B-07 / B-17 — an update cannot move a resource out of the project.
#[test]
fn updates_cannot_move_a_resource_out_of_the_project() {
    for (method, pattern) in [
        ("PUT", "/api/workflows/{id}"),
        ("PATCH", "/api/discussions/{id}"),
        ("PUT", "/api/quick-apis/{id}"),
    ] {
        let route = route(method, pattern);
        assert!(prepare_body(route, Some("p1"), Some(&json!({"project_id": null}))).is_err());
        assert!(prepare_body(route, Some("p1"), Some(&json!({"project_id": "p2"}))).is_err());
    }
    let task = route("PATCH", "/api/planning/tasks/{id}");
    assert!(prepare_body(task, Some("p1"), Some(&json!({"project_ids": []}))).is_err());
    // A Quick API update keeps the project the handler would otherwise reset.
    let qa = route("PUT", "/api/quick-apis/{id}");
    let kept = prepare_body(qa, Some("p1"), Some(&json!({"name": "x"})))
        .unwrap()
        .unwrap();
    assert_eq!(kept["project_id"], "p1");
}

/// B-03 / B-16 — a workflow may be scoped to the token's project only.
#[test]
fn workflow_scopes_stay_within_the_bound_project() {
    let put = route("PUT", "/api/workflows/{id}");
    let all = json!({"project_scope": {"type": "All"}});
    let other = json!({"project_scope": {"type": "Projects", "project_ids": ["p2"]}});
    let wider = json!({"project_scope": {"type": "Projects", "project_ids": ["p1", "p2"]}});
    let own = json!({"project_scope": {"type": "Projects", "project_ids": ["p1"]}});
    let none = json!({"project_scope": null});
    assert!(prepare_body(put, Some("p1"), Some(&all)).is_err());
    assert!(prepare_body(put, Some("p1"), Some(&other)).is_err());
    assert!(prepare_body(put, Some("p1"), Some(&wider)).is_err());
    assert!(prepare_body(put, Some("p1"), Some(&own)).is_ok());
    assert!(prepare_body(put, Some("p1"), Some(&none)).is_ok());
    let import = route("POST", "/api/workflows/import");
    let content = json!({"workflow": {"project_scope": {"type": "All"}}}).to_string();
    assert!(prepare_body(import, Some("p1"), Some(&json!({"content": content}))).is_err());
}

#[test]
fn a_trigger_naming_no_project_gets_the_bound_one() {
    for pattern in RUNS_FOR_A_PROJECT {
        let route = route("POST", pattern);
        let body = prepare_body(route, Some("p1"), Some(&json!({"workflow_id": "w"})))
            .unwrap()
            .unwrap();
        assert_eq!(body["project_id"], "p1", "{pattern}");
    }
    let trigger = route("POST", "/api/mcp/workflow-trigger");
    assert!(prepare_body(trigger, Some("p1"), Some(&json!({"project_id": "p2"}))).is_err());
    let append = route("POST", "/api/disc/append");
    assert!(prepare_body(append, Some("p1"), Some(&json!({})))
        .unwrap()
        .is_none());
}

// ─── Authorization ──────────────────────────────────────────────────────────

#[test]
fn a_discussion_token_writes_only_its_own_room() {
    let grant = grant(&["a-room"]);
    let append = route("POST", "/api/disc/append");
    let own = [reference(Kind::Discussion, "a-room")];
    assert!(authorize(&grant, append, Some("p1"), &own, world).is_ok());
    for other in ["a-other-room", "b-room", "g-room"] {
        let ids = [reference(Kind::Discussion, other)];
        assert!(
            authorize(&grant, append, Some("p1"), &ids, world).is_err(),
            "append to {other} must be refused"
        );
    }
}

#[test]
fn a_discussion_the_launch_created_is_its_own() {
    let grant = grant(&["a-room"]);
    let append = route("POST", "/api/disc/append");
    let child = [reference(Kind::Discussion, "a-child")];
    assert!(authorize(&grant, append, Some("p1"), &child, world).is_err());
    grant.adopt_discussion("a-child");
    assert!(authorize(&grant, append, Some("p1"), &child, world).is_ok());
}

#[test]
fn reads_stay_inside_the_token_s_project() {
    let grant = grant(&["a-room"]);
    let meta = route("GET", "/api/discussions/{id}/meta");
    assert!(authorize(
        &grant,
        meta,
        Some("p1"),
        &[target(Kind::Discussion, "a-other")],
        world
    )
    .is_ok());
    assert!(authorize(
        &grant,
        meta,
        Some("p1"),
        &[target(Kind::Discussion, "b-room")],
        world
    )
    .is_err());
    let list = route("GET", "/api/discussions");
    assert!(authorize(
        &grant,
        list,
        Some("p1"),
        &[reference(Kind::Project, "p2")],
        world
    )
    .is_err());
    assert!(authorize(
        &grant,
        list,
        Some("p1"),
        &[reference(Kind::Project, "p1")],
        world
    )
    .is_ok());
    assert!(authorize(&grant, list, None, &[reference(Kind::Project, "p1")], world).is_err());
    // An id that resolves to nothing is refused, never let through.
    assert!(authorize(
        &grant,
        list,
        Some("p1"),
        &[reference(Kind::Task, "zzz")],
        world
    )
    .is_err());
}

/// C-01 — a project-less discussion is private to its own launches.
#[test]
fn a_general_discussion_is_private_to_its_launches() {
    let grant = grant(&["g-own"]);
    let meta = route("GET", "/api/discussions/{id}/meta");
    assert!(authorize(
        &grant,
        meta,
        Some("p1"),
        &[target(Kind::Discussion, "g-other")],
        world
    )
    .is_err());
    assert!(authorize(
        &grant,
        meta,
        None,
        &[target(Kind::Discussion, "g-other")],
        world
    )
    .is_err());
    assert!(authorize(
        &grant,
        meta,
        None,
        &[target(Kind::Discussion, "g-own")],
        world
    )
    .is_ok());
}

/// B-04 / F-02 — shared resources are read, never written.
#[test]
fn shared_resources_are_read_never_written() {
    let grant = grant(&["a-room"]);
    for (method, pattern, kind) in [
        ("PUT", "/api/workflows/{id}", Kind::Workflow),
        ("PUT", "/api/quick-prompts/{id}", Kind::QuickPrompt),
        ("DELETE", "/api/quick-prompts/{id}", Kind::QuickPrompt),
        ("PUT", "/api/quick-apis/{id}", Kind::QuickApi),
        ("PUT", "/api/quick-execs/{id}", Kind::QuickExec),
        ("PATCH", "/api/planning/tasks/{id}", Kind::Task),
    ] {
        let write = route(method, pattern);
        for shared in ["g-res", "all-res", "m-res"] {
            assert!(
                authorize(&grant, write, Some("p1"), &[target(kind, shared)], world).is_err(),
                "{method} {pattern} on {shared}"
            );
        }
        assert!(authorize(&grant, write, Some("p1"), &[target(kind, "a-res")], world).is_ok());
    }
    let read = route("GET", "/api/workflows/{id}");
    for shared in ["g-res", "all-res", "m-res"] {
        assert!(authorize(
            &grant,
            read,
            Some("p1"),
            &[target(Kind::Workflow, shared)],
            world
        )
        .is_ok());
    }
    // Referencing a shared resource from an own one is a read.
    let put = route("PUT", "/api/workflows/{id}");
    let ids = [
        target(Kind::Workflow, "a-wf"),
        reference(Kind::QuickPrompt, "g-qp"),
    ];
    assert!(authorize(&grant, put, Some("p1"), &ids, world).is_ok());
    let foreign = [
        target(Kind::Workflow, "a-wf"),
        reference(Kind::Workflow, "b-child"),
    ];
    assert!(authorize(&grant, put, Some("p1"), &foreign, world).is_err());
}

/// B-05 / B-06 / F-03 — effects on shared resources only where the handler
/// runs them for the bound project, and never for a project-less token on a
/// resource serving every project.
#[test]
fn effects_on_shared_resources_follow_the_handler() {
    let grant = grant(&["a-room"]);
    let trigger = route("POST", "/api/mcp/workflow-trigger");
    assert!(authorize(
        &grant,
        trigger,
        Some("p1"),
        &[target(Kind::Workflow, "a-wf")],
        world
    )
    .is_ok());
    assert!(authorize(
        &grant,
        trigger,
        Some("p1"),
        &[target(Kind::Workflow, "b-wf")],
        world
    )
    .is_err());
    assert!(authorize(
        &grant,
        trigger,
        Some("p1"),
        &[target(Kind::Workflow, "g-wf")],
        world
    )
    .is_ok());
    assert!(authorize(
        &grant,
        trigger,
        None,
        &[target(Kind::Workflow, "all-wf")],
        world
    )
    .is_err());
    let resume = route("POST", "/api/workflow-runs/{run_id}/resume");
    assert!(authorize(
        &grant,
        resume,
        Some("p1"),
        &[target(Kind::Run, "g-run")],
        world
    )
    .is_err());
    let qa = route("POST", "/api/quick-apis/{id}/run");
    assert!(authorize(&grant, qa, None, &[target(Kind::QuickApi, "g-qa")], world).is_ok());
    let call = route("POST", "/api/agent-api/call");
    assert!(authorize(
        &grant,
        call,
        Some("p1"),
        &[reference(Kind::McpConfig, "gen-cfg")],
        world
    )
    .is_err());
    assert!(authorize(
        &grant,
        call,
        None,
        &[reference(Kind::McpConfig, "gen-cfg")],
        world
    )
    .is_ok());
    assert!(authorize(
        &grant,
        call,
        Some("p1"),
        &[reference(Kind::McpConfig, "b-cfg")],
        world
    )
    .is_err());
}

#[test]
fn a_failed_lookup_refuses() {
    let grant = grant(&["a-room"]);
    let meta = route("GET", "/api/discussions/{id}/meta");
    let broken = |_: Kind, _: &str| Err(Refusal("db down".into()));
    assert!(authorize(
        &grant,
        meta,
        Some("p1"),
        &[target(Kind::Discussion, "a-other")],
        broken
    )
    .is_err());
}

// ─── Response scoping ───────────────────────────────────────────────────────

fn residences(entries: &[(Kind, &str)]) -> Residences {
    entries
        .iter()
        .map(|(kind, id)| ((*kind, id.to_string()), world(*kind, id).unwrap()))
        .collect()
}

/// B-13 — default deny, any depth, typed ids, counts follow.
#[test]
fn responses_are_scoped_at_any_depth() {
    let grant = grant(&["a-room"]);
    let route = route("GET", "/api/mcp/workflow-run-discussions/{run_id}");
    let mut data = json!({
        "disc_count": 3,
        "discussions": [
            {"disc_id": "a-1"},
            {"disc_id": "b-1"},
            {"run": {"project_id": "p2"}},
            {"wrap": {"deeper": [{"discussion_id": "b-2"}, {"discussion_id": "a-3"}]}},
        ]
    });
    let known = residences(&[
        (Kind::Discussion, "a-1"),
        (Kind::Discussion, "b-1"),
        (Kind::Discussion, "b-2"),
        (Kind::Discussion, "a-3"),
    ]);
    scope_response(route, &mut data, &grant, Some("p1"), &known).unwrap();
    let kept = data["discussions"].as_array().unwrap();
    assert_eq!(kept.len(), 2, "{data}");
    assert_eq!(data["disc_count"], 2, "count recomputed");
    assert_eq!(
        kept[1]["wrap"]["deeper"].as_array().unwrap().len(),
        1,
        "a depth-3 list is filtered too"
    );
    // A discussion named under another key, at the root: refused.
    let status = route_for("GET", "/api/disc/session-status").unwrap();
    let mut session = json!({"bound_disc_id": "b-disc"});
    let known = residences(&[(Kind::Discussion, "b-disc")]);
    assert!(scope_response(status, &mut session, &grant, Some("p1"), &known).is_err());
    // An id that resolves to nothing hides its object.
    let mut unknown = json!([{"workflow_id": "ghost"}, {"workflow_id": "a-wf"}]);
    let known = residences(&[(Kind::Workflow, "a-wf")]);
    let runs = route_for("GET", "/api/workflows/{id}/runs").unwrap();
    scope_response(runs, &mut unknown, &grant, Some("p1"), &known).unwrap();
    assert_eq!(unknown.as_array().unwrap().len(), 1);
}

/// C-01 — a project-less discussion not owned is absent from lists.
#[test]
fn a_general_discussion_is_dropped_from_lists() {
    let grant = grant(&["a-room"]);
    let list = route("GET", "/api/discussions");
    let mut data = json!([
        {"id": "a-room", "project_id": "p1"},
        {"id": "g-other", "project_id": null},
    ]);
    let known = residences(&[(Kind::Discussion, "a-room"), (Kind::Discussion, "g-other")]);
    scope_response(list, &mut data, &grant, Some("p1"), &known).unwrap();
    assert_eq!(data.as_array().unwrap().len(), 1, "{data}");
}

/// A resource's own child lists are not typed as the resource's kind.
#[test]
fn a_resource_s_own_lists_are_not_mistaken_for_the_resource() {
    let grant = grant(&["a-room"]);
    let get = route("GET", "/api/workflows/{id}");
    let mut data =
        json!({"id": "a-wf", "project_id": "p1", "steps": [{"id": "step-1", "name": "s"}]});
    let known = residences(&[(Kind::Workflow, "a-wf")]);
    scope_response(get, &mut data, &grant, Some("p1"), &known).unwrap();
    assert_eq!(data["steps"].as_array().unwrap().len(), 1);
}

/// B-14 — an id resolver answer naming another project's config is refused.
#[test]
fn a_resolved_config_outside_the_project_is_refused() {
    let grant = grant(&["a-room"]);
    let resolve = route("GET", "/api/resolve/{id}");
    let mut data = json!({"kind": "mcp_config", "id": "b-cfg", "title": "x"});
    let known = residences(&[(Kind::McpConfig, "b-cfg")]);
    assert!(scope_response(resolve, &mut data, &grant, Some("p1"), &known).is_err());
}

#[test]
fn query_values_are_percent_decoded() {
    assert_eq!(percent_decode("a%20b+c"), "a b c");
    assert_eq!(percent_decode("%E2%9C%93"), "✓");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz"), "%zz");
}

#[test]
fn workflow_scopes_are_read_from_their_json() {
    assert_eq!(
        workflow_residence(Some("p1".into()), Some(r#"{"type":"All"}"#.into())),
        Residence::AllProjects
    );
    assert_eq!(
        workflow_residence(
            Some("p1".into()),
            Some(r#"{"type":"Projects","project_ids":["p2"]}"#.into())
        ),
        Residence::Projects(HashSet::from(["p1".to_string(), "p2".to_string()]))
    );
    assert_eq!(workflow_residence(None, None), Residence::Global);
    assert_eq!(
        workflow_residence(Some("p1".into()), None),
        in_project("p1")
    );
}
