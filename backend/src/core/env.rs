//! Runtime environment detection helpers.
//!
//! Distinguishes Docker container execution from native desktop/CLI execution.
//! All checks are runtime (env vars), not compile-time — the same binary works in both contexts.

/// Detect if running inside a Docker container.
///
/// Checks the explicit `KRONN_IN_DOCKER` marker (set by our Dockerfile /
/// docker-compose) plus the container runtimes' own markers, which also cover
/// images launched from an older compose file. Deliberately NOT keyed on
/// `KRONN_DATA_DIR`: that is a generic data-dir override a NATIVE user can set
/// too, and "docker" downgrades security posture (auth off by default, the
/// LAN-bind boot guard trusts KRONN_BIND instead of the real bind host) — a
/// false positive there is fail-open.
pub fn is_docker() -> bool {
    in_docker_marker(
        crate::core::child_env::var("KRONN_IN_DOCKER")
            .ok()
            .as_deref(),
    ) || std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
}

/// `KRONN_IN_DOCKER` counts only when explicitly true: "0" or an empty value
/// is not a container, and Docker relaxes the security posture.
fn in_docker_marker(value: Option<&str>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true"))
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
    if let Ok(host_os) = crate::core::child_env::var("KRONN_HOST_OS") {
        if !host_os.is_empty() && host_os != "host" {
            return host_os;
        }
    }

    // 2. Compile-time + runtime detection
    #[cfg(target_os = "linux")]
    {
        // WSL2 always sets WSL_DISTRO_NAME — check it first (most reliable)
        if crate::core::child_env::var("WSL_DISTRO_NAME").is_ok() {
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
    {
        "macOS".into()
    }

    #[cfg(target_os = "windows")]
    {
        "Windows".into()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    "Unknown".into()
}

/// Whether the host is a Mac with an Apple Silicon chip (KT-930).
///
/// The OS comes from `host_os_label`, so a Kronn in Docker on a Mac still
/// answers for the Mac, not for its Linux container. The chip is the build's
/// own architecture: Docker Desktop runs arm64 containers natively on Apple
/// Silicon and x86_64 ones on an Intel Mac. An amd64 image emulated on Apple
/// Silicon reads as Intel, which errs on the side of not offering MLX.
pub fn host_is_apple_silicon() -> bool {
    apple_silicon_from(&host_os_label(), std::env::consts::ARCH)
}

fn apple_silicon_from(host_os: &str, arch: &str) -> bool {
    host_os == "macOS" && arch == "aarch64"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn apple_silicon_needs_both_a_mac_and_an_arm_chip() {
        assert!(apple_silicon_from("macOS", "aarch64"));
        // An Intel Mac, and a Linux or WSL box that happens to be arm64.
        assert!(!apple_silicon_from("macOS", "x86_64"));
        assert!(!apple_silicon_from("Linux", "aarch64"));
        assert!(!apple_silicon_from("WSL", "aarch64"));
        assert!(!apple_silicon_from("Windows", "aarch64"));
        assert!(!apple_silicon_from("Unknown", "aarch64"));
    }

    #[test]
    fn only_an_explicitly_true_marker_means_docker() {
        for yes in ["1", "true", "TRUE", " 1 "] {
            assert!(in_docker_marker(Some(yes)), "{yes}");
        }
        for no in ["0", "", "false", "no", "yes"] {
            assert!(!in_docker_marker(Some(no)), "{no}");
        }
        assert!(!in_docker_marker(None));
    }

    #[test]
    #[serial]
    fn is_docker_true_when_marker_set() {
        let old = crate::core::child_env::var("KRONN_IN_DOCKER").ok();
        crate::core::child_env::set_var("KRONN_IN_DOCKER", "1");
        assert!(is_docker());
        if let Some(v) = old {
            crate::core::child_env::set_var("KRONN_IN_DOCKER", v);
        } else {
            crate::core::child_env::remove_var("KRONN_IN_DOCKER");
        }
    }

    #[test]
    #[serial]
    fn is_docker_ignores_data_dir_override() {
        // KRONN_DATA_DIR is a generic data-relocation knob a NATIVE user can
        // set; it must NOT flip docker mode (fail-open on the LAN guard).
        let old_marker = crate::core::child_env::var("KRONN_IN_DOCKER").ok();
        let old_data = crate::core::child_env::var("KRONN_DATA_DIR").ok();
        crate::core::child_env::remove_var("KRONN_IN_DOCKER");
        crate::core::child_env::set_var("KRONN_DATA_DIR", "/tmp/relocated");
        // (skip on machines actually running tests inside a container)
        if !std::path::Path::new("/.dockerenv").exists()
            && !std::path::Path::new("/run/.containerenv").exists()
        {
            assert!(!is_docker(), "data-dir override alone must not mean docker");
        }
        if let Some(v) = old_marker {
            crate::core::child_env::set_var("KRONN_IN_DOCKER", v);
        }
        match old_data {
            Some(v) => crate::core::child_env::set_var("KRONN_DATA_DIR", v),
            None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
        }
    }

    #[test]
    #[serial]
    fn auth_off_by_default_under_docker() {
        let old = crate::core::child_env::var("KRONN_IN_DOCKER").ok();
        crate::core::child_env::set_var("KRONN_IN_DOCKER", "1");
        assert!(!auth_on_by_default(), "Docker → auth must default OFF (localhost bypass can't see the real client behind NAT)");
        if let Some(v) = old {
            crate::core::child_env::set_var("KRONN_IN_DOCKER", v);
        } else {
            crate::core::child_env::remove_var("KRONN_IN_DOCKER");
        }
    }

    #[test]
    #[serial]
    fn auth_on_by_default_when_native() {
        let old = crate::core::child_env::var("KRONN_IN_DOCKER").ok();
        crate::core::child_env::remove_var("KRONN_IN_DOCKER");
        if !std::path::Path::new("/.dockerenv").exists()
            && !std::path::Path::new("/run/.containerenv").exists()
        {
            assert!(
                auth_on_by_default(),
                "Native (Tauri/CLI) → auth defaults ON; localhost bypass keeps it transparent"
            );
        }
        if let Some(v) = old {
            crate::core::child_env::set_var("KRONN_IN_DOCKER", v);
        }
    }

    #[test]
    #[serial]
    fn host_os_label_from_env() {
        let old = crate::core::child_env::var("KRONN_HOST_OS").ok();
        crate::core::child_env::set_var("KRONN_HOST_OS", "macOS");
        assert_eq!(host_os_label(), "macOS");
        if let Some(v) = old {
            crate::core::child_env::set_var("KRONN_HOST_OS", v);
        } else {
            crate::core::child_env::remove_var("KRONN_HOST_OS");
        }
    }

    #[test]
    #[serial]
    fn host_os_label_ignores_empty() {
        let old = crate::core::child_env::var("KRONN_HOST_OS").ok();
        crate::core::child_env::set_var("KRONN_HOST_OS", "");
        let label = host_os_label();
        assert!(
            !label.is_empty(),
            "Should fall through to platform detection"
        );
        if let Some(v) = old {
            crate::core::child_env::set_var("KRONN_HOST_OS", v);
        } else {
            crate::core::child_env::remove_var("KRONN_HOST_OS");
        }
    }

    #[test]
    #[serial]
    fn host_os_label_wsl_via_distro_name() {
        let old_os = crate::core::child_env::var("KRONN_HOST_OS").ok();
        let old_wsl = crate::core::child_env::var("WSL_DISTRO_NAME").ok();
        crate::core::child_env::remove_var("KRONN_HOST_OS");
        crate::core::child_env::set_var("WSL_DISTRO_NAME", "Ubuntu");
        let label = host_os_label();
        if let Some(v) = old_os {
            crate::core::child_env::set_var("KRONN_HOST_OS", v);
        } else {
            crate::core::child_env::remove_var("KRONN_HOST_OS");
        }
        if let Some(v) = old_wsl {
            crate::core::child_env::set_var("WSL_DISTRO_NAME", v);
        } else {
            crate::core::child_env::remove_var("WSL_DISTRO_NAME");
        }
        #[cfg(target_os = "linux")]
        assert_eq!(label, "WSL");
        #[cfg(not(target_os = "linux"))]
        let _ = label; // WSL_DISTRO_NAME ignored on non-Linux
    }

    #[test]
    fn host_os_label_returns_known_platform() {
        let label = host_os_label();
        let known = ["Linux", "WSL", "macOS", "Windows", "Unknown"];
        assert!(
            known.contains(&label.as_str()),
            "Unexpected platform: {}",
            label
        );
    }
}
