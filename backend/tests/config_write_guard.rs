// Proof test for the runtime write guard: an integration test that FORGOT
// isolate_config_dir() must be refused, not silently clobber the real config.
#[tokio::test]
async fn unisolated_config_write_is_refused() {
    // Deliberately NO isolate_config_dir() and KRONN_DATA_DIR cleared.
    std::env::remove_var("KRONN_DATA_DIR");
    let cfg = kronn::core::config::default_config();
    let err = kronn::core::config::save(&cfg)
        .await
        .expect_err("write from an unisolated test binary must be refused");
    assert!(err.to_string().contains("isolate_config_dir"), "{err}");
}
