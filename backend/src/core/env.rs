//! Runtime environment detection helpers.
//!
//! Distinguishes Docker container execution from native desktop/CLI execution.
//! All checks are runtime (env vars), not compile-time — the same binary works in both contexts.

/// Detect if running inside a Docker container.
/// Docker is indicated by `KRONN_DATA_DIR` (set by docker-compose).
pub fn is_docker() -> bool {
    std::env::var("KRONN_DATA_DIR").is_ok()
}

/// Detect the host operating system label.
pub fn host_os_label() -> String {
    // 1. Trust environment variable (set by docker-compose from Makefile)
    if let Ok(host_os) = std::env::var("KRONN_HOST_OS") {
        if !host_os.is_empty() && host_os != "host" {
            return host_os;
        }
    }

    // 2. Compile-time + runtime detection
    #[cfg(target_os = "linux")]
    {
        if let Ok(version) = std::fs::read_to_string("/proc/version") {
            let lower = version.to_lowercase();
            if lower.contains("microsoft") || lower.contains("wsl") {
                return "WSL".into();
            }
        }
        "Linux".into()
    }

    #[cfg(target_os = "macos")]
    { "macOS".into() }

    #[cfg(target_os = "windows")]
    { "Windows".into() }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    "Unknown".into()
}
