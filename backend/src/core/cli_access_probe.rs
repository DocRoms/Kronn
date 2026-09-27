//! CLI access probe — KT-829.
//!
//! Checks a plugin's CLI access the way a human would: is the binary present,
//! is its version acceptable, is it actually logged in. Every step runs
//! through the Quick Exec engine (`core::quick_exec`) — no shell, an
//! allowlisted binary, a literal argv fixed by the registry declaration —
//! exactly like every other process Kronn spawns on a caller's behalf.
//!
//! Deliberately does NOT read or log the CLI's own credential: the version
//! and auth-check commands are chosen (in `registry::cli_access_probe`)
//! specifically because they never print a secret on success.

use crate::core::quick_exec::{self, QuickExecSpec, QuickExecStatus, Summariser};
use crate::core::registry::CliAccessProbe;
use crate::models::ProbeDiagnosticCode;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub struct CliAccessResult {
    pub code: ProbeDiagnosticCode,
    pub detail: String,
}

const PROBE_TIMEOUT_SECS: u64 = 8;

/// Run the three-step check: presence, version floor, active auth.
///
/// `roots` is any set of declared Quick Exec roots — the probe's cwd doesn't
/// matter to `fastly`/`glab` (they report on the CLI's own login state, not
/// on a project's code), so any existing directory from the caller's roots
/// will do. Mirrors `api::rtk_state`'s "any declared root will do, but there
/// must BE one" rule.
pub async fn probe(
    declared: &CliAccessProbe,
    roots: &[PathBuf],
    cancel: &CancellationToken,
) -> CliAccessResult {
    let Some(cwd) = roots.iter().find(|path| path.is_dir()).cloned() else {
        return CliAccessResult {
            code: ProbeDiagnosticCode::CliMissing,
            detail: "no declared root is available to run the CLI probe in".into(),
        };
    };

    let version = match run_step(declared.binary, declared.version_args, &cwd, roots, cancel).await
    {
        Ok(result) if result.status.is_success() => result,
        Ok(result) => {
            return CliAccessResult {
                code: ProbeDiagnosticCode::CliMissing,
                detail: format!(
                    "`{} {}` did not report a usable version: {}",
                    declared.binary,
                    declared.version_args.join(" "),
                    result.summary
                ),
            }
        }
        Err(detail) => {
            return CliAccessResult {
                code: ProbeDiagnosticCode::CliMissing,
                detail,
            }
        }
    };

    if let Some(min_version) = declared.min_version {
        if let (Some(found), Some(floor)) =
            (extract_semver(&version.summary), parse_semver(min_version))
        {
            if found < floor {
                return CliAccessResult {
                    code: ProbeDiagnosticCode::CliVersionTooOld,
                    detail: format!(
                        "`{}` reports version {}.{}.{}, below the required {min_version}",
                        declared.binary, found.0, found.1, found.2
                    ),
                };
            }
        }
    }

    match run_step(declared.binary, declared.auth_check_args, &cwd, roots, cancel).await {
        Ok(result) if result.status.is_success() => CliAccessResult {
            code: ProbeDiagnosticCode::Ok,
            detail: format!(
                "`{} {}` succeeded",
                declared.binary,
                declared.auth_check_args.join(" ")
            ),
        },
        Ok(result) => CliAccessResult {
            code: ProbeDiagnosticCode::CliNotAuthenticated,
            detail: format!(
                "`{} {}` did not confirm an active session: {}",
                declared.binary,
                declared.auth_check_args.join(" "),
                result.summary
            ),
        },
        Err(detail) => CliAccessResult {
            code: ProbeDiagnosticCode::CliNotAuthenticated,
            detail,
        },
    }
}

/// Validate + run one Quick Exec step. `Err` means the binary itself could
/// not be spawned (missing, or refused by the allowlist) — the caller
/// classifies that as `CliMissing`.
async fn run_step(
    binary: &str,
    argv: &[&str],
    cwd: &Path,
    roots: &[PathBuf],
    cancel: &CancellationToken,
) -> Result<quick_exec::QuickExecResult, String> {
    let spec = QuickExecSpec {
        binary: binary.to_string(),
        argv: argv.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        timeout_secs: Some(PROBE_TIMEOUT_SECS),
        stdin: None,
        summariser: Summariser::Generic,
    };
    let validated = quick_exec::validate(&spec, roots)
        .map_err(|rejection| format!("`{binary}` could not be spawned: {rejection}"))?;
    let result = quick_exec::run(&validated, None, cancel)
        .await
        .map_err(|error| format!("`{binary}` could not be spawned: {error}"))?;
    if matches!(result.status, QuickExecStatus::Rejected) {
        return Err(format!("`{binary}` could not be spawned: {}", result.summary));
    }
    Ok(result)
}

/// Parse an exact `major.minor.patch` string — the shape a registry
/// declaration's `min_version` is written in.
fn parse_semver(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.trim().parse().ok()?;
    Some((major, minor, patch))
}

/// Find the first `\d+\.\d+\.\d+` substring in free-form CLI output (e.g.
/// `"Fastly CLI version v10.13.0"`, `"glab version 1.36.0 (abcdef)"`).
fn extract_semver(text: &str) -> Option<(u64, u64, u64)> {
    let regex = regex_lite::Regex::new(r"(\d+)\.(\d+)\.(\d+)").ok()?;
    let captures = regex.captures(text)?;
    Some((
        captures.get(1)?.as_str().parse().ok()?,
        captures.get(2)?.as_str().parse().ok()?,
        captures.get(3)?.as_str().parse().ok()?,
    ))
}

#[cfg(test)]
#[path = "cli_access_probe_test.rs"]
mod cli_access_probe_test;
