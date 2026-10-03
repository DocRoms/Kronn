//! Tests for the CLI access probe — KT-829.
//!
//! Uses `git` (always present — this whole toolchain depends on it) instead
//! of the real `fastly`/`glab` binaries so every branch is deterministic
//! regardless of which plugin CLIs happen to be installed on the machine
//! running the suite. `git --version` and a bogus subcommand give the two
//! independently controllable exit codes `true`/`false`/`echo` cannot (they
//! ignore their arguments).

use super::*;
use crate::core::registry::CliAccessProbe;
use tokio_util::sync::CancellationToken;

fn roots() -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let roots = vec![dir.path().to_path_buf()];
    (dir, roots)
}

// ── semver parsing (pure) ────────────────────────────────────────────

#[test]
fn parse_semver_reads_an_exact_triple() {
    assert_eq!(parse_semver("5.0.0"), Some((5, 0, 0)));
    assert_eq!(parse_semver(" 12.34.56 "), Some((12, 34, 56)));
    assert_eq!(parse_semver("not-a-version"), None);
    assert_eq!(parse_semver("5.0"), None);
}

#[test]
fn extract_semver_finds_a_triple_inside_free_form_cli_output() {
    assert_eq!(
        extract_semver("Fastly CLI version v10.13.0"),
        Some((10, 13, 0))
    );
    assert_eq!(
        extract_semver("glab version 1.36.0 (2024-01-01)"),
        Some((1, 36, 0))
    );
    assert_eq!(extract_semver("no digits here"), None);
}

// ── run_step: the three outcomes a single Quick Exec call can produce ─

#[tokio::test]
async fn run_step_reports_success_for_a_clean_exit() {
    let (_dir, roots) = roots();
    let result = run_step("git", &["--version"], &roots[0], &roots, &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.status.is_success());
}

#[tokio::test]
async fn run_step_reports_ok_with_a_failed_status_for_a_nonzero_exit() {
    let (_dir, roots) = roots();
    let result = run_step(
        "git",
        &["this-is-not-a-real-git-subcommand"],
        &roots[0],
        &roots,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!result.status.is_success());
}

#[tokio::test]
async fn run_step_errs_when_the_binary_is_not_allowlisted() {
    let (_dir, roots) = roots();
    let error = run_step(
        "totally-not-a-real-binary-xyz",
        &["--version"],
        &roots[0],
        &roots,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(error.contains("could not be spawned"));
}

// ── probe(): the full three-step classification ──────────────────────

#[tokio::test]
async fn probe_is_ok_when_version_and_auth_both_succeed() {
    let (_dir, roots) = roots();
    let declared = CliAccessProbe {
        binary: "git",
        version_args: &["--version"],
        min_version: None,
        auth_check_args: &["--version"],
    };
    let result = probe(&declared, &roots, &CancellationToken::new()).await;
    assert_eq!(result.code, ProbeDiagnosticCode::Ok, "{}", result.detail);
}

#[tokio::test]
async fn probe_reports_cli_not_authenticated_when_only_the_auth_check_fails() {
    let (_dir, roots) = roots();
    let declared = CliAccessProbe {
        binary: "git",
        version_args: &["--version"],
        min_version: None,
        auth_check_args: &["this-is-not-a-real-git-subcommand"],
    };
    let result = probe(&declared, &roots, &CancellationToken::new()).await;
    assert_eq!(result.code, ProbeDiagnosticCode::CliNotAuthenticated);
}

#[tokio::test]
async fn probe_reports_cli_version_too_old_below_a_declared_floor() {
    let (_dir, roots) = roots();
    let declared = CliAccessProbe {
        binary: "git",
        version_args: &["--version"],
        // No real git release will ever reach this — deterministic regardless
        // of which git is installed on the machine running the suite.
        min_version: Some("999.0.0"),
        auth_check_args: &["--version"],
    };
    let result = probe(&declared, &roots, &CancellationToken::new()).await;
    assert_eq!(result.code, ProbeDiagnosticCode::CliVersionTooOld);
}

#[tokio::test]
async fn probe_reports_cli_missing_when_the_binary_is_not_allowlisted() {
    let (_dir, roots) = roots();
    let declared = CliAccessProbe {
        binary: "totally-not-a-real-binary-xyz",
        version_args: &["--version"],
        min_version: None,
        auth_check_args: &["whoami"],
    };
    let result = probe(&declared, &roots, &CancellationToken::new()).await;
    assert_eq!(result.code, ProbeDiagnosticCode::CliMissing);
}

#[tokio::test]
async fn probe_reports_cli_missing_when_no_root_is_declared() {
    let declared = CliAccessProbe {
        binary: "git",
        version_args: &["--version"],
        min_version: None,
        auth_check_args: &["--version"],
    };
    let result = probe(&declared, &[], &CancellationToken::new()).await;
    assert_eq!(result.code, ProbeDiagnosticCode::CliMissing);
}

#[tokio::test]
async fn probe_reports_the_declared_fastly_and_gitlab_shapes_without_panicking() {
    // Not asserting a specific code here — whether `fastly`/`glab` are
    // actually installed on the machine running the suite is exactly what
    // this probe exists to answer, so any of the four stable codes is a
    // legitimate outcome. What this test proves is that the real registry
    // declarations survive the whole plumbing (validate → run → classify)
    // without erroring or panicking.
    let (_dir, roots) = roots();
    for server_id in ["mcp-fastly", "mcp-gitlab"] {
        let declared = crate::core::registry::cli_access_probe(server_id)
            .expect("declared CLI access probe");
        let result = probe(&declared, &roots, &CancellationToken::new()).await;
        assert!(
            matches!(
                result.code,
                ProbeDiagnosticCode::Ok
                    | ProbeDiagnosticCode::CliMissing
                    | ProbeDiagnosticCode::CliVersionTooOld
                    | ProbeDiagnosticCode::CliNotAuthenticated
            ),
            "unexpected code for {server_id}: {:?} — {}",
            result.code,
            result.detail
        );
    }
}
