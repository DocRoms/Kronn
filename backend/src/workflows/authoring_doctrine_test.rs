use super::*;

const GUIDE_SOURCE: &str = include_str!("../skills/workflow-architect.md");
const BRIDGE_SOURCE: &str = include_str!("../../scripts/disc-introspection-mcp.py");

fn guide() -> String {
    crate::core::skills::get_skill(crate::core::skill_guide::WORKFLOW_GUIDE)
        .expect("builtin guide")
        .content
}

#[test]
fn the_schema_and_the_guide_serve_one_doctrine() {
    let schema = crate::api::workflows::canonical_step_schema();
    assert_eq!(
        schema["authoring_doctrine"].as_str().unwrap(),
        AUTHORING_DOCTRINE
    );
    // The guide's source only marks where it goes: no second copy to drift.
    assert_eq!(GUIDE_SOURCE.matches(DOCTRINE_MARKER).count(), 1);
    assert!(
        !GUIDE_SOURCE.contains("### At equal token cost"),
        "the guide source must not carry its own copy of the doctrine"
    );
    let guide = guide();
    assert!(guide.contains(AUTHORING_DOCTRINE.trim_end()));
    assert!(!guide.contains("<!-- kronn:"), "every marker is rendered");
    // And as the one section an agent reads on demand.
    let section = crate::core::skill_guide::guide(
        crate::core::skill_guide::WORKFLOW_GUIDE,
        Some("Authoring doctrine"),
    )
    .unwrap();
    assert_eq!(
        section["content"].as_str().unwrap(),
        AUTHORING_DOCTRINE.trim()
    );
}

#[test]
fn the_doctrine_stays_short_and_carries_the_equal_cost_rules() {
    // 2 to 3 000 tokens, at the 3.7 bytes per token the MCP budget assumes.
    assert!(
        AUTHORING_DOCTRINE.len() <= 11_100,
        "{} bytes",
        AUTHORING_DOCTRINE.len()
    );
    for rule in [
        "ApiCall",
        "BatchApiCall",
        "CollectApiData",
        "Quick Exec",
        "exec_script_files",
        "step outputs",
        "PAGINATION_TRUNCATED",
        "GraphQL",
        "1 to 20 steps",
    ] {
        assert!(AUTHORING_DOCTRINE.contains(rule), "missing: {rule}");
    }
}

#[test]
fn every_surface_lists_the_step_types_of_the_code() {
    let names = StepType::names();
    assert_eq!(names.len(), StepType::ALL.len());
    // schema
    let schema = crate::api::workflows::canonical_step_schema();
    let closed: Vec<&str> = schema["step_types_closed_set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(closed, names);
    for name in &names {
        assert!(schema["fields_by_type"].get(*name).is_some(), "{name}");
    }
    // guide: the count, the list and one section per type
    let guide = guide();
    assert!(guide.contains(&format!(
        "**{} step types**: {}",
        names.len(),
        step_types_inline()
    )));
    for name in &names {
        assert!(
            guide
                .lines()
                .any(|l| l.starts_with("### ") && l.contains(&format!("`{name}`"))),
            "the guide has no section for {name}"
        );
    }
    // MCP bridge: the tuple the greeting follows, and the draft description
    let tuple = BRIDGE_SOURCE
        .split("STEP_TYPES = (")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("STEP_TYPES in the bridge");
    let bridge: Vec<&str> = tuple.split('"').skip(1).step_by(2).collect();
    assert_eq!(bridge, names);
    let described = BRIDGE_SOURCE
        .split("of the closed \"\n            \"set **")
        .nth(1)
        .expect("the workflow_create_draft description names the set")
        .split("**")
        .next()
        .unwrap();
    let described: String = described
        .replace("\"\n            \"", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(described, names.join(", "));
}

#[test]
fn the_stale_passages_are_gone() {
    let guide = guide();
    for stale in [
        "Limit to 4-5 steps",
        "Never end on Structured",
        "fall back to `Agent` + bash tool",
        "as of 0.6.0",
        "fifteen step types",
        "The 15 step types",
    ] {
        assert!(!guide.contains(stale), "stale: {stale}");
    }
    assert!(!BRIDGE_SOURCE.contains("closed 12"));
    assert!(!BRIDGE_SOURCE.contains("des 12 types"));
    // 20 is the real cap wherever it is stated.
    assert!(guide.contains("the hard cap is 20 steps"));
    // `bash -c` is an Exec form, and no reason to leave Exec.
    assert!(guide.contains("`bash -c`, `python3 -c` and `node -e` are valid Exec forms"));
}

#[test]
fn the_schema_states_what_happens_past_the_pagination_cap_and_for_graphql() {
    let schema = crate::api::workflows::canonical_step_schema();
    let api = &schema["fields_by_type"]["ApiCall"];
    let limits = api["pagination_limits"].as_str().unwrap();
    assert!(limits.contains("PAGINATION_TRUNCATED") && limits.contains("50"));
    assert!(limits.contains("neither fails nor truncates silently"));
    let graphql = api["graphql"].as_str().unwrap();
    assert!(graphql.contains("/graphql") && graphql.contains("does NOT paginate"));
    assert!(api["optional"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f.as_str().unwrap().starts_with("api_pagination")));
}
