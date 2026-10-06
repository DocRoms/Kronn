use super::*;
use crate::models::ActivityCategory::*;
use serde_json::json;

/// A string no output may ever contain.
pub(crate) const SENTINEL: &str = "hunter2-SENTINEL-sk_live_abcdefghijklmnop";

#[test]
fn every_call_is_counted_by_category_even_when_nobody_reads_in_between() {
    let (sink, rx) = tokio::sync::watch::channel(None);
    for _ in 0..49 {
        tool_started(Some(&sink), Read);
    }
    tool_started(Some(&sink), Execute);
    let latest = rx.borrow().clone().unwrap();
    assert_eq!((latest.category, latest.calls), (Execute, 50));
}

#[test]
fn each_agent_familys_built_in_tools_map_to_their_category() {
    let families: &[(&str, &[(&str, ActivityCategory)])] = &[
        (
            "Claude Code",
            &[
                ("Read", Read),
                ("Grep", Search),
                ("Glob", Search),
                ("Edit", Edit),
                ("Write", Edit),
                ("Bash", Execute),
                ("WebFetch", Web),
                ("TodoWrite", Think),
            ],
        ),
        (
            "Codex",
            &[
                ("shell", Execute),
                ("exec_command", Execute),
                ("apply_patch", Edit),
                ("read_file", Read),
                ("update_plan", Think),
                ("web_search", Web),
            ],
        ),
        (
            "OpenCode",
            &[
                ("read", Read),
                ("grep", Search),
                ("edit", Edit),
                ("bash", Execute),
                ("webfetch", Web),
                ("todowrite", Think),
            ],
        ),
        (
            "Vibe",
            &[
                ("read_file", Read),
                ("write_file", Edit),
                ("search_replace", Edit),
                ("grep", Search),
                ("bash", Execute),
                ("todo", Think),
            ],
        ),
        (
            "Copilot",
            &[
                ("view", Read),
                ("create", Edit),
                ("str_replace", Edit),
                ("bash", Execute),
            ],
        ),
        (
            "Gemini",
            &[
                ("read_file", Read),
                ("search_file_content", Search),
                ("replace", Edit),
                ("run_shell_command", Execute),
                ("google_web_search", Web),
            ],
        ),
        (
            "Kronn HTTP loop",
            &[
                ("read_file", Read),
                ("search_text", Search),
                ("edit_lines", Edit),
                ("git_commit", Edit),
                ("web_fetch", Web),
                ("api_call", Web),
                ("disc_list", Kronn),
                ("task_create", Kronn),
            ],
        ),
        (
            "MCP",
            &[
                ("mcp__kronn-internal__disc_list", Kronn),
                ("mcp__kronn-a1b2__task_get", Kronn),
                ("mcp__github__create_pull_request", Mcp),
            ],
        ),
    ];
    for (family, tools) in families {
        for (name, expected) in *tools {
            assert_eq!(category_of(name), *expected, "{family}: {name}");
        }
    }
    for kind in [
        "read", "search", "edit", "delete", "move", "execute", "fetch", "think", "other",
    ] {
        let expected = match kind {
            "read" => Read,
            "search" => Search,
            "edit" | "delete" | "move" => Edit,
            "execute" => Execute,
            "fetch" => Web,
            "think" => Think,
            _ => Other,
        };
        assert_eq!(category_of_acp_kind(kind), expected, "ACP kind {kind}");
    }
    for unknown in [SENTINEL, "Run: mysql -phunter2", "my_custom_tool", ""] {
        assert_eq!(category_of(unknown), Other, "{unknown:?}");
    }
}

#[test]
fn the_recent_actions_are_bounded_newest_first_and_updated_in_place_by_id() {
    let mut recent = RecentActivity::default();
    assert_eq!(recent.snapshot(), AuditRecentActivity::default());
    for call in 0..40 {
        let name = if call % 2 == 0 { "Read" } else { "Bash" };
        assert!(recent.apply(&ToolActivityUpdate::named(None, name)));
    }
    let shown = recent.snapshot();
    assert_eq!(shown.entries.len(), RECENT_MAX_ENTRIES);
    assert_eq!(shown.entries[0].category, Execute, "the newest first");
    assert_eq!(shown.entries[1].category, Read);

    let mut recent = RecentActivity::default();
    let id = Some("call-1".to_owned());
    assert!(recent.apply(&ToolActivityUpdate::named(id.clone(), "Bash")));
    for _ in 0..20 {
        assert!(!recent.apply(&ToolActivityUpdate::named(id.clone(), "Bash")));
    }
    assert_eq!(recent.snapshot().entries.len(), 1, "one call, one entry");
}

/// Generic ACP: the kind and the id, never the title, raw input or locations.
#[test]
fn a_generic_acp_update_reads_its_kind_only() {
    let update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "t1",
        "title": format!("mysql -phunter2 {SENTINEL}"),
        "kind": "execute",
        "rawInput": {"command": SENTINEL, "file_path": SENTINEL},
        "locations": [{"path": SENTINEL}]
    });
    let parsed = ToolActivityUpdate::from_acp(&update).unwrap();
    assert_eq!(parsed.category(), Some(Execute));
    assert!(!format!("{parsed:?}").contains("hunter2"));
    let mut recent = RecentActivity::default();
    assert!(recent.apply(&parsed));
    let later =
        ToolActivityUpdate::from_acp(&json!({"toolCallId": "t1", "title": SENTINEL})).unwrap();
    assert!(!later.carries_detail());
    assert!(!recent.apply(&later));
    let shown = serde_json::to_string(&recent.snapshot()).unwrap();
    assert!(
        !shown.contains("hunter2") && !shown.contains("SENTINEL"),
        "{shown}"
    );
}

/// An activity stored before 0.14.3 held the tool's name and target: both are
/// dropped when it is read, so they are never served again.
#[test]
fn a_legacy_activity_loses_its_name_and_target_when_read() {
    let legacy = json!({
        "tool": SENTINEL, "target": format!("cat {SENTINEL}"),
        "at": "2026-10-01T00:00:00Z", "calls": 3
    });
    let read: AgentActivity = serde_json::from_value(legacy).unwrap();
    assert_eq!((read.category, read.calls), (Other, 3));
    let served = serde_json::to_string(&read).unwrap();
    assert!(
        !served.contains("SENTINEL") && !served.contains("tool") && !served.contains("target"),
        "{served}"
    );
}
