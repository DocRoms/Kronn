// Proof test for the runtime write guard: an integration test that FORGOT
// isolate_config_dir() must be refused, not silently clobber the real config.
// Serial: the test below restores KRONN_DATA_DIR, which would let this save pass.
#[tokio::test]
#[serial_test::serial]
async fn unisolated_config_write_is_refused() {
    // Deliberately NO isolate_config_dir() and KRONN_DATA_DIR cleared.
    kronn::core::child_env::remove_var("KRONN_DATA_DIR");
    let cfg = kronn::core::config::default_config();
    let err = kronn::core::config::save(&cfg)
        .await
        .expect_err("write from an unisolated test binary must be refused");
    assert!(err.to_string().contains("isolate_config_dir"), "{err}");
}

// Outside unit tests, path resolution is the product's: an executable that
// happens to live under a `deps/` directory (as this one does) still resolves
// the user's data directory, never a test fallback.
#[test]
#[serial_test::serial]
fn an_integration_binary_resolves_the_real_data_directory() {
    let previous = kronn::core::child_env::var_os("KRONN_DATA_DIR");
    kronn::core::child_env::remove_var("KRONN_DATA_DIR");
    let dir = kronn::core::config::config_dir();
    if let Some(value) = previous {
        kronn::core::child_env::set_var("KRONN_DATA_DIR", value);
    }
    let dir = dir.unwrap();
    assert!(
        !dir.to_string_lossy().contains("kronn-test-data-"),
        "{}",
        dir.display()
    );
    assert!(
        dir.ends_with("kronn") || dir.ends_with("com.kronn.kronn"),
        "{}",
        dir.display()
    );
}
