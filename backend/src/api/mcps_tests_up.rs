use super::*;

/// A probed MCP server, possibly chosen by a repository's `.mcp.json`,
/// sees its own configured values and no backend secret (B4-04).
#[cfg(unix)]
// dumps the child environment with probe::env_dumping_program, a #!/bin/sh script made executable through unix mode bits
#[test]
fn a_probed_mcp_server_sees_no_backend_secret() {
    use crate::core::child_env::probe;
    probe::plant_real_sentinel();
    let bin = tempfile::tempdir().unwrap();
    let out = probe::env_dumping_program(bin.path(), "repo-mcp");
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let mut parent = probe::parent_with_secrets(&path, "/home/u");
    parent.push(("KRONN_KEK".into(), "sentinel-legacy-kek".into()));
    let borrowed: Vec<(&str, &str)> = parent
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let env =
        std::collections::HashMap::from([("SERVER_API_TOKEN".to_string(), "its-own".to_string())]);
    let command = bin.path().join("repo-mcp");
    let _ = crate::core::child_env::with_parent_env(&borrowed, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(probe_mcp_stdio_with_timeout(
                command.to_str().unwrap(),
                &[],
                &env,
                // The fake exits once it has written its environment, so
                // the probe returns then; the deadline only bounds a hang.
                Duration::from_secs(60),
            ))
    });
    let seen = probe::read_dump(&out);
    probe::assert_dump_without_secrets(&seen, &[]);
    assert!(!seen.contains_key("KRONN_KEK"));
    assert_eq!(
        seen.get("SERVER_API_TOKEN").map(String::as_str),
        Some("its-own")
    );
}

use crate::core::mcp_scanner::McpServerEntry;
