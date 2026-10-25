//! KT-1341 — the MCP bridge (CLI agents) and the native catalogue (HTTP
//! agents) offer the same Kronn capabilities. A tool on one side only fails
//! here until it is listed below with the decision that keeps it so: adding a
//! capability to one audience is never an accident.
use super::*;
use std::collections::BTreeSet;

const SESSION: &str = "CLI session plumbing: an HTTP agent's turn is its own message, with no joined session, lease or peer";
const HUMAN_ONLY: &str = "deleting a skill (it drops the history) and writing profiles or directives stay human; the bridge refuses these routes to every agent";
const DEFERRED: &str = "deferred to KT-1356: HTTP agents do not have it yet";

/// MCP tools an HTTP agent does not get, and why.
const MCP_ONLY: &[(&str, &str)] = &[
    ("bridge_info", SESSION),
    (
        "kronn_intro",
        "an HTTP agent receives the introduction in its system context",
    ),
    ("disc_join", SESSION),
    ("disc_leave", SESSION),
    ("disc_wait_for_peer", SESSION),
    ("disc_transfer_session", SESSION),
    ("disc_find_by_session", SESSION),
    ("disc_link", SESSION),
    ("disc_unlink", SESSION),
    ("disc_invite_peer", SESSION),
    ("disc_append", SESSION),
    (
        "disc_workspace_get",
        "an HTTP agent's file tools are bound to its workspace already",
    ),
    (
        "disc_workspace_set",
        "an HTTP agent's file tools are bound to its workspace already",
    ),
    ("disc_workspace_history_lease", SESSION),
    (
        "task_exec_accept_worker_offer",
        "only a CLI worker takes an offer; a native worker is launched directly",
    ),
    (
        "task_exec_resume",
        "resumes a CLI worker's session; a native worker is relaunched",
    ),
    ("disc_load_other", "HTTP equivalent: disc_read"),
    ("skill_delete", HUMAN_ONLY),
    ("profile_create", HUMAN_ONLY),
    ("profile_update", HUMAN_ONLY),
    ("profile_delete", HUMAN_ONLY),
    ("directive_create", HUMAN_ONLY),
    ("directive_update", HUMAN_ONLY),
    ("directive_delete", HUMAN_ONLY),
    ("audit_prepare", DEFERRED),
    ("audit_launch", DEFERRED),
    ("audit_status", DEFERRED),
    ("audit_install_template", DEFERRED),
    ("convention_get", DEFERRED),
    ("directives_list", DEFERRED),
    ("directive_get", DEFERRED),
    ("profiles_list", DEFERRED),
    ("profile_get", DEFERRED),
    ("skills_list", DEFERRED),
    ("disc_create", DEFERRED),
    ("disc_create_room", DEFERRED),
    ("disc_get_message", DEFERRED),
    ("disc_meta", DEFERRED),
    ("disc_note_list", DEFERRED),
    ("disc_question_list", DEFERRED),
    ("disc_search", DEFERRED),
    ("disc_summarize", DEFERRED),
    ("disc_update", DEFERRED),
    ("disk_usage", DEFERRED),
    ("learning_propose", DEFERRED),
    ("page_delete_dataset", DEFERRED),
    ("proposal_get", DEFERRED),
    ("proposal_list", DEFERRED),
    ("qp_batch_run", DEFERRED),
    ("qp_delete", DEFERRED),
    ("resolve_id", DEFERRED),
    ("task_assign", DEFERRED),
    ("task_changes", DEFERRED),
    ("task_unlink_discussion", DEFERRED),
    ("workflow_active_runs", DEFERRED),
    ("workflow_cancel_run", DEFERRED),
    ("workflow_clone", DEFERRED),
    ("workflow_resume_run", DEFERRED),
    ("workflow_run_discussions", DEFERRED),
    ("workflow_run_get", DEFERRED),
    ("workflow_run_status", DEFERRED),
    ("workflow_runs", DEFERRED),
    ("workflow_set_enabled", DEFERRED),
    ("workflow_trigger", DEFERRED),
    ("workflow_update_step", DEFERRED),
    ("workflow_validate", DEFERRED),
    ("workflow_wait_for_completion", DEFERRED),
];

const OWN_TOOLS: &str =
    "a CLI agent has its own file, git and web tools; Kronn gives them to an HTTP agent";

/// HTTP tools an MCP agent does not get, and why.
const HTTP_ONLY: &[(&str, &str)] = &[
    ("read_file", OWN_TOOLS),
    ("write_file", OWN_TOOLS),
    ("edit_file", OWN_TOOLS),
    ("edit_lines", OWN_TOOLS),
    ("insert_after_line", OWN_TOOLS),
    ("list_files", OWN_TOOLS),
    ("find_files", OWN_TOOLS),
    ("search_text", OWN_TOOLS),
    ("git_status", OWN_TOOLS),
    ("git_diff", OWN_TOOLS),
    ("git_log", OWN_TOOLS),
    ("git_commit", OWN_TOOLS),
    ("web_fetch", OWN_TOOLS),
    ("disc_read", "MCP equivalent: disc_load_other"),
    (
        "api_endpoints",
        "MCP agents read a plugin's endpoints in mcp_list",
    ),
    (
        "tools_load",
        "loads a family of a tiered native catalogue; the MCP catalogue is not tiered",
    ),
    (
        "agent_job_start",
        "a native agent's own background job; a CLI agent runs it in its session",
    ),
    (
        "agent_schedule_wake",
        "a native agent's own wake-up; a CLI agent waits with disc_wait_for_peer",
    ),
    (
        "agent_resume_status",
        "a native agent's own background job; a CLI agent runs it in its session",
    ),
    (
        "agent_resume_cancel",
        "a native agent's own background job; a CLI agent runs it in its session",
    ),
];

/// Every tool an HTTP agent can be given, in any context.
fn http_tools() -> BTreeSet<String> {
    let mut all = full_discussion_catalogue();
    all.extend(workflow_tool_catalogue(true));
    all.extend(workflow_workspace_tool_catalogue());
    all.extend(audit_tool_catalogue());
    all.push(tools_load_declaration());
    all.iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// Every tool the MCP bridge declares in `TOOLS`.
fn mcp_tools() -> BTreeSet<String> {
    let script = include_str!("../../scripts/disc-introspection-mcp.py");
    let start = script.find("\nTOOLS = [").expect("TOOLS in the bridge");
    let end = start + script[start..].find("\n]\n").expect("end of TOOLS");
    script[start..end]
        .lines()
        .filter_map(|line| line.strip_prefix("        \"name\": \""))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_owned)
        .collect()
}

fn listed(exclusions: &[(&str, &str)]) -> BTreeSet<String> {
    exclusions
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect()
}

fn one_sided(
    only_here: &BTreeSet<String>,
    exclusions: &[(&str, &str)],
    other_side: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let excluded = listed(exclusions);
    let missing = only_here.difference(&excluded).cloned().collect();
    // An exclusion for a tool both sides now have, or that no longer exists,
    // is stale: it would hide the next asymmetry under the same name.
    let stale = excluded
        .iter()
        .filter(|name| !only_here.contains(*name) || other_side.contains(*name))
        .cloned()
        .collect();
    (missing, stale)
}

#[test]
fn every_tool_on_one_side_only_is_a_listed_decision() {
    let (mcp, http) = (mcp_tools(), http_tools());
    assert!(
        mcp.len() > 100,
        "the TOOLS parser lost the catalogue: {}",
        mcp.len()
    );
    assert!(
        http.contains("room_scope"),
        "HTTP agents ask for a project too"
    );
    let mcp_only: BTreeSet<String> = mcp.difference(&http).cloned().collect();
    let http_only: BTreeSet<String> = http.difference(&mcp).cloned().collect();
    let (mcp_missing, mcp_stale) = one_sided(&mcp_only, MCP_ONLY, &http);
    let (http_missing, http_stale) = one_sided(&http_only, HTTP_ONLY, &mcp);
    assert!(
        mcp_missing.is_empty(),
        "MCP tools with no HTTP counterpart: add them to the native catalogue, \
         or list them in MCP_ONLY with the reason: {mcp_missing:?}"
    );
    assert!(
        http_missing.is_empty(),
        "HTTP tools with no MCP counterpart: add them to the bridge, \
         or list them in HTTP_ONLY with the reason: {http_missing:?}"
    );
    assert!(
        mcp_stale.is_empty() && http_stale.is_empty(),
        "stale exclusions, remove them: {mcp_stale:?} {http_stale:?}"
    );
    for (name, reason) in MCP_ONLY.iter().chain(HTTP_ONLY) {
        assert!(!reason.trim().is_empty(), "{name} needs its reason");
    }
}

/// The guide every agent reads before building a workflow or a Page answers
/// through the same tool on both sides.
#[test]
fn both_audiences_load_the_workflow_guide_through_tool_manual() {
    let listed = tool_manual(None, None);
    assert!(listed["available"]
        .as_array()
        .unwrap()
        .contains(&json!(WORKFLOW_GUIDE)));
    let reach = tool_manual(Some(WORKFLOW_GUIDE), Some("Reach"));
    assert!(reach["content"]
        .as_str()
        .unwrap()
        .contains("Projects accessible"));
    let script = include_str!("../../scripts/disc-introspection-mcp.py");
    assert!(script.contains("WORKFLOW_GUIDE = \"workflow-architect\""));
    assert!(script.contains("/api/skills/{WORKFLOW_GUIDE}/guide"));
    assert!(crate::core::bridge_token::route_for("GET", "/api/skills/{id}/guide").is_some());
}

/// Versions, shadow runs and refs (KT-1434/1447/1448/1449): both audiences
/// read the same manuals, word for word, and the same guide section.
#[test]
fn version_and_shadow_knowledge_is_the_same_on_both_sides() {
    let script = include_str!("../../scripts/disc-introspection-mcp.py");
    for manual in [
        SKILL_GET_MANUAL,
        SKILL_CREATE_MANUAL,
        SKILL_UPDATE_MANUAL,
        QP_GET_MANUAL,
        WORKFLOW_VARIANTS_MANUAL,
    ] {
        for sentence in manual.split(". ") {
            let sentence = sentence.trim_end_matches('.');
            assert!(
                script.contains(sentence),
                "the bridge's manual lacks: {sentence}"
            );
        }
    }
    assert_eq!(
        tool_manual(Some("skill_get"), None)["manual"],
        SKILL_GET_MANUAL
    );
    assert_eq!(tool_manual(Some("qp_get"), None)["manual"], QP_GET_MANUAL);
    for (tool, manual) in [
        ("skill_create", SKILL_CREATE_MANUAL),
        ("skill_update", SKILL_UPDATE_MANUAL),
    ] {
        assert_eq!(tool_manual(Some(tool), None)["manual"], manual);
    }
    for tool in ["workflow_create_draft", "workflow_update"] {
        let manual = tool_manual(Some(tool), None)["manual"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(manual.ends_with(WORKFLOW_VARIANTS_MANUAL), "{tool}");
    }
    let section = tool_manual(Some(WORKFLOW_GUIDE), Some("Versions and shadow runs"));
    let content = section["content"].as_str().unwrap();
    for fact in [
        "external_effect: true",
        "{{run.variant}}",
        "custom-x@N",
        "qp_get({qp_id, version: N})",
        "**human-only**",
        "KRONN_REF_<NAME>",
    ] {
        assert!(content.contains(fact), "the guide section lacks {fact}");
    }
    let schema = crate::api::workflows::canonical_step_schema();
    assert!(schema["shape"]
        .as_str()
        .unwrap()
        .contains("external_effect"));
    let namespaces = &schema["template_vars"]["namespaces"];
    assert!(namespaces["run.variant"].is_string() && namespaces["refs.<name>"].is_string());
}
