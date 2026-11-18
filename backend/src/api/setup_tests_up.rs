
use super::*;

use crate::core::config;

use serial_test::serial;

#[test]
#[serial_test::serial]
fn codex_writer_refuses_an_isolated_instance_and_names_no_key() {
    let _restore = crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&[
        "HOME",
        "KRONN_DATA_DIR",
        "KRONN_HOST_HOME",
        "KRONN_HOST_MCP_SYNC",
        "KRONN_IN_DOCKER",
    ]);
    let dir = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("HOME", dir.path());
    crate::core::child_env::set_var("KRONN_DATA_DIR", dir.path().join("data"));
    for name in ["KRONN_HOST_HOME", "KRONN_HOST_MCP_SYNC", "KRONN_IN_DOCKER"] {
        crate::core::child_env::remove_var(name);
    }

    for key in [Some("sk-kt1162-never-leaks"), None] {
        let error = sync_codex_auth(key).unwrap_err();
        assert!(error.contains("KRONN_HOST_HOME"), "{error}");
        assert!(!error.contains("sk-kt1162"), "{error}");
    }
    assert!(!dir.path().join(".codex").exists());

    // The explicit redirect is honoured outside Docker.
    let host = dir.path().join("host");
    crate::core::child_env::set_var("KRONN_HOST_HOME", &host);
    assert_eq!(codex_auth_path(), host.join(".codex/auth.json"));
}
