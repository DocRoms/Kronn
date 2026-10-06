use super::*;
use serde_json::json;

#[test]
fn a_target_completes_the_call_started_last_and_never_invents_one() {
    let (sink, rx) = tokio::sync::watch::channel(None);
    tool_target(Some(&sink), "orphan".into());
    assert_eq!(*rx.borrow(), None, "no call started, nothing to complete");

    tool_started(Some(&sink), "Read");
    tool_target(Some(&sink), "src/lib.rs".into());
    let first = rx.borrow().clone().unwrap();
    assert_eq!(
        (first.tool.as_str(), first.target.as_deref()),
        ("Read", Some("src/lib.rs"))
    );

    tool_started(Some(&sink), "Bash");
    let second = rx.borrow().clone().unwrap();
    assert_eq!((second.tool.as_str(), second.target), ("Bash", None));
    assert!(second.at >= first.at);
    assert_eq!((first.calls, second.calls), (1, 2));
}

#[test]
fn every_call_is_counted_even_when_nobody_reads_in_between() {
    let (sink, rx) = tokio::sync::watch::channel(None);
    for _ in 0..50 {
        tool_started(Some(&sink), "Read");
        tool_target(Some(&sink), "src/é.rs".into());
    }
    assert_eq!(rx.borrow().as_ref().unwrap().calls, 50);
}

fn shell(command: &str) -> Option<String> {
    input_target(&json!({ "command": command })).map(SafeTarget::into_string)
}

/// Every leak the r24 review confirmed, and the secret bytes that must never show.
const LEAKS: &[(&str, &[&str])] = &[
    (
        "curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123' https://api.example.com/x",
        &["abcdefghij", "Bearer", "api.example.com"],
    ),
    (
        "curl -u admin:hunter2 https://api.example.com/x",
        &["admin", "hunter2"],
    ),
    (
        "curl --user admin:hunter2 https://api.example.com/x",
        &["admin", "hunter2"],
    ),
    (
        "curl https://admin:hunter2@example.com/api",
        &["admin", "hunter2"],
    ),
    (
        "git clone https://oauth2:glpat-AbCdEfGhIjKlMnOpQrSt@gitlab.com/o/r.git",
        &["oauth2", "glpat", "AbCdEf"],
    ),
    ("mysql -u root -phunter2 mydb", &["hunter2", "root"]),
    ("sshpass -p hunter2 ssh host", &["hunter2"]),
    (
        "docker login -u bob -p hunter2 registry.example.com",
        &["hunter2", "bob"],
    ),
    ("redis-cli -a hunter2 ping", &["hunter2"]),
    ("mytool --pass hunter2", &["hunter2"]),
    (
        "echo sk_live_abcdefghijklmnopqrstuvwx",
        &["sk_live", "abcdefgh"],
    ),
    (
        "echo kbt_0123456789abcdef0123456789abcdef",
        &["kbt_", "0123456789"],
    ),
    (
        "aws configure set aws_secret_access_key wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        &["wJalr", "EXAMPLEKEY", "aws_secret"],
    ),
    ("PGPASSWORD='hunter two' psql -h db", &["hunter", "two"]),
    ("mytool --password 'hunter two'", &["hunter", "two"]),
    (
        "cat > ~/.pgpass <<EOF\ndb:5432:*:app:hunter2\nEOF",
        &["hunter2", "5432"],
    ),
    (
        "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz npm test",
        &["sk-proj", "abcdefgh"],
    ),
    (
        "export T=1; sk_live_abcdefghijklmnop1234 --run",
        &["sk_live", "abcdefgh"],
    ),
    (
        "echo \"$(cat ~/.token)\" | gh auth login --with-token",
        &["token)", "cat ~"],
    ),
    (
        "psql postgres://app:hunter2@db/app -c 'select 1'",
        &["hunter2", "app:"],
    ),
];

#[test]
fn no_confirmed_leak_shows_a_secret_byte() {
    for (command, secrets) in LEAKS {
        let shown = shell(command).unwrap_or_default();
        for secret in *secrets {
            assert!(
                !shown.contains(secret),
                "{command:?} showed {secret:?}: {shown:?}"
            );
        }
    }
}

#[test]
fn a_shell_command_shows_its_programs_and_plain_redirect_paths_only() {
    let cases = [
        ("curl -s https://x.io | jq .data", "curl | jq"),
        // A secret-like name gets the redaction marker, any other the ellipsis.
        (
            "PGPASSWORD='hunter two' psql -h db",
            "PGPASSWORD=***REDACTED*** psql",
        ),
        ("LANG=C sort", "LANG=… sort"),
        ("cat > ~/.pgpass <<EOF\nsecret\nEOF", "cat > ~/.pgpass"),
        ("cargo test --lib 2>&1 | tail -20", "cargo | tail"),
        ("cd backend && npm test; echo done", "cd && npm; echo"),
        ("/usr/bin/env FOO=1 make", "env"),
        ("grep -r x src > out.txt", "grep > out.txt"),
        ("cat <<< 'hunter2' > /tmp/x", "cat > /tmp/x"),
        ("echo $(whoami) hunter2", "echo"),
        ("echo 'unterminated hunter2", "echo"),
        ("TOKEN=abc", "TOKEN=***REDACTED***"),
        ("x86_64-linux-gnu-gcc-12 main.c", "…"),
    ];
    for (command, shown) in cases {
        assert_eq!(shell(command).as_deref(), Some(shown), "{command:?}");
    }
    assert_eq!(
        shell("'$weird' a").as_deref(),
        Some("…"),
        "an unreadable program shows as …"
    );
    // An argv shows its program, or the script it runs.
    let argv = |argv: serde_json::Value| {
        input_target(&json!({ "command": argv })).map(SafeTarget::into_string)
    };
    assert_eq!(
        argv(json!(["bash", "-lc", "mysql -phunter2 db"])).as_deref(),
        Some("mysql")
    );
    assert_eq!(
        argv(json!(["git", "push", "https://x:ghp_abc@github.com"])).as_deref(),
        Some("git")
    );
}

#[test]
fn a_url_shows_its_scheme_and_host_only() {
    let url = |url: &str| input_target(&json!({ "url": url })).map(SafeTarget::into_string);
    assert_eq!(
        url("https://admin:hunter2@example.com/api?k=v#f").as_deref(),
        Some("https://example.com")
    );
    assert_eq!(
        url("https://u:pa/ss@example.com:8443/x").as_deref(),
        Some("https://example.com")
    );
    assert_eq!(
        url("https://h.io/p?access_token=abc").as_deref(),
        Some("https://h.io")
    );
    let ambiguous = url("https://user:1234/x").unwrap();
    assert!(!ambiguous.contains("1234"), "{ambiguous}");
    assert_eq!(url("not a url"), None);
}

#[test]
fn a_file_tool_shows_its_path_and_a_search_never_its_pattern() {
    let target = |input: serde_json::Value| input_target(&input).map(SafeTarget::into_string);
    assert_eq!(
        target(json!({"file_path": "docs/a.md", "content": "TOP SECRET"})).as_deref(),
        Some("docs/a.md")
    );
    assert_eq!(
        target(json!({"pattern": "sk-live-hunter2", "path": "src/"})).as_deref(),
        Some("in src/")
    );
    assert_eq!(target(json!({"pattern": "password=hunter2"})), None);
    assert_eq!(target(json!({"old_string": "a", "new_string": "b"})), None);
    assert_eq!(target(json!({"query": "my password hunter2"})), None);
    assert_eq!(
        target(json!({"path": "docs/sk_live_abcdef..."})),
        None,
        "shortened by the runtime"
    );
    assert_eq!(target(json!({"path": "a\nhunter2"})), None);
    let long = target(json!({"path": "é".repeat(300)})).unwrap();
    assert_eq!(long.chars().count(), TARGET_MAX_CHARS + 1);
    assert!(long.ends_with('…'));
}

#[test]
fn a_tool_name_that_is_not_an_identifier_is_never_shown() {
    assert_eq!(
        safe_tool_name("mcp__github__create_pull_request"),
        "mcp__github__create_pull_request"
    );
    assert_eq!(safe_tool_name("Run: mysql -phunter2"), "tool");
    assert_eq!(safe_tool_name(&"T".repeat(80)), "tool");
}

#[test]
fn the_recent_actions_are_bounded_newest_first_and_updated_in_place_by_id() {
    let mut recent = RecentActivity::default();
    assert_eq!(recent.snapshot(), AuditRecentActivity::default());
    for call in 0..40 {
        assert!(recent.apply(&ToolActivityUpdate::call(
            None,
            &format!("Read{call}"),
            None
        )));
        let target = input_target(&json!({"path": format!("src/{call}.rs")})).unwrap();
        assert!(!recent.apply(&ToolActivityUpdate::target(None, target)));
    }
    let shown = recent.snapshot();
    assert_eq!(shown.entries.len(), RECENT_MAX_ENTRIES);
    assert_eq!(shown.entries[0].tool, "Read39");
    assert_eq!(shown.entries[0].target.as_deref(), Some("src/39.rs"));
    assert_eq!(shown.entries[RECENT_MAX_ENTRIES - 1].tool, "Read25");

    // Progress updates of one call replace its entry; they never add one.
    let mut recent = RecentActivity::default();
    let id = Some("call-1".to_owned());
    assert!(recent.apply(&ToolActivityUpdate::call(id.clone(), "Execute", None)));
    for _ in 0..20 {
        assert!(!recent.apply(&ToolActivityUpdate::call(id.clone(), "Execute", None)));
    }
    let target = input_target(&json!({"command": "npm test"})).unwrap();
    assert!(!recent.apply(&ToolActivityUpdate::target(id, target)));
    let shown = recent.snapshot();
    assert_eq!(shown.entries.len(), 1);
    assert_eq!(shown.entries[0].target.as_deref(), Some("npm"));
}

/// Generic ACP: kind and raw input, never the title; one entry per call id.
#[test]
fn a_generic_acp_update_is_built_from_its_structured_fields_never_its_title() {
    let update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "t1",
        "title": "mysql -u root -phunter2 mydb",
        "kind": "execute",
        "status": "in_progress",
        "rawInput": {"command": "cat > ~/.pgpass <<EOF\ndb:5432:*:app:hunter2\nEOF"}
    });
    let parsed = acp_tool_update(&update).unwrap();
    assert_eq!(parsed.tool(), Some("Execute"));
    assert_eq!(parsed.target_text(), Some("cat > ~/.pgpass"));
    let mut recent = RecentActivity::default();
    assert!(recent.apply(&parsed));
    // A later update with only a truncated title and a location.
    let later = acp_tool_update(&json!({
        "toolCallId": "t1",
        "title": "Reading sk-ant-api03-abc...",
        "locations": [{"path": "docs/a.md"}]
    }))
    .unwrap();
    assert!(!recent.apply(&later));
    let all = serde_json::to_string(&recent.snapshot()).unwrap();
    assert!(
        !all.contains("hunter2") && !all.contains("sk-ant") && !all.contains("mysql"),
        "{all}"
    );
    assert_eq!(recent.snapshot().entries.len(), 1);
    assert_eq!(
        recent.snapshot().entries[0].target.as_deref(),
        Some("docs/a.md")
    );
}

/// The Claude adapter's target: the heredoc body after the first line never shows.
#[test]
fn the_raw_input_target_keeps_the_first_line_rules() {
    let raw = r#"{"command":"cat > ~/.pgpass <<EOF\ndb:5432:*:app:hunter2\nEOF"}"#;
    assert_eq!(tool_input_target(raw).as_deref(), Some("cat > ~/.pgpass"));
    assert_eq!(tool_input_target("{not json"), None);
    assert_eq!(tool_input_target(r#"{"todos":[]}"#), None);
}

/// Input past the scan bound is never parsed whole: only its first program shows.
#[test]
fn a_huge_command_is_bounded_before_any_parsing() {
    let huge = format!("echo {} hunter2", "a".repeat(INPUT_SCAN_MAX_BYTES * 4));
    assert_eq!(shell(&huge).as_deref(), Some("echo"));
}
