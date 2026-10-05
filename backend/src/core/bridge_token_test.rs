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
    assert!(mint(BridgeScope::default()).is_none(), "nothing to bind to");
}

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

#[test]
fn ids_are_read_from_path_query_and_body() {
    let route = route_for("GET", "/api/workflows/{id}/runs/{run_id}").unwrap();
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
            "nested": {"project_id": "not-top-level"}
        })),
    );
    for expected in [
        (Kind::Workflow, "wf-1"),
        (Kind::Run, "run-1"),
        (Kind::Project, "p-1"),
        (Kind::Discussion, "d 2"),
        (Kind::Discussion, "d-body"),
        (Kind::Project, "p-a"),
        (Kind::Project, "p-b"),
    ] {
        assert!(
            ids.contains(&(expected.0, expected.1.to_string())),
            "{expected:?} missing from {ids:?}"
        );
    }
    assert_eq!(ids.len(), 7, "{ids:?}");
}

fn grant(discussions: &[&str]) -> BridgeGrant {
    BridgeGrant {
        id: "test".into(),
        scope: scope(discussions),
        adopted: Mutex::new(Vec::new()),
    }
}

fn in_project(project: &str) -> Residence {
    Residence::Projects(HashSet::from([project.to_string()]))
}

/// Resources of a fixture world: `a*` live in project `p1`, `b*` in `p2`,
/// `g*` are project-less.
fn world(_: Kind, id: &str) -> Result<Option<Residence>, Refusal> {
    Ok(match id.chars().next() {
        Some('a') => Some(in_project("p1")),
        Some('b') => Some(in_project("p2")),
        Some('g') => Some(Residence::Global),
        _ => None,
    })
}

#[test]
fn a_discussion_token_writes_only_its_own_room() {
    let grant = grant(&["a-room"]);
    let append = route_for("POST", "/api/disc/append").unwrap();
    let own = [(Kind::Discussion, "a-room".to_string())];
    assert!(authorize(&grant, append, Some("p1"), &own, world).is_ok());
    for other in ["a-other-room", "b-room", "g-room"] {
        let ids = [(Kind::Discussion, other.to_string())];
        assert!(
            authorize(&grant, append, Some("p1"), &ids, world).is_err(),
            "append to {other} must be refused"
        );
    }
}

#[test]
fn a_discussion_the_launch_created_is_its_own() {
    let grant = grant(&["a-room"]);
    let append = route_for("POST", "/api/disc/append").unwrap();
    let child = [(Kind::Discussion, "a-child".to_string())];
    assert!(authorize(&grant, append, Some("p1"), &child, world).is_err());
    grant.adopt_discussion("a-child");
    assert!(authorize(&grant, append, Some("p1"), &child, world).is_ok());
}

#[test]
fn reads_stay_inside_the_token_s_project_for_path_and_body_ids() {
    let grant = grant(&["a-room"]);
    let meta = route_for("GET", "/api/discussions/{id}/meta").unwrap();
    let same_project = [(Kind::Discussion, "a-other".to_string())];
    assert!(authorize(&grant, meta, Some("p1"), &same_project, world).is_ok());
    let cross = [(Kind::Discussion, "b-room".to_string())];
    assert!(authorize(&grant, meta, Some("p1"), &cross, world).is_err());
    // A project named explicitly must be the token's own.
    let list = route_for("GET", "/api/discussions").unwrap();
    let other_project = [(Kind::Project, "p2".to_string())];
    assert!(authorize(&grant, list, Some("p1"), &other_project, world).is_err());
    let own_project = [(Kind::Project, "p1".to_string())];
    assert!(authorize(&grant, list, Some("p1"), &own_project, world).is_ok());
    // A project-less token cannot name a project at all.
    assert!(authorize(&grant, list, None, &own_project, world).is_err());
    // An id that resolves to nothing is refused, never let through.
    let missing = [(Kind::Task, "zzz".to_string())];
    assert!(authorize(&grant, list, Some("p1"), &missing, world).is_err());
}

#[test]
fn effects_land_only_on_the_bound_project() {
    let grant = grant(&["a-room"]);
    let trigger = route_for("POST", "/api/mcp/workflow-trigger").unwrap();
    let own_workflow = [(Kind::Workflow, "a-wf".to_string())];
    assert!(authorize(&grant, trigger, Some("p1"), &own_workflow, world).is_ok());
    let foreign_workflow = [(Kind::Workflow, "b-wf".to_string())];
    assert!(authorize(&grant, trigger, Some("p1"), &foreign_workflow, world).is_err());
    // A shared (project-less) workflow runs only when aimed at the bound project.
    let shared = [(Kind::Workflow, "g-wf".to_string())];
    assert!(authorize(&grant, trigger, Some("p1"), &shared, world).is_err());
    let shared_on_own = [
        (Kind::Workflow, "g-wf".to_string()),
        (Kind::Project, "p1".to_string()),
    ];
    assert!(authorize(&grant, trigger, Some("p1"), &shared_on_own, world).is_ok());
    let shared_on_other = [
        (Kind::Workflow, "g-wf".to_string()),
        (Kind::Project, "p2".to_string()),
    ];
    assert!(authorize(&grant, trigger, Some("p1"), &shared_on_other, world).is_err());
}

#[test]
fn a_failed_lookup_refuses() {
    let grant = grant(&["a-room"]);
    let meta = route_for("GET", "/api/discussions/{id}/meta").unwrap();
    let ids = [(Kind::Discussion, "a-other".to_string())];
    let broken = |_: Kind, _: &str| Err(Refusal("db down".into()));
    assert!(authorize(&grant, meta, Some("p1"), &ids, broken).is_err());
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

#[test]
fn a_trigger_naming_no_project_gets_the_bound_one() {
    let trigger = route_for("POST", "/api/mcp/workflow-trigger").unwrap();
    let body = with_bound_project(trigger, Some("p1"), Some(json!({"workflow_id": "w"}))).unwrap();
    assert_eq!(body["project_id"], "p1");
    // An explicit project is kept (and then checked); other routes untouched.
    assert!(with_bound_project(trigger, Some("p1"), Some(json!({"project_id": "p2"}))).is_none());
    assert!(with_bound_project(trigger, None, Some(json!({}))).is_none());
    let append = route_for("POST", "/api/disc/append").unwrap();
    assert!(with_bound_project(append, Some("p1"), Some(json!({}))).is_none());
}
