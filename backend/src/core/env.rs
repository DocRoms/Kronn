//! Runtime environment detection helpers.
//!
//! Distinguishes Docker container execution from native desktop/CLI execution.
//! All checks are runtime (env vars), not compile-time — the same binary works in both contexts.

/// Detect if running inside a Docker container.
/// Docker is indicated by `KRONN_DATA_DIR` (set by docker-compose).
pub fn is_docker() -> bool {
    std::env::var("KRONN_DATA_DIR").is_ok()
}

/// Whether API auth should be ENABLED by default when a fresh config first
/// generates its token (`core::config::load`).
///
/// ON for native (Tauri/CLI): the `auth_middleware` localhost bypass keeps it
/// transparent for the single-machine user. OFF under Docker: Docker Desktop
/// (macOS/Windows) NATs every published-port request to the Docker network
/// gateway, so the bypass can't recognise the real client as local — auth-on
/// would 401 the user on first launch ("Cannot connect to backend"). The token
/// is still generated (ready for opt-in multi-user); an exposed Docker server
/// enables auth explicitly. The middleware honours `auth_enabled` either way.
pub fn auth_on_by_default() -> bool {
    !is_docker()
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
        // WSL2 always sets WSL_DISTRO_NAME — check it first (most reliable)
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return "WSL".into();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn is_docker_true_when_data_dir_set() {
        let old = std::env::var("KRONN_DATA_DIR").ok();
        std::env::set_var("KRONN_DATA_DIR", "/data");
        assert!(is_docker());
        if let Some(v) = old { std::env::set_var("KRONN_DATA_DIR", v); } else { std::env::remove_var("KRONN_DATA_DIR"); }
    }

    #[test]
    #[serial]
    fn is_docker_false_when_data_dir_unset() {
        let old = std::env::var("KRONN_DATA_DIR").ok();
        std::env::remove_var("KRONN_DATA_DIR");
        assert!(!is_docker());
        if let Some(v) = old { std::env::set_var("KRONN_DATA_DIR", v); }
    }

    #[test]
    #[serial]
    fn auth_off_by_default_under_docker() {
        let old = std::env::var("KRONN_DATA_DIR").ok();
        std::env::set_var("KRONN_DATA_DIR", "/data");
        assert!(!auth_on_by_default(), "Docker → auth must default OFF (localhost bypass can't see the real client behind NAT)");
        if let Some(v) = old { std::env::set_var("KRONN_DATA_DIR", v); } else { std::env::remove_var("KRONN_DATA_DIR"); }
    }

    #[test]
    #[serial]
    fn auth_on_by_default_when_native() {
        let old = std::env::var("KRONN_DATA_DIR").ok();
        std::env::remove_var("KRONN_DATA_DIR");
        assert!(auth_on_by_default(), "Native (Tauri/CLI) → auth defaults ON; localhost bypass keeps it transparent");
        if let Some(v) = old { std::env::set_var("KRONN_DATA_DIR", v); }
    }

    #[test]
    #[serial]
    fn host_os_label_from_env() {
        let old = std::env::var("KRONN_HOST_OS").ok();
        std::env::set_var("KRONN_HOST_OS", "macOS");
        assert_eq!(host_os_label(), "macOS");
        if let Some(v) = old { std::env::set_var("KRONN_HOST_OS", v); } else { std::env::remove_var("KRONN_HOST_OS"); }
    }

    #[test]
    #[serial]
    fn host_os_label_ignores_empty() {
        let old = std::env::var("KRONN_HOST_OS").ok();
        std::env::set_var("KRONN_HOST_OS", "");
        let label = host_os_label();
        assert!(!label.is_empty(), "Should fall through to platform detection");
        if let Some(v) = old { std::env::set_var("KRONN_HOST_OS", v); } else { std::env::remove_var("KRONN_HOST_OS"); }
    }

    #[test]
    #[serial]
    fn host_os_label_wsl_via_distro_name() {
        let old_os = std::env::var("KRONN_HOST_OS").ok();
        let old_wsl = std::env::var("WSL_DISTRO_NAME").ok();
        std::env::remove_var("KRONN_HOST_OS");
        std::env::set_var("WSL_DISTRO_NAME", "Ubuntu");
        let label = host_os_label();
        if let Some(v) = old_os { std::env::set_var("KRONN_HOST_OS", v); } else { std::env::remove_var("KRONN_HOST_OS"); }
        if let Some(v) = old_wsl { std::env::set_var("WSL_DISTRO_NAME", v); } else { std::env::remove_var("WSL_DISTRO_NAME"); }
        #[cfg(target_os = "linux")]
        assert_eq!(label, "WSL");
        #[cfg(not(target_os = "linux"))]
        let _ = label; // WSL_DISTRO_NAME ignored on non-Linux
    }

    #[test]
    fn host_os_label_returns_known_platform() {
        let label = host_os_label();
        let known = ["Linux", "WSL", "macOS", "Windows", "Unknown"];
        assert!(known.contains(&label.as_str()), "Unexpected platform: {}", label);
    }
}
