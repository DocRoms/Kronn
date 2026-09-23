fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["wait_for_backend", "restart_app", "pick_folders"]),
    ))
    .expect("failed to build Tauri command permissions");
}
