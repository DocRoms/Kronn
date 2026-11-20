use super::*;

use std::fs;

#[test]
fn ccusage_gets_claude_logs_location_and_no_backend_secret() {
    use crate::core::child_env::probe;
    let mut parent = probe::parent_with_secrets("/usr/bin", "/home/u");
    parent.push(("CLAUDE_CONFIG_DIR".into(), "/home/u/.claude-alt".into()));
    let borrowed: Vec<(&str, &str)> = parent
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let command = crate::core::child_env::with_parent_env(&borrowed, || {
        ccusage_command(std::path::Path::new("/usr/bin/ccusage"), "daily")
    });
    probe::assert_built_without_secrets(command.as_std(), "/usr/bin", &[]);
    assert_eq!(
        probe::env_of(command.as_std())
            .get("CLAUDE_CONFIG_DIR")
            .map(String::as_str),
        Some("/home/u/.claude-alt")
    );
}
