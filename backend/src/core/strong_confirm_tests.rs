use super::*;

#[test]
fn every_route_of_the_inventory_is_protected() {
    for (method, pattern) in [
        ("POST", "/api/config/recovery/set"),
        ("GET", "/api/config/export"),
        ("POST", "/api/mcps/configs/{id}/reveal"),
        ("POST", "/api/external-api/connections/{id}/reveal"),
        ("POST", "/api/execution-context/{run_kind}/{run_id}/reveal"),
        ("POST", "/api/config/auth-token/regenerate"),
        ("POST", "/api/config/network-exposure"),
        ("POST", "/api/config/sync-agent-tokens"),
        ("POST", "/api/config/agent-access"),
        ("POST", "/api/config/server"),
        ("PUT", "/api/projects/{id}/github"),
        ("POST", "/api/agents/install"),
        // Everything `is_destructive` lists.
        ("POST", "/api/setup/reset"),
        ("POST", "/api/config/import"),
        ("PUT", "/api/projects/{id}/validations-access"),
        ("POST", "/api/agents/uninstall"),
        // The sensitive deletions, an explicit list.
        ("DELETE", "/api/projects/{id}"),
        ("DELETE", "/api/mcps/configs/{id}"),
        ("DELETE", "/api/external-api/connections/{id}"),
        ("DELETE", "/api/config/api-keys/{id}"),
        ("POST", "/api/human-credentials/revoke"),
    ] {
        assert_eq!(
            protection(method, pattern),
            Protection::Always,
            "{method} {pattern}"
        );
    }
}

/// Owner decision, 28/10: the everyday deletions stay open, and a new DELETE
/// route is open until it is added to the list on purpose.
#[test]
fn an_everyday_deletion_is_open() {
    for pattern in [
        "/api/discussions/{id}",
        "/api/discussions/{id}/messages/{message_id}",
        "/api/planning/tasks/{id}/blockers/{blocker_id}",
        "/api/workflows/{id}",
        "/api/workflows/{id}/runs/{run_id}",
        "/api/pages/{id}",
        "/api/quick-prompts/{id}",
        "/api/quick-apis/{id}",
        "/api/quick-execs/{id}",
        "/api/skills/{id}",
        "/api/profiles/{id}",
        "/api/directives/{id}",
        "/api/contacts/{id}",
        "/api/something/new/{id}",
    ] {
        assert_eq!(protection("DELETE", pattern), Protection::Open, "{pattern}");
    }
    // The destructive gate (local or token) is not this module's to loosen.
    assert!(crate::is_destructive(
        &axum::http::Method::DELETE,
        "/api/discussions/{id}"
    ));
}

#[test]
fn a_head_is_protected_like_the_get_it_runs() {
    assert_eq!(protection("HEAD", "/api/config/export"), Protection::Always);
    assert_eq!(protection("HEAD", "/api/discussions"), Protection::Open);
}

#[test]
fn the_routes_that_decide_in_the_handler_are_conditional() {
    for (method, pattern) in [
        ("POST", "/api/mcps/bundles/export"),
        ("PUT", "/api/mcps/custom/{server_id}"),
        ("PUT", "/api/external-api/connections/{id}"),
        ("POST", "/api/quick-execs"),
        ("PUT", "/api/quick-execs/{id}"),
    ] {
        assert_eq!(
            protection(method, pattern),
            Protection::Conditional,
            "{method} {pattern}"
        );
    }
}

#[test]
fn reads_and_the_routes_with_their_own_proof_stay_open() {
    for (method, pattern) in [
        ("GET", "/api/config/tokens"),
        ("GET", "/api/config/server"),
        ("GET", "/api/discussions"),
        ("GET", "/api/mcps/configs"),
        // The trust routes check the code themselves: asking twice spends two.
        ("PUT", "/api/trust/instance"),
        ("DELETE", "/api/trust/rooms/{id}"),
        ("POST", "/api/trust/proof/disable"),
        ("POST", "/api/trust/grants"),
        // A restore carries the recovery passphrase; a lost key has no seed to check.
        ("POST", "/api/config/recovery/restore"),
        ("POST", "/api/config/recovery/start-new-key"),
    ] {
        assert_eq!(
            protection(method, pattern),
            Protection::Open,
            "{method} {pattern}"
        );
    }
}

#[test]
fn the_object_is_the_values_of_the_pattern_segments() {
    assert_eq!(object_of("/api/config/export", "/api/config/export"), "-");
    assert_eq!(
        object_of(
            "/api/mcps/configs/{id}/reveal",
            "/api/mcps/configs/c1/reveal"
        ),
        "c1"
    );
    assert_eq!(
        object_of(
            "/api/execution-context/{run_kind}/{run_id}/reveal",
            "/api/execution-context/workflow/r9/reveal"
        ),
        "workflow/r9"
    );
}

#[test]
fn the_body_field_is_read_from_json_only() {
    assert_eq!(
        body_code(br#"{"exposed":true,"kronn_confirm":" 123456 "}"#),
        Some("123456".into())
    );
    assert_eq!(body_code(br#"{"kronn_confirm":""}"#), None);
    assert_eq!(body_code(br#"{"kronn_confirm":12}"#), None);
    assert_eq!(body_code(b"not json"), None);
}

#[test]
fn a_grant_is_single_use_and_bound_to_its_call() {
    let target = target_of("delete", "/api/projects/p1");
    assert_eq!(target, "DELETE /api/projects/p1");
    let grant = mint(&target, 1_000);
    assert!(grant.starts_with("kgr_"));
    assert!(consume(&grant, &target, 1_010));
    assert!(
        !consume(&grant, &target, 1_011),
        "a grant is spent by its first use"
    );
}

#[test]
fn a_grant_for_another_call_is_refused_and_burnt() {
    let grant = mint("DELETE /api/projects/p1", 1_000);
    assert!(!consume(&grant, "DELETE /api/projects/p2", 1_001));
    assert!(
        !consume(&grant, "DELETE /api/projects/p1", 1_002),
        "a mismatched attempt burns the grant"
    );
}

#[test]
fn a_grant_expires() {
    let target = "POST /api/config/export";
    let grant = mint(target, 1_000);
    assert!(!consume(&grant, target, 1_000 + GRANT_TTL_SECS));
    assert!(!consume("kgr_unknown", target, 1_000));
}

#[test]
fn grants_are_bounded() {
    let first = mint("GET /a", 1_000);
    for index in 0..MAX_GRANTS {
        mint(&format!("GET /b{index}"), 1_001 + index as u64);
    }
    assert!(!consume(&first, "GET /a", 1_100), "the oldest was dropped");
}

#[test]
fn the_data_dir_overlap_covers_inside_equal_and_containing() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let data = root.join("data");
    std::fs::create_dir_all(data.join("inner")).unwrap();
    std::fs::create_dir_all(root.join("other")).unwrap();
    assert!(overlaps_data_dir(&data, &data));
    assert!(overlaps_data_dir(&data.join("inner"), &data));
    assert!(overlaps_data_dir(&data.join("not-yet"), &data));
    assert!(
        overlaps_data_dir(&root, &data),
        "a parent contains the vault"
    );
    assert!(overlaps_data_dir(&data.join("inner").join(".."), &data));
    assert!(!overlaps_data_dir(&root.join("other"), &data));
    assert!(!overlaps_data_dir(&root.join("data-sibling"), &data));
    #[cfg(unix)]
    {
        let link = root.join("other").join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&data, &link).unwrap();
        assert!(overlaps_data_dir(&link, &data), "a symlink to the vault");
    }
}

/// Every `(method, pattern)` the production router registers under `/api/`,
/// read from the route table of `lib.rs` (the part before its tests).
fn registered_routes() -> Vec<(String, String)> {
    let source = include_str!("../lib.rs");
    let start = source
        .find("pub fn build_router_with_auth")
        .expect("the router builder");
    let table = &source[start..];
    let table = &table[..table.find("#[cfg(all(test").unwrap_or(table.len())];
    let mut routes = Vec::new();
    for chunk in table.split(".route(").skip(1) {
        let Some(open) = chunk.find('"') else {
            continue;
        };
        let rest = &chunk[open + 1..];
        let Some(close) = rest.find('"') else {
            continue;
        };
        let path = &rest[..close];
        if !path.starts_with("/api/") {
            continue;
        }
        // The handlers follow the path, up to the end of this `.route(` call.
        let handlers = &rest[close + 1..];
        let bytes = handlers.as_bytes();
        for verb in ["get", "post", "put", "patch", "delete"] {
            let needle = format!("{verb}(");
            let mut from = 0;
            while let Some(at) = handlers[from..].find(&needle) {
                let at = from + at;
                let before = if at == 0 { b' ' } else { bytes[at - 1] };
                // `.put(`, ` put(`, `(put(`; never `::delete(` or `my_delete(`.
                if !(before.is_ascii_alphanumeric() || before == b'_' || before == b':') {
                    routes.push((verb.to_ascii_uppercase(), path.to_string()));
                }
                from = at + needle.len();
            }
        }
    }
    routes.sort();
    routes.dedup();
    routes
}

/// The routes `is_destructive` gates that deliberately do not ask for the
/// strong confirmation (owner decision, 28/10): they are not a secret and not an
/// operation on the instance. They keep their local-or-token gate. Everything
/// else the destructive gate covers is in `ALWAYS`, `SENSITIVE_DELETES`,
/// `OWN_PROOF` or a `/api/trust` route.
const OPEN_ON_PURPOSE: &[(&str, &str)] = &[
    // Maintenance.
    ("POST", "/api/debug/logs/clear"),
    ("POST", "/api/api-call-logs/purge"),
    (
        "POST",
        "/api/orchestration/provider-quotas/{provider}/rearm",
    ),
    ("POST", "/api/audit-runs/cleanup"),
    ("POST", "/api/rtk/deactivate"),
    ("POST", "/api/config/embed-origins"),
    ("POST", "/api/projects/{id}/context-audit/baseline"),
    ("POST", "/api/projects/{id}/audit-attestation"),
    ("POST", "/api/mcps/custom/{server_id}/cleanup-orphan-env"),
    // Everyday deletions: a message, a discussion, a task, a workflow, a page, a
    // Quick Prompt / API / Exec, a skill, a profile, a directive, a contact, …
    ("DELETE", "/api/contacts/{id}"),
    ("DELETE", "/api/directives/{id}"),
    ("DELETE", "/api/discussions/{id}"),
    ("DELETE", "/api/discussions/{id}/context-files/{file_id}"),
    ("DELETE", "/api/discussions/{id}/messages/last"),
    ("DELETE", "/api/discussions/{id}/messages/{message_id}"),
    ("DELETE", "/api/lite-llm/model-failures"),
    ("DELETE", "/api/live-page-actions/{id}/trust"),
    ("DELETE", "/api/pages/{id}"),
    ("DELETE", "/api/pages/{id}/datasets/{name}"),
    ("DELETE", "/api/pages/{id}/discussions/{discussion_id}"),
    ("DELETE", "/api/planning/tasks/{id}/assignees"),
    ("DELETE", "/api/planning/tasks/{id}/blockers/{blocker_id}"),
    ("DELETE", "/api/planning/tasks/{id}/discussions"),
    ("DELETE", "/api/profiles/{id}"),
    ("DELETE", "/api/projects/{id}/template-merge"),
    ("DELETE", "/api/quick-apis/{id}"),
    ("DELETE", "/api/quick-execs/{id}"),
    ("DELETE", "/api/quick-prompts/{id}"),
    ("DELETE", "/api/quick-prompts/{id}/versions/{version_index}"),
    ("DELETE", "/api/skills/{id}"),
    ("DELETE", "/api/user-context/{name}"),
    ("DELETE", "/api/workflow-runs/{run_id}"),
    ("DELETE", "/api/workflows/{id}"),
    ("DELETE", "/api/workflows/{id}/runs"),
    ("DELETE", "/api/workflows/{id}/runs/{run_id}"),
    ("DELETE", "/api/workflows/{id}/runs/{run_id}/test-worktree"),
];

/// A new `is_destructive` route fails here until it is classified on purpose:
/// asking for the code (`ALWAYS`, `SENSITIVE_DELETES`, a conditional route, a
/// route with its own proof) or staying open (`OPEN_ON_PURPOSE`).
#[test]
fn every_destructive_route_is_classified_on_purpose() {
    let routes = registered_routes();
    assert!(routes.len() > 100, "the scan found {} routes", routes.len());
    let mut unclassified = Vec::new();
    for (method, pattern) in &routes {
        let verb = method.parse::<axum::http::Method>().unwrap();
        if !crate::is_destructive(&verb, pattern) {
            continue;
        }
        let asks = protection(method, pattern) != Protection::Open;
        let own_proof = pattern.starts_with("/api/trust")
            || OWN_PROOF.iter().any(|(m, p)| m == method && p == pattern);
        let open = OPEN_ON_PURPOSE
            .iter()
            .any(|(m, p)| m == method && p == pattern);
        if !(asks || own_proof || open) {
            unclassified.push(format!("{method} {pattern}"));
        }
        assert!(
            !(asks && open),
            "{method} {pattern} is both protected and open on purpose"
        );
    }
    assert!(
        unclassified.is_empty(),
        "classify these in `strong_confirm` (ALWAYS / SENSITIVE_DELETES) or in OPEN_ON_PURPOSE: {unclassified:#?}"
    );
}

/// The open list cannot rot: every entry is a real destructive route and is
/// really open.
#[test]
fn the_open_on_purpose_list_names_real_open_routes() {
    let routes = registered_routes();
    for (method, pattern) in OPEN_ON_PURPOSE {
        assert!(
            routes.iter().any(|(m, p)| m == method && p == pattern),
            "{method} {pattern} is not a registered route"
        );
        let verb = method.parse::<axum::http::Method>().unwrap();
        assert!(
            crate::is_destructive(&verb, pattern),
            "{method} {pattern} is not gated by is_destructive: nothing to classify"
        );
        assert_eq!(protection(method, pattern), Protection::Open);
    }
}

/// Every entry of the explicit lists is a registered route, so a rename of a
/// route cannot silently drop its protection.
#[test]
fn the_protected_lists_name_registered_routes() {
    let routes = registered_routes();
    let known = |method: &str, pattern: &str| {
        routes
            .iter()
            .any(|(m, p)| m.eq_ignore_ascii_case(method) && p == pattern)
    };
    for (method, pattern) in ALWAYS.iter().chain(CONDITIONAL).chain(OWN_PROOF) {
        assert!(known(method, pattern), "{method} {pattern}");
    }
    for pattern in SENSITIVE_DELETES {
        assert!(known("DELETE", pattern), "DELETE {pattern}");
    }
}

/// The owner's split, by name: a maintenance route is open and an instance
/// operation asks.
#[test]
fn a_maintenance_route_is_open_and_an_instance_operation_asks() {
    for (method, pattern) in OPEN_ON_PURPOSE {
        assert_eq!(protection(method, pattern), Protection::Open, "{pattern}");
    }
    for (method, pattern) in [
        ("POST", "/api/setup/reset"),
        ("POST", "/api/config/import"),
        ("POST", "/api/config/recovery/reencrypt"),
        ("PUT", "/api/projects/{id}/validations-access"),
        ("POST", "/api/agents/uninstall"),
        ("POST", "/api/pages/import"),
    ] {
        assert_eq!(protection(method, pattern), Protection::Always, "{pattern}");
    }
}
