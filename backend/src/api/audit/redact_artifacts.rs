//! 0.8.13 security blocker — redact secret literals from Kronn-managed audit
//! artifacts, FAIL-CLOSED, at every boundary of the audit flow.
//!
//! Why (room design, Codex review): the audit agent runs with the project root
//! as its cwd + full filesystem access, so it can `Read` a prior/produced
//! `docs/…` artifact directly. A leaked secret literal there would enter the
//! agent's context (which leaves the machine) before we could rewrite it. So we
//! sanitize the Kronn-managed artifacts:
//!   * PRE-AGENT — the existing priors, before the snapshot/digest/prompt;
//!   * PER-STEP — the file a step just wrote, before the NEXT step spawns
//!     (else steps N+1..16 could read a secret step N emitted);
//!   * POST-LOOP / PRE-PUBLICATION — a final backstop before reconciliation,
//!     baseline and validation.
//!
//! FAIL-CLOSED: any inability to guarantee a clean artifact (unreadable,
//! unwritable, a symlinked target we refuse to follow) returns `Err`. The
//! caller MUST abort the run and publish nothing — a best-effort sweep that
//! silently skips a file would let the literal through.
//!
//! Scope is STRICT and EXPLICIT: the audit chain's own `target_file`s plus the
//! `docs/tech-debt/*.md` details. Never the whole tree or human-authored docs.
//! Telemetry is `path + redaction count`; the matched value is NEVER logged.
//! Writes are atomic, compare-before-rename, and preserve the original mode.

use std::path::Component;
use std::path::{Path, PathBuf};

use crate::core::redact::redact_for_audit_artifact;

/// Kronn-managed artifacts to sweep: the explicit audit `target_file`s (relative
/// to the project root, e.g. `docs/AGENTS.md`; the synthetic `"REVIEW"` step and
/// empties are skipped) PLUS every `docs/tech-debt/*.md` detail file (created by
/// Step 8, not an individual step target). Absolute paths, de-duplicated.
pub fn managed_targets(
    project_path: &Path,
    step_targets: &[String],
) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for t in step_targets {
        if t.is_empty() || t == "REVIEW" {
            continue;
        }
        let relative = Path::new(t);
        if relative.is_absolute()
            || relative.components().any(|c| {
                matches!(
                    c,
                    Component::CurDir
                        | Component::ParentDir
                        | Component::RootDir
                        | Component::Prefix(_)
                )
            })
        {
            return Err(format!("unsafe audit target outside project scope: {t}"));
        }
        out.push(project_path.join(relative));
    }
    let td_dir = project_path.join("docs/tech-debt");
    refuse_symlink_chain(project_path, &td_dir)?;
    match std::fs::symlink_metadata(&td_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!(
                "{}: tech-debt directory is a symlink — refusing to follow",
                td_dir.display()
            ));
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(format!("{}: expected a directory", td_dir.display()));
        }
        Ok(_) => {
            let rd = std::fs::read_dir(&td_dir)
                .map_err(|e| format!("{}: read_dir failed: {e}", td_dir.display()))?;
            for entry in rd {
                let entry = entry
                    .map_err(|e| format!("{}: directory entry failed: {e}", td_dir.display()))?;
                let p = entry.path();
                if p.extension().and_then(|x| x.to_str()) == Some("md") {
                    out.push(p);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: cannot stat: {e}", td_dir.display())),
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn refuse_symlink_chain(project_path: &Path, path: &Path) -> Result<(), String> {
    let relative = path.strip_prefix(project_path).map_err(|_| {
        format!(
            "{} escapes project root {}",
            path.display(),
            project_path.display()
        )
    })?;
    let mut cursor = project_path.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "{} contains an unsafe path component",
                path.display()
            ));
        }
        cursor.push(component.as_os_str());
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{}: symlink in managed artifact path — refusing to follow",
                    cursor.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(format!(
                    "{}: cannot inspect path component: {e}",
                    cursor.display()
                ))
            }
        }
    }
    Ok(())
}

/// Redact one artifact in place, FAIL-CLOSED. `Ok(count)` = redactions applied
/// (0 = already clean / absent). `Err` = we could NOT guarantee the file is
/// clean (symlink, unreadable, unwritable) — caller must abort.
///
/// Refuses symlinks (never follow a planted link out of scope), preserves the
/// original file mode (a docs file stays `0644`, not the temp's `0600`), and
/// uses compare-before-rename so a concurrent edit is never silently clobbered.
fn sanitize_file(project_path: &Path, path: &Path, boundary: &str) -> Result<usize, String> {
    refuse_symlink_chain(project_path, path)?;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        // A target that doesn't exist yet (a step hasn't written it) is clean.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: cannot stat: {e}", path.display())),
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{}: audit artifact is a symlink — refusing to follow (fail-closed)",
            path.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!(
            "{}: managed audit artifact is not a regular file",
            path.display()
        ));
    }

    let original =
        std::fs::read(path).map_err(|e| format!("{}: read failed: {e}", path.display()))?;
    let content = std::str::from_utf8(&original).map_err(|_| {
        format!(
            "{}: managed audit artifact is not valid UTF-8",
            path.display()
        )
    })?;
    let (redacted, count) = redact_for_audit_artifact(content);
    // Content equality is the sole authority for deciding whether to write.
    // A telemetry-count bug must never be able to leave changed (and therefore
    // secret-bearing) content on disk.
    if redacted == content {
        debug_assert_eq!(count, 0, "unchanged audit artifact reported redactions");
        return Ok(0);
    }
    let count = count.max(1);

    // Compare-before-rename: only replace if still byte-identical to what we read.
    crate::core::mcp_scanner::atomic_write_if_unchanged_preserving_permissions(
        path, &redacted, &original,
    )
    .map_err(|e| format!("{}: atomic write failed: {e}", path.display()))?;

    // kronn::invariant: a secret leak into an artifact is a breach worth
    // surfacing — path + count ONLY, never the value.
    tracing::warn!(
        target: "kronn::invariant",
        boundary, path = %path.display(), redactions = count,
        "redact: masked secret literal(s) in audit artifact"
    );
    Ok(count)
}

/// Sanitize an explicit set of artifacts, FAIL-CLOSED. Returns the modified
/// `(path, count)` list on success, or `Err` on the FIRST file we cannot
/// guarantee clean — the caller must then abort the run and publish nothing.
pub fn sanitize_files(
    project_path: &Path,
    files: &[PathBuf],
    boundary: &str,
) -> Result<Vec<(String, usize)>, String> {
    let mut report = Vec::new();
    for path in files {
        let count = sanitize_file(project_path, path, boundary)?;
        if count > 0 {
            let relative = path.strip_prefix(project_path).unwrap_or(path);
            report.push((relative.display().to_string(), count));
        }
    }
    Ok(report)
}

/// Convenience: sweep the full managed set for a run (pre-agent + post-loop).
pub fn sanitize_all(
    project_path: &Path,
    step_targets: &[String],
    boundary: &str,
) -> Result<Vec<(String, usize)>, String> {
    let targets = managed_targets(project_path, step_targets)?;
    sanitize_files(project_path, &targets, boundary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mode(p: &Path, s: &str, mode: u32) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    #[test]
    fn masks_literal_preserves_mode_and_leaves_clean_files_and_checksums() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path();
        // A step target (relative path) with a bare secret-assignment literal, 0644.
        let td = proj.join("docs/tech-debt/TD-app-secret.md");
        write_mode(
            &td,
            "Committed `APP_SECRET=61cc954cdeadbeef0123456789abcdef` at .env.dist:7\n",
            0o644,
        );
        // A clean step target — must be untouched.
        let agents = proj.join("docs/AGENTS.md");
        write_mode(&agents, "# Agents\n\nNo secrets here.\n", 0o644);
        let agents_before = std::fs::read_to_string(&agents).unwrap();

        let targets = vec![
            "docs/AGENTS.md".to_string(),
            "docs/tech-debt/TD-app-secret.md".to_string(),
        ];
        let report = sanitize_all(proj, &targets, "test").expect("fail-closed sweep ok");

        assert_eq!(report.len(), 1, "only the TD was redacted: {report:?}");
        let out = std::fs::read_to_string(&td).unwrap();
        assert!(
            !out.contains("61cc954cdeadbeef0123456789abcdef"),
            "value masked"
        );
        // Mode preserved (0644, not the temp 0600).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&td).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "original 0644 must be preserved, got {mode:o}");
        }
        // Clean file untouched.
        assert_eq!(std::fs::read_to_string(&agents).unwrap(), agents_before);
    }

    #[test]
    fn marker_inside_secret_is_still_written_and_reported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path();
        let artifact = proj.join("docs/tech-debt/TD-marker-bypass.md");
        let leak = "prefix***REDACTED***still-secret";
        write_mode(&artifact, &format!("APP_SECRET={leak}\n"), 0o644);

        let report = sanitize_all(
            proj,
            &["docs/tech-debt/TD-marker-bypass.md".to_string()],
            "test-marker-bypass",
        )
        .expect("changed secret-bearing content must be persisted");

        let out = std::fs::read_to_string(&artifact).unwrap();
        assert!(
            !out.contains(leak),
            "literal must not survive on disk: {out}"
        );
        assert_eq!(out, "APP_SECRET=***REDACTED***\n");
        assert_eq!(report.len(), 1, "the changed artifact must be reported");
        assert!(
            report[0].1 > 0,
            "changed output must have positive telemetry"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_artifact_fail_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path();
        let real = proj.join("secret-target.md");
        std::fs::write(&real, "apikey=Ab3xZ9Qw7Lm2Ns5Pt8Rv\n").unwrap();
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let link = proj.join("docs/AGENTS.md");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err = sanitize_files(proj, &[link], "test").unwrap_err();
        assert!(
            err.contains("symlink"),
            "must refuse symlink fail-closed: {err}"
        );
        // The link target was NOT followed/rewritten.
        assert!(std::fs::read_to_string(&real)
            .unwrap()
            .contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"));
    }

    #[test]
    fn missing_target_is_clean_not_an_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A step whose target file hasn't been written yet → Ok, count 0.
        let report = sanitize_all(
            tmp.path(),
            &["docs/decisions.md".to_string(), "REVIEW".to_string()],
            "test",
        )
        .unwrap();
        assert!(report.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_tech_debt_directory_fail_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("docs/tech-debt")).unwrap();

        let err = sanitize_all(tmp.path(), &[], "test").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_tech_debt_parent_before_enumeration() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let outside_docs = outside.path().join("docs");
        let outside_td = outside_docs.join("tech-debt/TD-outside.md");
        std::fs::create_dir_all(outside_td.parent().unwrap()).unwrap();
        std::fs::write(&outside_td, "apikey=Ab3xZ9Qw7Lm2Ns5Pt8Rv\n").unwrap();
        std::os::unix::fs::symlink(&outside_docs, tmp.path().join("docs")).unwrap();

        let err = managed_targets(tmp.path(), &[]).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(
            std::fs::read_to_string(&outside_td)
                .unwrap()
                .contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"),
            "the external artifact must not be touched"
        );
    }

    #[test]
    fn refuses_invalid_utf8_and_non_regular_targets_fail_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let invalid = tmp.path().join("docs/invalid.md");
        std::fs::create_dir_all(invalid.parent().unwrap()).unwrap();
        std::fs::write(&invalid, [0xff, 0xfe, 0xfd]).unwrap();
        let err = sanitize_files(tmp.path(), &[invalid], "test").unwrap_err();
        assert!(err.contains("not valid UTF-8"), "{err}");

        let directory = tmp.path().join("docs/directory.md");
        std::fs::create_dir_all(&directory).unwrap();
        let err = sanitize_files(tmp.path(), &[directory], "test").unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn refuses_targets_that_escape_project_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = sanitize_all(tmp.path(), &["../outside.md".to_string()], "test").unwrap_err();
        assert!(
            err.contains("unsafe audit target outside project scope"),
            "{err}"
        );
    }

    #[test]
    fn refuses_current_directory_components_at_target_validation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = sanitize_all(tmp.path(), &["./docs/AGENTS.md".to_string()], "test").unwrap_err();
        assert!(
            err.contains("unsafe audit target outside project scope"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_permission_helper_refuses_symlink_without_caller_precheck() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("outside.md");
        let link = tmp.path().join("artifact.md");
        let original = b"APP_SECRET=secret-before\n";
        std::fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = crate::core::mcp_scanner::atomic_write_if_unchanged_preserving_permissions(
            &link,
            "APP_SECRET=***REDACTED***\n",
            original,
        )
        .unwrap_err();

        assert!(
            err.contains("symlink"),
            "helper must refuse directly: {err}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    /// ORDERING / ALL-TARGETS invariant (Codex 689#1): the boundary sweep is
    /// keyed off the WHOLE managed surface, not the step that just ran — so a
    /// secret an early step wrote into ANY managed target (its own or another)
    /// is gone before the next step's agent can read it. Simulates the per-step
    /// boundary after "step 1": both step-1's target and an unrelated managed
    /// target are clean afterwards, so no subsequent spawn can observe a literal.
    #[test]
    fn boundary_sweeps_every_managed_target_before_next_spawn() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path();
        let targets = vec![
            "docs/repo-map.md".to_string(), // pretend: the step that just ran
            "docs/glossary.md".to_string(), // pretend: a later step's target
            "docs/AGENTS.md".to_string(),   // pretend: an earlier step's target
        ];
        write_mode(
            &proj.join("docs/repo-map.md"),
            "map with APP_SECRET=61cc954cdeadbeef0123456789abcdef here\n",
            0o644,
        );
        write_mode(
            &proj.join("docs/glossary.md"),
            "term: apikey=Ab3xZ9Qw7Lm2Ns5Pt8Rv\n",
            0o644,
        );
        write_mode(&proj.join("docs/AGENTS.md"), "clean, no secrets\n", 0o644);

        // The per-step boundary sweeps the full target list, not just repo-map.
        let report = sanitize_all(proj, &targets, "per-step").expect("fail-closed sweep ok");

        // BOTH secret-bearing targets are clean before any next spawn could read them.
        assert!(!std::fs::read_to_string(proj.join("docs/repo-map.md"))
            .unwrap()
            .contains("61cc954cdeadbeef0123456789abcdef"));
        assert!(!std::fs::read_to_string(proj.join("docs/glossary.md"))
            .unwrap()
            .contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"));
        // Exactly the two secret-bearing files were reported; the clean one wasn't.
        assert_eq!(
            report.len(),
            2,
            "both tainted managed targets redacted: {report:?}"
        );
        assert!(report.iter().all(|(p, _)| p != "docs/AGENTS.md"));

        // Idempotent: a second sweep finds nothing (the literal is truly gone,
        // not just masked-on-read).
        let again = sanitize_all(proj, &targets, "per-step").expect("ok");
        assert!(again.is_empty(), "re-sweep must be a no-op: {again:?}");
    }
}
