//! Tests for Quick Exec — KT-195.
//!
//! Two families. The first is the security boundary: this module spawns
//! processes, so what it REFUSES is as much the behaviour as what it runs. The
//! second is honesty of the bounded result: a timeout, a cancellation, a missing
//! binary and a truncated log all produce no findings, and none of them may read
//! as a clean run.

use super::*;

fn spec(binary: &str, argv: &[&str], cwd: &Path) -> QuickExecSpec {
    QuickExecSpec {
        binary: binary.to_string(),
        argv: argv.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        timeout_secs: Some(10),
        stdin: None,
        summariser: Summariser::Generic,
    }
}

// ── the allowlist is the boundary ───────────────────────────────────

#[test]
fn a_shell_is_refused_even_when_the_allowlist_contains_it() {
    // The allowlist is a source file and will be edited. This hands `check_binary`
    // an allowlist that DOES contain every shell, so the refusal can only come
    // from the denylist — testing against today's allowlist would pass for the
    // wrong reason and keep passing if the denylist were deleted.
    for denied in DENIED_BINARIES {
        let permissive = [*denied, "cargo"];
        let rejection = check_binary(denied, &permissive)
            .expect_err(&format!("`{denied}` was accepted from an allowlist"));
        assert!(
            rejection.0.contains(denied),
            "the rejection must name the binary so a human can fix the spec"
        );
        assert!(
            rejection.0.contains("never a shell"),
            "`{denied}` was refused, but for the wrong reason: {}",
            rejection.0
        );
    }
}

#[test]
fn every_denied_name_is_refused_through_the_public_entry_point() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    for denied in DENIED_BINARIES {
        assert!(
            validate(&spec(denied, &["-c", "id"], root.path()), &roots).is_err(),
            "`{denied}` was accepted"
        );
    }
}

#[test]
fn no_allowlisted_name_is_also_on_the_denylist() {
    // The two lists live in the same file and will both be edited. If a name
    // ends up on both, the denylist wins at runtime — this asserts we never
    // ship that contradiction in the first place.
    for allowed in ALLOWED_BINARIES {
        assert!(
            !DENIED_BINARIES.contains(allowed),
            "`{allowed}` is allowlisted and denylisted at once"
        );
    }
}

#[test]
fn no_shell_is_allowlisted() {
    // The DoD is "no sh -c". The structural guarantee behind it is that nothing
    // able to interpret a command line of its own is reachable at all.
    for shell in ["sh", "bash", "zsh", "cmd", "powershell", "env", "xargs"] {
        assert!(
            !ALLOWED_BINARIES.contains(&shell),
            "`{shell}` would give Quick Exec a shell"
        );
    }
}

#[test]
fn an_explicit_path_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    // The allowlist is worthless if the caller can point at any executable.
    for path in ["/bin/sh", "./run.sh", "../../bin/sh", "C:\\Windows\\cmd"] {
        assert!(
            validate(&spec(path, &[], root.path()), &roots).is_err(),
            "`{path}` was accepted as a binary"
        );
    }
}

#[test]
fn a_leading_dash_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    assert!(validate(&spec("--version", &[], root.path()), &roots).is_err());
}

#[test]
fn an_unlisted_binary_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let rejection = validate(&spec("curl", &[], root.path()), &roots).expect_err("accepted");
    assert!(rejection.0.contains("allowlist"));
}

// ── the working directory is bounded ────────────────────────────────

#[test]
fn a_cwd_outside_every_root_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    assert!(validate(&spec("echo", &[], elsewhere.path()), &roots).is_err());
}

#[test]
fn a_symlink_pointing_out_of_the_root_is_refused() {
    // A lexical containment check passes here and a canonicalising one does not.
    // This is the case that decides which of the two we use.
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = root.path().join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    #[cfg(not(unix))]
    return;
    let roots = vec![root.path().to_path_buf()];
    let rejection = validate(&spec("echo", &[], &link), &roots).expect_err("symlink accepted");
    assert!(rejection.0.contains("outside"));
}

#[test]
fn a_relative_cwd_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    assert!(validate(&spec("echo", &[], Path::new("backend")), &roots).is_err());
}

#[test]
fn a_caller_with_no_declared_root_gets_nothing() {
    // Fails closed: an empty root list means "no project is attached", not
    // "anywhere is fine".
    let root = tempfile::tempdir().unwrap();
    assert!(validate(&spec("echo", &[], root.path()), &[]).is_err());
}

#[test]
fn a_file_is_not_a_working_directory() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("a.txt");
    std::fs::write(&file, b"x").unwrap();
    let roots = vec![root.path().to_path_buf()];
    assert!(validate(&spec("echo", &[], &file), &roots).is_err());
}

// ── argv and timeout bounds ─────────────────────────────────────────

#[test]
fn a_nul_byte_in_an_argument_is_refused() {
    // exec truncates at the NUL, so the command that runs would not be the
    // command that was reviewed.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    assert!(validate(&spec("echo", &["safe\0--dangerous"], root.path()), &roots).is_err());
}

#[test]
fn too_many_arguments_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let many: Vec<String> = (0..MAX_ARGV + 1).map(|i| i.to_string()).collect();
    let mut s = spec("echo", &[], root.path());
    s.argv = many;
    assert!(validate(&s, &roots).is_err());
}

#[test]
fn a_zero_or_oversized_timeout_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let mut s = spec("echo", &[], root.path());
    s.timeout_secs = Some(0);
    assert!(validate(&s, &roots).is_err());
    s.timeout_secs = Some(MAX_TIMEOUT_SECS + 1);
    assert!(validate(&s, &roots).is_err());
    s.timeout_secs = None;
    assert_eq!(
        validate(&s, &roots).unwrap().timeout_secs,
        DEFAULT_TIMEOUT_SECS
    );
}

// ── argv reaches the process literally ──────────────────────────────

#[tokio::test]
async fn shell_metacharacters_are_passed_as_literal_text() {
    // The proof that no shell is involved: these would be a command separator, a
    // substitution and a glob under `sh -c`, and here they are just characters.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let payload = "a; whoami $(whoami) `whoami` && rm -rf * > /tmp/x";
    let validated = validate(&spec("echo", &[payload], root.path()), &roots).unwrap();
    let artifacts = root.path().join("artifacts");
    let result = run(&validated, Some(&artifacts), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, QuickExecStatus::Passed);
    let log = std::fs::read_to_string(&result.artifact.unwrap().path).unwrap();
    assert!(
        log.contains(payload),
        "the argument did not arrive verbatim: {log}"
    );
    assert!(
        !root.path().join("..").join("x").exists(),
        "a redirection was interpreted"
    );
}

// ── status is never optimistic ──────────────────────────────────────

#[tokio::test]
async fn a_nonzero_exit_is_a_failure_with_its_code() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let validated = validate(&spec("false", &[], root.path()), &roots).unwrap();
    let result = run(&validated, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, QuickExecStatus::Failed);
    assert_eq!(result.exit_code, Some(1));
    assert!(!result.status.is_success());
}

#[tokio::test]
async fn a_zero_exit_is_the_only_pass() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let validated = validate(&spec("true", &[], root.path()), &roots).unwrap();
    let result = run(&validated, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, QuickExecStatus::Passed);
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn a_timeout_is_not_a_pass() {
    // The failure this guards: a long command killed at the deadline reporting
    // no findings, and no findings being read as green.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let mut s = spec("sleep", &["30"], root.path());
    s.timeout_secs = Some(1);
    let validated = validate(&s, &roots).unwrap();
    let result = run(&validated, None, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, QuickExecStatus::TimedOut);
    assert!(!result.status.is_success());
    assert_ne!(result.exit_code, Some(0));
    assert!(result.duration_ms < 20_000, "the child was not killed");
}

#[tokio::test]
async fn a_cancellation_is_reported_as_such() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let validated = validate(&spec("sleep", &["30"], root.path()), &roots).unwrap();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        trigger.cancel();
    });
    let result = run(&validated, None, &cancel).await.unwrap();
    assert_eq!(result.status, QuickExecStatus::Cancelled);
    assert!(!result.status.is_success());
}

#[test]
fn only_passed_counts_as_success() {
    for status in [
        QuickExecStatus::Failed,
        QuickExecStatus::TimedOut,
        QuickExecStatus::Cancelled,
        QuickExecStatus::Rejected,
    ] {
        assert!(!status.is_success(), "{status:?} was treated as success");
    }
    assert!(QuickExecStatus::Passed.is_success());
}

#[test]
fn a_binary_that_never_started_is_rejected_not_passed() {
    let result = spawn_failure("tsc", "No such file or directory", 3);
    assert_eq!(result.status, QuickExecStatus::Rejected);
    assert_eq!(result.exit_code, None);
    assert!(
        !result.findings_complete,
        "nothing ran, so nothing is complete"
    );
    assert!(result.failed_tests.is_empty());
}

// ── stdin is explicit ───────────────────────────────────────────────

#[tokio::test]
async fn stdin_is_delivered_when_given() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    // `git hash-object --stdin` reads stdin and writes a hash — a deterministic
    // proof the bytes arrived, without needing a shell.
    let mut s = spec("git", &["hash-object", "--stdin"], root.path());
    s.stdin = Some("hello".to_string());
    let validated = validate(&s, &roots).unwrap();
    let artifacts = root.path().join("artifacts");
    let result = run(&validated, Some(&artifacts), &CancellationToken::new())
        .await
        .unwrap();
    if result.status == QuickExecStatus::Rejected {
        return; // git absent from this machine; nothing to assert.
    }
    let log = std::fs::read_to_string(&result.artifact.unwrap().path).unwrap();
    assert!(
        log.contains("b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"),
        "stdin did not reach the process: {log}"
    );
}

// ── truncation is announced ─────────────────────────────────────────

#[test]
fn a_partial_stream_says_so_on_the_first_line() {
    // A reader who only sees the head of the summary must still learn the lists
    // are incomplete — which is why the notice is not appended at the end.
    let extracted = summarise(
        Summariser::CargoTest,
        "test result: ok. 12 passed; 0 failed",
        "",
        QuickExecStatus::Passed,
        false,
    );
    assert!(
        extracted.summary.starts_with("PARTIAL OUTPUT"),
        "got: {}",
        extracted.summary
    );
}

#[test]
fn a_complete_stream_carries_no_partial_notice() {
    // Negative side of the previous test: the warning must mean something.
    let extracted = summarise(
        Summariser::CargoTest,
        "test result: ok. 12 passed; 0 failed",
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert!(!extracted.summary.contains("PARTIAL"));
}

#[test]
fn findings_are_only_complete_when_both_streams_were_seen_whole() {
    // Each of the three ways to lose output has to defeat completeness on its
    // own — an empty failed_tests list drawn from any of them is not evidence.
    assert!(findings_are_complete(false, true, true));
    assert!(
        !findings_are_complete(true, true, true),
        "a truncated stream still claimed complete findings"
    );
    assert!(
        !findings_are_complete(false, false, true),
        "stdout never reached EOF and the findings were still called complete"
    );
    assert!(
        !findings_are_complete(false, true, false),
        "stderr never reached EOF and the findings were still called complete"
    );
}

#[test]
fn a_truncated_artifact_is_labelled_in_the_file_itself() {
    // The artifact outlives the result it came with, so the caveat has to travel
    // inside it.
    let dir = tempfile::tempdir().unwrap();
    let reference = write_artifact(dir.path(), b"out", b"err", true).unwrap();
    let body = std::fs::read_to_string(&reference.path).unwrap();
    assert!(body.contains("TRUNCATED"));
    assert!(body.contains("out") && body.contains("err"));
    assert!(reference.truncated);
}

#[test]
fn the_number_of_named_failures_is_capped() {
    let noise: String = (0..5_000)
        .map(|i| format!("test long::name::{i} ... FAILED\n"))
        .collect();
    let extracted = summarise(
        Summariser::CargoTest,
        &noise,
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert_eq!(extracted.failed_tests.len(), MAX_FAILED_TESTS);
}

#[test]
fn the_summary_is_capped_in_bytes() {
    // Long lines, not many lines: the per-item caps already bound the count, so
    // only a byte cap bounds a summary made of a few enormous entries. Sized well
    // past SUMMARY_MAX_BYTES so the assertion cannot pass by accident.
    let long_lines: String = (0..20)
        .map(|i| format!("{} {}\n", "x".repeat(2_000), i))
        .collect();
    let extracted = summarise(
        Summariser::Generic,
        "",
        &long_lines,
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        extracted.summary.len() <= SUMMARY_MAX_BYTES + 32,
        "summary was {} bytes",
        extracted.summary.len()
    );
    assert!(extracted.summary.contains("truncated"));
}

#[test]
fn truncation_never_splits_a_character() {
    // Cutting mid-character would leave the summary invalid where a UI reads it.
    let text = "é".repeat(100);
    let cut = truncate_on_char_boundary(text, 51);
    assert!(cut.starts_with(&"é".repeat(25)));
    assert!(cut.contains("truncated"));
}

// ── summarisers ─────────────────────────────────────────────────────

#[test]
fn cargo_test_failures_are_named() {
    let extracted = summarise(
        Summariser::CargoTest,
        "test db::a::works ... ok\ntest db::b::breaks ... FAILED\ntest result: FAILED. 1 passed; 1 failed",
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert_eq!(extracted.failed_tests, vec!["db::b::breaks"]);
    assert!(extracted.summary.contains("test result: FAILED"));
}

#[test]
fn clippy_diagnostics_keep_their_location() {
    let extracted = summarise(
        Summariser::Clippy,
        "",
        "warning: unused variable: `x`\n  --> src/a.rs:12:9\n   |\nerror: this is unreachable\n  --> src/b.rs:40:1\n",
        QuickExecStatus::Failed,
        true,
    );
    assert_eq!(extracted.diagnostics.len(), 2);
    assert_eq!(extracted.diagnostics[0].path.as_deref(), Some("src/a.rs"));
    assert_eq!(extracted.diagnostics[0].line, Some(12));
    assert_eq!(extracted.diagnostics[1].path.as_deref(), Some("src/b.rs"));
}

#[test]
fn tsc_errors_are_parsed_from_the_paren_form() {
    let extracted = summarise(
        Summariser::Tsc,
        "src/App.tsx(42,17): error TS2339: Property 'foo' does not exist.\n",
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert_eq!(extracted.diagnostics.len(), 1);
    assert_eq!(
        extracted.diagnostics[0].path.as_deref(),
        Some("src/App.tsx")
    );
    assert_eq!(extracted.diagnostics[0].line, Some(42));
    assert!(extracted.diagnostics[0].message.contains("TS2339"));
}

#[test]
fn vitest_failures_are_named_for_each_marker() {
    let extracted = summarise(
        Summariser::Vitest,
        " FAIL  src/a.test.ts > renders\n × src/b.test.ts > throws\n ✓ src/c.test.ts > ok\n",
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert_eq!(extracted.failed_tests.len(), 2);
    assert!(extracted.failed_tests[0].contains("src/a.test.ts"));
}

#[test]
fn the_generic_summariser_prefers_stderr_but_falls_back_to_stdout() {
    let with_stderr = summarise(
        Summariser::Generic,
        "noise",
        "the actual error",
        QuickExecStatus::Failed,
        true,
    );
    assert!(with_stderr.summary.contains("the actual error"));
    let without = summarise(
        Summariser::Generic,
        "only stdout",
        "  \n",
        QuickExecStatus::Passed,
        true,
    );
    assert!(without.summary.contains("only stdout"));
}

// ── idempotency key ─────────────────────────────────────────────────

#[test]
fn the_fingerprint_is_stable_and_covers_what_changes_the_work() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let base = validate(&spec("cargo", &["test", "--lib"], root.path()), &roots).unwrap();
    assert_eq!(spec_fingerprint(&base), spec_fingerprint(&base));

    let other_argv = validate(&spec("cargo", &["test", "--all"], root.path()), &roots).unwrap();
    assert_ne!(spec_fingerprint(&base), spec_fingerprint(&other_argv));

    let nested = root.path().join("sub");
    std::fs::create_dir(&nested).unwrap();
    let other_cwd = validate(&spec("cargo", &["test", "--lib"], &nested), &roots).unwrap();
    assert_ne!(
        spec_fingerprint(&base),
        spec_fingerprint(&other_cwd),
        "the same command in another directory is another run"
    );
}

#[test]
fn a_longer_timeout_is_the_same_work() {
    // Retrying because the deadline was too short must hit the same key, or the
    // idempotency check would treat it as a new run and execute twice.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let mut short = spec("cargo", &["test"], root.path());
    short.timeout_secs = Some(30);
    let mut long = short.clone();
    long.timeout_secs = Some(600);
    assert_eq!(
        spec_fingerprint(&validate(&short, &roots).unwrap()),
        spec_fingerprint(&validate(&long, &roots).unwrap())
    );
}

#[test]
fn argument_boundaries_are_part_of_the_identity() {
    // `["a b"]` and `["a", "b"]` are different commands. A fingerprint that
    // joined them would call two different runs the same.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let joined = validate(&spec("echo", &["a b"], root.path()), &roots).unwrap();
    let split = validate(&spec("echo", &["a", "b"], root.path()), &roots).unwrap();
    assert_ne!(spec_fingerprint(&joined), spec_fingerprint(&split));
}

// ── retention ───────────────────────────────────────────────────────

#[test]
fn pruning_removes_the_oldest_until_the_directory_fits() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..5 {
        let path = dir.path().join(format!("{index}.log"));
        std::fs::write(&path, vec![b'x'; 1_000]).unwrap();
        // Distinct mtimes, oldest first — the order the prune must follow.
        let when = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(1_700_000_000 + index * 60);
        filetime_set(&path, when);
    }
    let removed = prune_artifacts(dir.path(), 2_500).unwrap();
    assert_eq!(removed, 3);
    assert!(!dir.path().join("0.log").exists());
    assert!(dir.path().join("4.log").exists(), "the newest was deleted");
}

#[test]
fn pruning_a_directory_under_the_cap_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.log"), b"small").unwrap();
    assert_eq!(prune_artifacts(dir.path(), 1_000).unwrap(), 0);
    assert!(dir.path().join("a.log").exists());
}

#[test]
fn pruning_a_missing_directory_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(prune_artifacts(&dir.path().join("absent"), 10).unwrap(), 0);
}

/// Set an mtime without pulling in a crate for it.
fn filetime_set(path: &Path, when: std::time::SystemTime) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(when).unwrap();
}
