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

/// KT-1438: a wrapper (`make`) prints the real failure on stdout and only its
/// own verdict on stderr; the tail alone sent a worker back with `Error 1`.
fn wrapped_phpunit_run() -> (String, String) {
    let mut stdout = String::from("PHPUnit 10.5\n");
    stdout.push_str("Twig\\Error\\LoaderError: Unable to find template \"@web/images/vector/menu-icons/icon-hr-language.svg\" (looked into: /app/public)\n");
    for index in 0..200 {
        stdout.push_str(&format!(
            "#{index} /app/vendor/twig/twig/src/Environment.php({index})\n"
        ));
    }
    stdout.push_str("Tests: 11, Assertions: 4, Errors: 7.\n");
    let stderr = "WARNING: The requested image's platform (linux/amd64) does not match\n\
                  make: *** [iso-test-class] Error 1\n"
        .to_string();
    (stdout, stderr)
}

#[test]
fn a_red_generic_run_keeps_its_first_error_lines_not_only_its_tail() {
    let (stdout, stderr) = wrapped_phpunit_run();
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        &stderr,
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        extracted.summary.contains("Unable to find template"),
        "the first error must survive: {}",
        extracted.summary
    );
    assert!(extracted.summary.contains("icon-hr-language.svg"));
    assert!(
        extracted.summary.contains("Error 1"),
        "the tail is kept too: {}",
        extracted.summary
    );
    assert!(extracted.summary.len() <= SUMMARY_MAX_BYTES);
}

#[test]
fn a_passing_generic_run_has_no_first_errors_section() {
    let extracted = summarise(
        Summariser::Generic,
        "0 errors, 0 failed\nall good\n",
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert!(!extracted.summary.contains("first errors"));
    assert!(extracted.summary.contains("all good"));
}

#[test]
fn a_count_of_zero_errors_is_not_a_first_error() {
    let extracted = summarise(
        Summariser::Generic,
        "build: 0 errors\n",
        "boom\n",
        QuickExecStatus::Failed,
        true,
    );
    assert!(!extracted.summary.contains("first errors"));
    assert!(extracted.summary.contains("boom"));
}

#[test]
fn the_first_error_section_cannot_push_the_tail_out_of_the_summary() {
    let stdout: String = (0..400)
        .map(|index| format!("error: noisy failure number {index} {}\n", "x".repeat(200)))
        .collect();
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        "the last line says make: Error 1\n",
        QuickExecStatus::Failed,
        true,
    );
    assert!(extracted.summary.contains("first errors"));
    assert!(extracted.summary.contains("noisy failure number 0 "));
    assert!(
        extracted.summary.contains("make: Error 1"),
        "tail lost: {}",
        extracted.summary
    );
}

/// KT-1438: error lines are where connection strings and tokens appear, and the
/// first errors leave to the room and the worker.
#[test]
fn the_first_errors_of_a_red_run_are_redacted() {
    let stdout =
        "PDOException: could not connect to postgres://app:Sup3rS3cretPw@db.internal:5432/kronn\n\
                  fatal: Authorization: Bearer abc123def456ghi789jkl was rejected\n\
                  APP_SECRET=61cc0ffee1234abcd5678 error while booting\n"
            .to_string();
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        "make: *** [test] Error 1\n",
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        extracted.summary.contains("first errors"),
        "{}",
        extracted.summary
    );
    assert!(extracted
        .summary
        .contains("could not connect to postgres://app:"));
    for secret in [
        "Sup3rS3cretPw",
        "abc123def456ghi789jkl",
        "61cc0ffee1234abcd5678",
    ] {
        assert!(
            !extracted.summary.contains(secret),
            "{secret} leaked: {}",
            extracted.summary
        );
    }
}

// ── KT-2035: why a red test run failed ──────────────────────────────

/// A red `cargo test` run as libtest prints it: the failure blocks between the
/// two `failures:` headings, then the verdict.
fn red_cargo_test_run(failing: usize, panic_line: &str) -> String {
    let mut stdout = String::from("\nrunning 3 tests\n");
    for index in 0..failing {
        stdout.push_str(&format!(
            "test core::preflight::case_{index}_is_refused ... FAILED\n"
        ));
    }
    stdout.push_str("test core::preflight::passes ... ok\n\nfailures:\n\n");
    for index in 0..failing {
        stdout.push_str(&format!(
            "---- core::preflight::case_{index}_is_refused stdout ----\n\
             \n\
             thread 'core::preflight::case_{index}_is_refused' panicked at src/core/preflight.rs:{line}:9:\n\
             {panic_line} {index}\n\
             assertion `left == right` failed\n\
             \x20 left: Refused(\"connection {index}\")\n\
             \x20right: Callable\n\
             note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\
             \n\n",
            line = 100 + index,
        ));
    }
    stdout.push_str("\nfailures:\n");
    for index in 0..failing {
        stdout.push_str(&format!("    core::preflight::case_{index}_is_refused\n"));
    }
    stdout.push_str(&format!(
        "\ntest result: FAILED. 1 passed; {failing} failed; 0 ignored; 0 measured; 900 filtered out; finished in 0.02s\n"
    ));
    stdout
}

#[test]
fn a_red_cargo_test_run_keeps_the_panic_message_of_its_failing_test() {
    let stdout = red_cargo_test_run(1, "the connection was not callable");
    let extracted = summarise(
        Summariser::CargoTest,
        &stdout,
        "error: test failed, to rerun pass `--lib`\n",
        QuickExecStatus::Failed,
        true,
    );
    let summary = &extracted.summary;
    assert_eq!(
        extracted.failed_tests,
        vec!["core::preflight::case_0_is_refused"]
    );
    for expected in [
        "panicked at src/core/preflight.rs:100:9",
        "the connection was not callable 0",
        "left: Refused(\"connection 0\")",
        "right: Callable",
        "test result: FAILED",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    assert!(
        !summary.contains("RUST_BACKTRACE"),
        "the backtrace hint is noise: {summary}"
    );
    assert!(summary.len() <= SUMMARY_MAX_BYTES);
}

#[test]
fn many_red_cargo_tests_keep_the_first_panics_within_the_summary_cap() {
    let stdout = red_cargo_test_run(MAX_FAILED_TESTS + 20, &"why it failed ".repeat(40));
    let extracted = summarise(
        Summariser::CargoTest,
        &stdout,
        "",
        QuickExecStatus::Failed,
        true,
    );
    let summary = &extracted.summary;
    assert!(summary.len() <= SUMMARY_MAX_BYTES, "{}", summary.len());
    assert!(
        summary.contains("panicked at src/core/preflight.rs:100:9"),
        "the first panic must survive the failing names: {summary}"
    );
    assert!(summary.contains("why it failed"), "{summary}");
    assert!(summary.contains("test result: FAILED"), "{summary}");
    assert_eq!(extracted.failed_tests.len(), MAX_FAILED_TESTS);
}

/// `cargo nextest` (or any runner without libtest's `---- name stdout ----`
/// blocks) still keeps the panic and the lines under it.
#[test]
fn a_red_rust_run_without_libtest_blocks_keeps_its_panic() {
    let stderr = "    FAIL [   0.004s] kronn core::preflight::refused\n\
                  --- STDERR:              kronn core::preflight::refused ---\n\
                  thread 'core::preflight::refused' panicked at src/core/preflight.rs:12:5:\n\
                  assertion `left == right` failed\n\
                  \x20 left: 1\n\
                  \x20right: 2\n\
                  \n\
                  error: test run failed\n";
    let extracted = summarise(
        Summariser::CargoTest,
        "",
        stderr,
        QuickExecStatus::Failed,
        true,
    );
    let summary = &extracted.summary;
    assert!(
        summary.contains("panicked at src/core/preflight.rs:12:5"),
        "{summary}"
    );
    assert!(summary.contains("left: 1"), "{summary}");
    assert!(summary.contains("right: 2"), "{summary}");
}

#[test]
fn the_kept_panic_lines_are_redacted() {
    let stdout = red_cargo_test_run(
        1,
        "could not connect to postgres://app:Sup3rS3cretPw@db.internal:5432/kronn with Authorization: Bearer abc123def456ghi789jkl",
    );
    let extracted = summarise(
        Summariser::CargoTest,
        &stdout,
        "",
        QuickExecStatus::Failed,
        true,
    );
    let summary = &extracted.summary;
    assert!(
        summary.contains("could not connect to postgres://app:"),
        "{summary}"
    );
    for secret in ["Sup3rS3cretPw", "abc123def456ghi789jkl"] {
        assert!(!summary.contains(secret), "{secret} leaked: {summary}");
    }
}

#[test]
fn a_green_cargo_test_summary_is_unchanged() {
    let stdout = "\nrunning 2 tests\ntest a ... ok\ntest b ... ok\n\n\
                  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    let extracted = summarise(
        Summariser::CargoTest,
        stdout,
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert_eq!(
        extracted.summary,
        "Passed\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
    );
}

/// A red vitest run as the default reporter prints it.
fn red_vitest_run(secret: &str) -> String {
    format!(
        " ❯ src/a.test.ts (2 tests | 1 failed) 4ms\n\
         \x20  × adds 2ms\n\
         \x20  ✓ subtracts 1ms\n\
         \n\
         ⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯\n\
         \n\
         \x20FAIL  src/a.test.ts > adds\n\
         AssertionError: expected 3 to be 4 // Object.is equality {secret}\n\
         \n\
         - Expected\n\
         + Received\n\
         \n\
         - 4\n\
         + 3\n\
         \n\
         \x20❯ src/a.test.ts:4:15\n\
         \x20     4|   expect(1 + 2).toBe(4)\n\
         \n\
         ⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯[1/1]⎯\n\
         \n\
         \x20Test Files  1 failed (1)\n\
         \x20     Tests  1 failed | 1 passed (2)\n"
    )
}

#[test]
fn a_red_vitest_run_keeps_its_failure_message() {
    let extracted = summarise(
        Summariser::Vitest,
        &red_vitest_run("token=ghp_0123456789abcdefghijABCDEFGHIJ012345"),
        "",
        QuickExecStatus::Failed,
        true,
    );
    let summary = &extracted.summary;
    assert!(
        summary.contains("AssertionError: expected 3 to be 4"),
        "{summary}"
    );
    assert!(summary.contains("src/a.test.ts:4:15"), "{summary}");
    assert!(
        !summary.contains("ghp_0123456789abcdefghijABCDEFGHIJ012345"),
        "leaked: {summary}"
    );
    assert!(summary.len() <= SUMMARY_MAX_BYTES);
}

#[test]
fn a_green_vitest_summary_is_unchanged() {
    let extracted = summarise(
        Summariser::Vitest,
        " ✓ src/a.test.ts (2 tests) 3ms\n\n Test Files  1 passed (1)\n      Tests  2 passed (2)\n",
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert_eq!(extracted.summary, "Passed\n");
}

// ── KT-2039: no secret shape leaves a red test run ──────────────────

/// The shapes `redact_stored_text` used to miss, as a failing test prints
/// them, each with the fake secret it carries.
const KT_2039_SHAPES: &[(&str, &str)] = &[
    (
        r#"runtime: ExternalHttpRuntime { model: "m", api_key: Some("Fake1Some2Value3Abc") }"#,
        "Fake1Some2Value3Abc",
    ),
    (
        "config: Config {\n    model: \"m\",\n    password: Some(\n        \"Fake4Pretty5Value6\",\n    ),\n}",
        "Fake4Pretty5Value6",
    ),
    (
        "headers: { 'x-api-key': 'Fake7Quoted8Key9', accept: 'application/json' }",
        "Fake7Quoted8Key9",
    ),
    (
        r#"key: ApiKey { id: "k1", provider: "nvidia", value: "nvapi-Fake0Nvidia1Key2Value3Abcdef4", active: true }"#,
        "Fake0Nvidia1Key2",
    ),
    (
        "could not reach rediss://default:Fake5Redis6Pass@cache.internal:6380/0",
        "Fake5Redis6Pass",
    ),
];

fn assert_no_kt_2039_secret(extracted: &Extracted, secret: &str, shape: &str) {
    let summary = &extracted.summary;
    assert!(
        !summary.contains(secret),
        "{secret} leaked from {shape:?}: {summary}"
    );
    assert!(
        summary.contains("***REDACTED***"),
        "the shape must be kept, masked, not cut away: {summary}"
    );
    for name in &extracted.failed_tests {
        assert!(!name.contains(secret), "{secret} leaked in a name: {name}");
    }
}

#[test]
fn a_red_cargo_test_run_stores_no_kt_2039_secret() {
    for (shape, secret) in KT_2039_SHAPES {
        let extracted = summarise(
            Summariser::CargoTest,
            &red_cargo_test_run(1, shape),
            "",
            QuickExecStatus::Failed,
            true,
        );
        assert_no_kt_2039_secret(&extracted, secret, shape);
        assert!(
            extracted
                .summary
                .contains("panicked at src/core/preflight.rs:100:9"),
            "{}",
            extracted.summary
        );
    }
}

/// The same shapes in what a failing test printed on its stdout, in the
/// `assert_eq!` sides of its panic, and under `cargo nextest`.
#[test]
fn a_red_cargo_test_run_masks_the_shapes_in_captured_output_and_assertions() {
    for (shape, secret) in KT_2039_SHAPES {
        let stdout = format!(
            "\nrunning 1 test\ntest kt::case ... FAILED\n\nfailures:\n\n\
             ---- kt::case stdout ----\n\
             {shape}\n\
             thread 'kt::case' panicked at src/kt.rs:3:5:\n\
             assertion `left == right` failed\n\
             \x20 left: {shape}\n\
             \x20right: None\n\n\
             failures:\n    kt::case\n\n\
             test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n"
        );
        let extracted = summarise(
            Summariser::CargoTest,
            &stdout,
            "",
            QuickExecStatus::Failed,
            true,
        );
        assert_no_kt_2039_secret(&extracted, secret, shape);

        let nextest = format!(
            "thread 'kt::case' panicked at src/kt.rs:3:5:\n{shape}\n\nerror: test run failed\n"
        );
        let extracted = summarise(
            Summariser::CargoTest,
            "",
            &nextest,
            QuickExecStatus::Failed,
            true,
        );
        assert_no_kt_2039_secret(&extracted, secret, shape);
    }
}

#[test]
fn a_red_vitest_run_stores_no_kt_2039_secret() {
    for (shape, secret) in KT_2039_SHAPES {
        let extracted = summarise(
            Summariser::Vitest,
            &red_vitest_run(shape),
            "",
            QuickExecStatus::Failed,
            true,
        );
        assert_no_kt_2039_secret(&extracted, secret, shape);
        assert!(
            extracted
                .summary
                .contains("AssertionError: expected 3 to be 4"),
            "{}",
            extracted.summary
        );
    }
}

/// A vitest test name is free text, and so is a Generic tail and a collector's
/// stderr: the summary is redacted whole, and so are the failing names.
#[test]
fn every_part_of_a_summary_is_redacted() {
    let name_secret = "ghp_Fake0123456789abcdefghijABCDEFGHIJ";
    let vitest = format!(
        " FAIL  src/a.test.ts > posts with token={name_secret}\n\
         \x20Test Files  1 failed (1)\n"
    );
    let extracted = summarise(
        Summariser::Vitest,
        &vitest,
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        !extracted.summary.contains("Fake0123456789"),
        "{}",
        extracted.summary
    );
    assert_eq!(extracted.failed_tests.len(), 1);
    assert!(
        !extracted.failed_tests[0].contains("Fake0123456789"),
        "{:?}",
        extracted.failed_tests
    );

    let cargo = format!("test kt::{name_secret} ... FAILED\n");
    let extracted = summarise(
        Summariser::CargoTest,
        &cargo,
        "",
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        !extracted.summary.contains("Fake0123456789"),
        "{}",
        extracted.summary
    );

    for (shape, secret) in KT_2039_SHAPES {
        let stdout = format!("starting\n{shape}\ndone\n");
        let extracted = summarise(
            Summariser::Generic,
            &stdout,
            "",
            QuickExecStatus::Passed,
            true,
        );
        assert!(
            !extracted.summary.contains(secret),
            "{secret} leaked from the generic tail: {}",
            extracted.summary
        );

        let extracted = summarise(
            Summariser::Collected,
            "{}",
            shape,
            QuickExecStatus::Failed,
            true,
        );
        assert!(
            !extracted.summary.contains(secret),
            "{secret} leaked from collected stderr: {}",
            extracted.summary
        );
    }
}

/// The generic tail keeps the END of its lines: cut right after a secret's
/// name, the value left behind is no assignment any more, so the tail is
/// redacted before it is cut.
#[test]
fn the_generic_tail_is_redacted_before_its_cut() {
    let first = "auth failed: password=Fake9Tail0Value1";
    let at = first.find("Fake9").unwrap();
    let mut lines = vec![first.to_string()];
    for index in 0..18 {
        lines.push(format!("filler {index:02} {}", "x".repeat(80)));
    }
    let used: usize = lines.iter().map(|line| line.len() + 1).sum();
    // The twentieth line is sized so that the cap cuts the first one right
    // before its value.
    lines.push("y".repeat(TAIL_MAX_BYTES + at - used));
    let stdout = lines.join("\n");
    assert_eq!(stdout.len() - TAIL_MAX_BYTES, at);
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert!(
        !extracted.summary.contains("Fake9Tail0Value1"),
        "{}",
        extracted.summary
    );
}

// ── KT-2044: no line window separates a name from its value ─────────

/// `{:#?}` prints the name of an `Option` field and its value on two lines.
const PRETTY_SECRET: &str = "Fake3Window4Value5";

fn pretty_config(value: &str) -> String {
    format!("Config {{\n    password: Some(\n        \"{value}\",\n    ),\n")
}

/// The generic tail keeps the last 20 non-empty lines: here the value line is
/// the first of them and the `password: Some(` line the one just before it.
#[test]
fn a_pretty_debug_name_sliced_off_by_the_generic_tail_leaves_no_value() {
    let mut stdout = pretty_config(PRETTY_SECRET);
    // The value line, its `),` and 18 more: 20 lines.
    for index in 0..18 {
        stdout.push_str(&format!("    field_{index:02}: {index},\n"));
    }
    let tail: Vec<&str> = stdout
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(20)
        .collect();
    assert!(tail.last().unwrap().contains(PRETTY_SECRET), "{tail:?}");
    assert!(!tail.iter().any(|line| line.contains("password")));
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        "",
        QuickExecStatus::Passed,
        true,
    );
    assert!(
        !extracted.summary.contains(PRETTY_SECRET),
        "{}",
        extracted.summary
    );
    assert!(
        extracted.summary.contains("\"***REDACTED***\","),
        "{}",
        extracted.summary
    );
}

/// The first error lines of a red run are picked one by one: a value line that
/// holds a marker word is picked without the name line above it.
#[test]
fn a_pretty_debug_value_picked_as_a_first_error_line_is_redacted() {
    let secret = "Fake6Error7Value8";
    let stdout = format!(
        "booting\n{}}}\nboot failed\n",
        pretty_config(&format!("{secret}-error"))
    );
    let extracted = summarise(
        Summariser::Generic,
        &stdout,
        "make: *** [boot] Error 1\n",
        QuickExecStatus::Failed,
        true,
    );
    assert!(
        extracted.summary.contains("first errors"),
        "{}",
        extracted.summary
    );
    assert!(!extracted.summary.contains(secret), "{}", extracted.summary);
}

/// A failure block keeps a `{:#?}` struct whole or cuts the value away: never
/// the value without its name.
#[test]
fn a_pretty_debug_secret_in_a_failure_block_is_redacted() {
    let secret = "Fake9Block0Value1";
    let shape = format!("{}}}", pretty_config(secret));
    for (summariser, stdout) in [
        (Summariser::CargoTest, red_cargo_test_run(1, &shape)),
        (Summariser::Vitest, red_vitest_run(&shape)),
    ] {
        let extracted = summarise(summariser, &stdout, "", QuickExecStatus::Failed, true);
        assert!(
            extracted.summary.contains("failure details"),
            "{}",
            extracted.summary
        );
        assert!(!extracted.summary.contains(secret), "{}", extracted.summary);
    }
}

/// A stream cut at the cap ends mid-line: a token cut below its pattern's
/// minimum length is no longer recognised, so that line is dropped before
/// extraction rather than summarised.
#[tokio::test]
async fn a_stream_cut_at_the_cap_drops_its_last_partial_line() {
    let token = "ghp_Fake0123456789abcdefghijABCDEFGHIJ";
    let stream = format!("first line\nthread panicked: bad credential {token}\nlast\n");
    // A cap that falls inside the token, as the GitHub shape needs 30 characters.
    let cap = stream.find(token).unwrap() + 11;
    let (kept, total) = drain_capped(stream.as_bytes(), cap).await;
    assert!(
        (kept.len() as u64) < total,
        "a cut stream reads as truncated"
    );
    let text = whole_lines(&kept);
    assert_eq!(text, "first line\n");
    let extracted = summarise(
        Summariser::Generic,
        &text,
        "",
        QuickExecStatus::Failed,
        false,
    );
    assert!(
        !extracted.summary.contains("ghp_Fake"),
        "{}",
        extracted.summary
    );

    // A stream kept whole is read as it is, its last line included.
    let (kept, total) = drain_capped(stream.as_bytes(), 1 << 20).await;
    assert_eq!(kept.len() as u64, total);
    assert_eq!(whole_lines(&kept), stream);
    let unterminated = "no newline at the end";
    let (kept, _) = drain_capped(unterminated.as_bytes(), 1 << 20).await;
    assert_eq!(whole_lines(&kept), unterminated);
    // A capped stream of a single line keeps nothing of it.
    let (kept, _) = drain_capped(unterminated.as_bytes(), 5).await;
    assert_eq!(whole_lines(&kept), "");
}

/// KT-2116 — a stream longer than the cap keeps whole lines of its start and of
/// its end, with one line saying what is missing, and never more than the cap.
#[tokio::test]
async fn a_stream_over_the_cap_keeps_its_start_and_its_end() {
    let lines: Vec<String> = (0..2_000).map(|n| format!("line {n:04}\n")).collect();
    let stream = lines.concat();
    let cap = 4_096;
    let (kept, total) = drain_capped(stream.as_bytes(), cap).await;
    assert_eq!(total, stream.len() as u64);
    assert!(kept.len() <= cap, "{} bytes kept", kept.len());
    let text = whole_lines(&kept);
    assert!(text.starts_with("line 0000\nline 0001\n"), "{text}");
    assert!(text.ends_with("line 1998\nline 1999\n"), "{text}");
    let markers: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("line "))
        .collect();
    assert_eq!(markers.len(), 1, "{markers:?}");
    assert!(markers[0].ends_with(" bytes not kept …"), "{markers:?}");
    for line in text.lines().filter(|line| line.starts_with("line ")) {
        assert_eq!(
            line.len(),
            "line 0000".len(),
            "a cut line was kept: {line:?}"
        );
    }
    let omitted: u64 = markers[0]
        .trim_start_matches("… ")
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(omitted + (text.len() - markers[0].len() - 1) as u64, total);
}

/// KT-2116 — the output of a whole-workspace run exceeds the capture: the tests
/// that failed late and the run's Summary line still reach the summary.
#[tokio::test]
async fn a_red_nextest_run_longer_than_the_capture_still_names_its_late_failure() {
    let mut stream = String::from("    Starting 3000 tests across 9 binaries\n");
    for n in 0..2_998 {
        stream.push_str(&format!(
            "        PASS [   0.010s] ({:>4}/3000) kronn-l1 core::sample::tests::case_{n:04}\n",
            n + 1
        ));
    }
    stream.push_str(
        "        FAIL [   0.020s] (2999/3000) kronn-l1 core::sample::tests::the_late_one\n\
         \x20 stderr ───\n\
         \n\
         \x20   thread 'core::sample::tests::the_late_one' panicked at src/core/sample.rs:9:5:\n\
         \x20   late assertion failed\n\
         \n\
         \x20       PASS [   0.010s] (3000/3000) kronn-l1 core::sample::tests::case_2999\n\
         ────────────\n\
         \x20    Summary [  12.000s] 3000 tests run: 2999 passed, 1 failed, 0 skipped\n\
         \x20       FAIL [   0.020s] (2999/3000) kronn-l1 core::sample::tests::the_late_one\n\
         error: test run failed\n",
    );
    let cap = 16_384;
    assert!(stream.len() > 10 * cap);
    let (kept, total) = drain_capped(stream.as_bytes(), cap).await;
    assert!((kept.len() as u64) < total);
    let extracted = summarise(
        Summariser::CargoNextest,
        "",
        &whole_lines(&kept),
        QuickExecStatus::Failed,
        false,
    );
    assert!(
        extracted
            .failed_tests
            .iter()
            .any(|name| name.contains("the_late_one")),
        "{:?}",
        extracted.failed_tests
    );
    assert!(
        extracted
            .summary
            .contains("3000 tests run: 2999 passed, 1 failed"),
        "{}",
        extracted.summary
    );
    assert!(
        extracted.summary.contains("late assertion failed"),
        "{}",
        extracted.summary
    );
}

/// KT-2039 with KT-2116 — a credential cut where the kept end begins is dropped
/// with its line, like one cut where the kept start ends.
#[tokio::test]
async fn a_credential_cut_where_the_kept_end_begins_is_not_kept() {
    let token = "ghp_Fake0123456789abcdefghijABCDEFGHIJ";
    let cap = 1_024;
    let filler = "x".repeat(60);
    let mut stream = String::new();
    for _ in 0..40 {
        stream.push_str(&filler);
        stream.push('\n');
    }
    // The kept end starts inside this line, wherever the cut falls in it.
    let long_line = format!("bad credential {} {token}\n", "y".repeat(cap));
    stream.push_str(&long_line);
    stream.push_str("the end\n");
    let (kept, _) = drain_capped(stream.as_bytes(), cap).await;
    let text = whole_lines(&kept);
    assert!(text.ends_with("the end\n"), "{text}");
    assert!(!text.contains("ghp_"), "{text}");
    assert!(!text.contains("yyyy"), "{text}");
}

// ── KT-2040: a red `cargo nextest` run says which tests failed and why ──

/// A nextest status line: the status right-aligned on 12 columns, the
/// duration, then `<binary id> <test>`.
fn nextest_status(status: &str, seconds: &str, name: &str) -> String {
    format!("{status:>12} [{seconds:>8}s] {name}")
}

/// One failed test as cargo-nextest prints it: the status line, then each
/// captured stream, indented, under its `stdout ───` / `stderr ───` header.
/// The stdout is libtest's report of the one test the process ran.
fn nextest_failure(status: &str, name: &str, stderr: &str) -> String {
    let test = name.rsplit(' ').next().unwrap();
    let indent = |text: &str| -> String {
        text.lines()
            .map(|line| {
                if line.is_empty() {
                    "\n".to_string()
                } else {
                    format!("    {line}\n")
                }
            })
            .collect()
    };
    let libtest = format!(
        "\nrunning 1 test\ntest {test} ... FAILED\n\nfailures:\n\nfailures:\n    {test}\n\n\
         test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s\n\n"
    );
    format!(
        "{}\n  stdout ───\n{}  stderr ───\n{}\n",
        nextest_status(status, "0.004", name),
        indent(&libtest),
        indent(stderr),
    )
}

/// The lines nextest prints before its first test.
const NEXTEST_HEADER: &[&str] = &[
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.31s",
    "────────────",
    " Nextest run ID 7f1c1c0e-6a52-4d0f-9d3c-0c5b6a1f2e11 with nextest profile: default",
    "    Starting 4 tests across 2 binaries (1 test skipped)",
];

/// A whole red run on stderr: passes around the failures, the `Summary` line,
/// then nextest's closing recap of the failed tests.
fn red_nextest_run(failures: &[String], summary: &str) -> String {
    let mut stderr = String::new();
    for line in NEXTEST_HEADER {
        stderr.push_str(line);
        stderr.push('\n');
    }
    let pass = |name: &str| format!("{}\n", nextest_status("PASS", "0.012", name));
    stderr.push_str(&pass("my-crate::bin/my-tool cli::parses_flags"));
    for failure in failures {
        stderr.push_str(failure);
    }
    stderr.push_str(&pass("kronn::it core::preflight::accepted"));
    stderr.push_str("────────────\n");
    stderr.push_str(&format!("     Summary [   0.035s] {summary}\n"));
    for failure in failures {
        stderr.push_str(failure.lines().next().unwrap());
        stderr.push('\n');
    }
    stderr.push_str("error: test run failed\n");
    stderr
}

const NEXTEST_PANIC: &str =
    "thread 'core::preflight::refused' panicked at src/core/preflight.rs:12:5:\n\
     assertion `left == right` failed\n\
     \x20 left: 1\n\
     \x20right: 2\n\
     note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";

fn nextest_summary_of(stderr: &str, status: QuickExecStatus) -> Extracted {
    summarise(Summariser::CargoNextest, "", stderr, status, true)
}

/// One panic among passes: the `Summary` line, the failed test named once
/// (nextest prints it live and in its recap) and the panic, without libtest's
/// one-test report or the stream headers.
#[test]
fn a_red_nextest_run_names_the_failed_test_and_keeps_its_panic() {
    let name = "kronn::it core::preflight::refused";
    let stderr = red_nextest_run(
        &[nextest_failure("FAIL", name, NEXTEST_PANIC)],
        "3 tests run: 2 passed, 1 failed, 1 skipped",
    );
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests, vec![name]);
    assert!(
        summary.starts_with(
            "Failed — 1 failing\nfailure details:\n        FAIL [   0.004s] kronn::it core::preflight::refused\n"
        ),
        "{summary}"
    );
    for expected in [
        "panicked at src/core/preflight.rs:12:5",
        "assertion `left == right` failed",
        "left: 1",
        "right: 2",
        "Summary [   0.035s] 3 tests run: 2 passed, 1 failed, 1 skipped",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    for noise in [
        "RUST_BACKTRACE",
        "running 1 test",
        "test result:",
        "failures:",
        "stderr ───",
        "stdout ───",
    ] {
        assert!(!summary.contains(noise), "{noise} is noise: {summary}");
    }
    assert!(summary.len() <= SUMMARY_MAX_BYTES);
}

/// A timed-out test and an aborted one are named with their status and keep
/// their output, in the `--- STDERR: <binary> <test> ---` layout too.
#[test]
fn a_nextest_timeout_and_an_abort_are_named_with_their_status() {
    let slow = "kronn::it api::slow::never_returns";
    let abort = "my-crate::bin/my-tool ffi::double_free";
    let stderr = [
        "    Starting 3 tests across 2 binaries".to_string(),
        nextest_status("TIMEOUT", "300.012", slow),
        String::new(),
        format!("--- STDOUT:              {slow} ---"),
        String::new(),
        "running 1 test".to_string(),
        String::new(),
        format!("--- STDERR:              {slow} ---"),
        "waiting for the server on 127.0.0.1:4010".to_string(),
        String::new(),
        nextest_status("SIGABRT", "0.153", abort),
        String::new(),
        format!("--- STDOUT:              {abort} ---"),
        String::new(),
        "running 1 test".to_string(),
        String::new(),
        format!("--- STDERR:              {abort} ---"),
        "thread 'ffi::double_free' panicked at src/ffi.rs:40:9:".to_string(),
        "panic in a function that cannot unwind".to_string(),
        "thread caused non-unwinding panic. aborting.".to_string(),
        String::new(),
        nextest_status("PASS", "0.020", "kronn::it core::preflight::accepted"),
        "------------".to_string(),
        "     Summary [ 300.214s] 3 tests run: 1 passed, 1 failed, 1 timed out, 0 skipped"
            .to_string(),
        nextest_status("TIMEOUT", "300.012", slow),
        nextest_status("SIGABRT", "0.153", abort),
        "error: test run failed".to_string(),
    ]
    .join("\n");
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(
        extracted.failed_tests,
        vec![format!("{slow} (TIMEOUT)"), format!("{abort} (SIGABRT)")]
    );
    for expected in [
        "Failed — 2 failing",
        "     TIMEOUT [ 300.012s] kronn::it api::slow::never_returns",
        "waiting for the server on 127.0.0.1:4010",
        "     SIGABRT [   0.153s] my-crate::bin/my-tool ffi::double_free",
        "panicked at src/ffi.rs:40:9",
        "panic in a function that cannot unwind",
        "Summary [ 300.214s] 3 tests run: 1 passed, 1 failed, 1 timed out, 0 skipped",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    for noise in ["--- STDERR", "--- STDOUT", "running 1 test"] {
        assert!(!summary.contains(noise), "{noise} is noise: {summary}");
    }
}

/// A compile error that stops the run before any test keeps the compiler's
/// error lines, as a `cargo test` run does.
#[test]
fn a_nextest_run_stopped_by_a_compile_error_keeps_the_error() {
    let stderr = "   Compiling kronn v0.15.4 (/work/backend)\n\
                  error[E0308]: mismatched types\n\
                  \x20 --> src/core/preflight.rs:12:18\n\
                  \x20  |\n\
                  12 |     let attempts: u32 = \"one\";\n\
                  \x20  |                   ---   ^^^^^ expected `u32`, found `&str`\n\
                  \n\
                  error: could not compile `kronn` (lib test) due to 1 previous error\n\
                  error: command `cargo test --no-run --message-format json-render-diagnostics` exited with code 101\n";
    let extracted = nextest_summary_of(stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert!(extracted.failed_tests.is_empty(), "{summary}");
    assert!(summary.starts_with("Failed\n"), "{summary}");
    for expected in [
        "error[E0308]: mismatched types",
        "error: could not compile `kronn` (lib test) due to 1 previous error",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
}

/// A green run is its `Summary` line: a test that passed while leaking a
/// handle is not a failure.
#[test]
fn a_green_nextest_summary_is_its_summary_line() {
    let stderr = [
        "    Starting 3 tests across 2 binaries".to_string(),
        nextest_status("PASS", "0.012", "my-crate::bin/my-tool cli::parses_flags"),
        nextest_status("LEAK", "0.120", "kronn::it api::server::spawns_a_child"),
        nextest_status("PASS", "0.020", "kronn::it core::preflight::accepted"),
        "────────────".to_string(),
        "     Summary [   0.152s] 3 tests run: 3 passed (1 leaky), 0 skipped".to_string(),
    ]
    .join("\n");
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Passed);
    assert_eq!(
        extracted.summary,
        "Passed\nSummary [   0.152s] 3 tests run: 3 passed (1 leaky), 0 skipped\n"
    );
    assert!(extracted.failed_tests.is_empty());
}

/// More failures than the cap: the names are capped and unique, the first
/// panics and the `Summary` line stay within the summary cap.
#[test]
fn many_red_nextest_tests_are_named_once_and_capped() {
    let failing = MAX_FAILED_TESTS + 20;
    let failures: Vec<String> = (0..failing)
        .map(|index| {
            nextest_failure(
                "FAIL",
                &format!("kronn::it core::preflight::case_{index}_is_refused"),
                &format!(
                    "thread 'core::preflight::case_{index}_is_refused' panicked at src/core/preflight.rs:{}:9:\n{} {index}\n",
                    100 + index,
                    "why it failed ".repeat(40),
                ),
            )
        })
        .collect();
    let counts = format!(
        "{} tests run: 2 passed, {failing} failed, 0 skipped",
        failing + 2
    );
    let stderr = red_nextest_run(&failures, &counts);
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests.len(), MAX_FAILED_TESTS);
    assert_eq!(
        extracted.failed_tests[0],
        "kronn::it core::preflight::case_0_is_refused"
    );
    let unique: std::collections::HashSet<&String> = extracted.failed_tests.iter().collect();
    assert_eq!(
        unique.len(),
        MAX_FAILED_TESTS,
        "{:?}",
        extracted.failed_tests
    );
    assert!(summary.len() <= SUMMARY_MAX_BYTES, "{}", summary.len());
    let summary_line = format!("Summary [   0.035s] {counts}");
    for expected in [
        "panicked at src/core/preflight.rs:100:9",
        "why it failed",
        summary_line.as_str(),
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
}

/// The kept output of a failed nextest test goes through the redaction of
/// the other failure blocks: a connection string, a bearer, the KT-2039 shapes.
#[test]
fn the_kept_nextest_failure_output_is_redacted() {
    let run = |panic_line: &str| {
        red_nextest_run(
            &[nextest_failure(
                "FAIL",
                "kronn::it db::pool::connects",
                &format!(
                    "thread 'db::pool::connects' panicked at src/db/pool.rs:31:9:\n{panic_line}\n"
                ),
            )],
            "3 tests run: 2 passed, 1 failed, 1 skipped",
        )
    };
    let extracted = nextest_summary_of(
        &run("could not connect to postgres://app:Sup3rS3cretPw@db.internal:5432/kronn with Authorization: Bearer abc123def456ghi789jkl"),
        QuickExecStatus::Failed,
    );
    let summary = &extracted.summary;
    assert!(
        summary.contains("could not connect to postgres://app:"),
        "{summary}"
    );
    for secret in ["Sup3rS3cretPw", "abc123def456ghi789jkl"] {
        assert!(!summary.contains(secret), "{secret} leaked: {summary}");
    }
    for (shape, secret) in KT_2039_SHAPES {
        let extracted = nextest_summary_of(&run(shape), QuickExecStatus::Failed);
        assert_no_kt_2039_secret(&extracted, secret, shape);
        assert!(
            extracted
                .summary
                .contains("panicked at src/db/pool.rs:31:9"),
            "{}",
            extracted.summary
        );
    }
}

fn summariser_of(command: &str) -> Summariser {
    let mut parts = command.split_whitespace();
    let binary = parts.next().unwrap();
    let argv: Vec<&str> = parts.collect();
    Summariser::for_command(binary, &argv)
}

/// The command lines that select the nextest summariser, and those that keep
/// the one they had: `cargo test`, clippy, tsc, vitest, a wrapper.
#[test]
fn a_cargo_nextest_command_selects_the_nextest_summariser() {
    for command in [
        "cargo nextest run",
        "cargo nextest run --manifest-path backend/Cargo.toml --workspace --partition slice:1/6",
        "cargo --locked nextest run --profile ci",
        "cargo llvm-cov nextest --lib",
        // A test filter named after the other subcommand.
        "cargo nextest run clippy",
        // A global option's value is not the subcommand.
        "cargo --config net.offline=true nextest run",
        "cargo +nightly -Z unstable-options -C backend nextest run",
        // No test runs: read as any command when red (see `for_command`).
        "cargo nextest list",
        "cargo nextest archive --archive-file tests.tar.zst",
    ] {
        assert_eq!(
            summariser_of(command),
            Summariser::CargoNextest,
            "{command}"
        );
    }
    for (command, expected) in [
        ("cargo test", Summariser::CargoTest),
        (
            "cargo test --manifest-path backend/Cargo.toml --lib preflight",
            Summariser::CargoTest,
        ),
        // `nextest` inside a longer word is a filter, not the subcommand.
        ("cargo test --lib nextest_summary", Summariser::CargoTest),
        // So is the exact word after the subcommand: a filter, a test target,
        // a package, the value of an option.
        ("cargo test --lib nextest", Summariser::CargoTest),
        (
            "cargo test --manifest-path backend/Cargo.toml --lib nextest",
            Summariser::CargoTest,
        ),
        ("cargo test -- nextest", Summariser::CargoTest),
        ("cargo test --test nextest", Summariser::CargoTest),
        ("cargo test -p nextest", Summariser::CargoTest),
        ("cargo --color nextest test", Summariser::CargoTest),
        ("cargo test clippy", Summariser::CargoTest),
        ("cargo llvm-cov --lib nextest", Summariser::CargoTest),
        ("cargo +nightly --color never clippy", Summariser::Clippy),
        ("cargo", Summariser::CargoTest),
        ("cargo check", Summariser::CargoTest),
        (
            "cargo clippy --all-targets -- -D warnings",
            Summariser::Clippy,
        ),
        ("cargo clippy -p nextest", Summariser::Clippy),
        ("tsc --noEmit", Summariser::Tsc),
        ("vitest run", Summariser::Vitest),
        ("make test", Summariser::Generic),
        // As for `cargo test`, a wrapper is summarised by its own name.
        ("rtk cargo nextest run", Summariser::Generic),
    ] {
        assert_eq!(summariser_of(command), expected, "{command}");
    }
}

/// With retries, a test that passed on a later try is not a failure; one that
/// failed every try is named once.
#[test]
fn a_nextest_test_that_passed_on_a_retry_is_not_named_as_failing() {
    let flaky = "kronn::it api::ws::reconnects";
    let broken = "kronn::it core::preflight::refused";
    let stderr = [
        "    Starting 2 tests across 1 binary".to_string(),
        nextest_status("TRY 1 FAIL", "0.004", flaky),
        nextest_status("TRY 1 FAIL", "0.004", broken),
        nextest_status("FLAKY 2/3", "0.005", flaky),
        nextest_status("TRY 2 FAIL", "0.004", broken),
        nextest_status("TRY 3 FAIL", "0.004", broken),
        "────────────".to_string(),
        "     Summary [   0.035s] 2 tests run: 1 passed (1 flaky), 1 failed, 0 skipped".to_string(),
        nextest_status("FLAKY 2/3", "0.005", flaky),
        nextest_status("TRY 3 FAIL", "0.004", broken),
        "error: test run failed".to_string(),
    ]
    .join("\n");
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    assert_eq!(extracted.failed_tests, vec![broken]);
}

/// A profile that prints the failures' output only after the `Summary` line
/// (`failure-output = "final"`) still gives the panic.
#[test]
fn a_nextest_failure_printed_after_the_summary_keeps_its_panic() {
    let name = "kronn::it core::preflight::refused";
    let mut stderr = String::from("    Starting 2 tests across 1 binary\n");
    stderr.push_str(&format!("{}\n", nextest_status("FAIL", "0.004", name)));
    stderr.push_str("────────────\n");
    stderr.push_str("     Summary [   0.035s] 2 tests run: 1 passed, 1 failed, 0 skipped\n");
    stderr.push_str(&nextest_failure("FAIL", name, NEXTEST_PANIC));
    stderr.push_str("error: test run failed\n");
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests, vec![name]);
    assert!(
        summary.contains("panicked at src/core/preflight.rs:12:5"),
        "{summary}"
    );
    assert!(summary.contains("right: 2"), "{summary}");
    assert!(!summary.contains("error: test run failed"), "{summary}");
}

/// A failed test that logged more lines than a block keeps still shows its
/// panic: the log gives way to it.
#[test]
fn a_chatty_failed_nextest_test_still_shows_its_panic() {
    let mut output: String = (0..40)
        .map(|attempt| format!("retrying the connection, attempt {attempt}\n"))
        .collect();
    output.push_str(NEXTEST_PANIC);
    let stderr = red_nextest_run(
        &[nextest_failure(
            "FAIL",
            "kronn::it core::preflight::refused",
            &output,
        )],
        "3 tests run: 2 passed, 1 failed, 1 skipped",
    );
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    for expected in [
        "        FAIL [   0.004s] kronn::it core::preflight::refused",
        "panicked at src/core/preflight.rs:12:5",
        "left: 1",
        "right: 2",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
}

/// The status lines nextest prints are told from a test's own output: the
/// label is right-aligned on 12 columns, a counter may precede the name.
#[test]
fn a_nextest_status_line_is_read_with_its_label_and_name() {
    for (line, expected) in [
        (
            "        FAIL [   1.000s] my-binary-id test1",
            Some(("FAIL", "my-binary-id test1")),
        ),
        (
            "   FLAKY 2/2 [   2.000s] [3/3] (  20/5000) my-binary-id test1",
            Some(("FLAKY 2/2", "my-binary-id test1")),
        ),
        (
            "        SLOW [> 30.000s] kronn::it api::slow::never_returns",
            Some(("SLOW", "kronn::it api::slow::never_returns")),
        ),
        // A retried try: the short status (a signal without `SIG`), the
        // counter padded; a label over 12 columns starts at the margin.
        (
            "  TRY 3 SEGV [   0.153s] (─────) my-crate::bin/my-tool ffi::double_free",
            Some(("TRY 3 SEGV", "my-crate::bin/my-tool ffi::double_free")),
        ),
        (
            "TRY 10 LKFAIL [   0.153s] my-crate::bin/my-tool ffi::double_free",
            Some(("TRY 10 LKFAIL", "my-crate::bin/my-tool ffi::double_free")),
        ),
        (
            "     Summary [   2.000s] 5 tests run: 5 passed, 0 skipped",
            None,
        ),
        // A test's own output: not aligned, or no duration.
        ("FAIL [   1.000s] my-binary-id test1", None),
        ("    FAIL [   1.000s] my-binary-id test1", None),
        ("   LOG LEVEL [debug] handlers ready", None),
        ("    assertion failed: [1, 2] == [1, 3]", None),
    ] {
        assert_eq!(nextest_status_line(line), expected, "{line}");
    }
}

/// A red run its summariser reads nothing from is not left as the bare
/// verdict: it keeps the redacted end of its output. A green one is unchanged.
#[test]
fn a_red_run_its_summariser_reads_nothing_from_keeps_the_end_of_its_output() {
    let stderr = "error: no such command: `nextest`\n\n\
                  \tView all installed commands with `cargo --list`\n\
                  \tlast login used password=Fake2Tail3Value4\n";
    for summariser in [
        Summariser::CargoNextest,
        Summariser::CargoTest,
        Summariser::Clippy,
        Summariser::Tsc,
        Summariser::Vitest,
    ] {
        let extracted = summarise(summariser, "", stderr, QuickExecStatus::Failed, true);
        let summary = &extracted.summary;
        assert!(summary.starts_with("Failed\n"), "{summariser:?}: {summary}");
        assert!(
            summary.contains("error: no such command: `nextest`"),
            "{summariser:?}: {summary}"
        );
        assert!(
            !summary.contains("Fake2Tail3Value4"),
            "{summariser:?}: {summary}"
        );
        let green = summarise(summariser, "", stderr, QuickExecStatus::Passed, true);
        assert_eq!(green.summary, "Passed\n", "{summariser:?}");
    }
}

// ── KT-2040 review: codeless compile errors, retries, leaks, setup scripts ──

/// A nextest build stopped by an error without a code (a lint denied by
/// `-D warnings`) keeps each error and the `-->` location under it, not only
/// cargo's `could not compile`.
#[test]
fn a_nextest_build_stopped_by_a_codeless_compile_error_keeps_the_error_and_its_location() {
    let mut stderr = String::from("   Compiling kronn v0.15.4 (/work/backend)\n");
    for index in 0..4 {
        let line = 12 + index;
        stderr.push_str(&format!(
            "error: unused variable: `attempts_{index}`\n\
             \x20 --> src/core/preflight.rs:{line}:9\n\
             \x20  |\n\
             {line} |     let attempts_{index} = 3;\n\
             \x20  |         ^^^^^^^^^^ help: if this is intentional, prefix it with an underscore: `_attempts_{index}`\n"
        ));
        if index == 0 {
            stderr.push_str(
                "\x20  |\n\
                 \x20  = note: `-D unused-variables` implied by `-D warnings`\n\
                 \x20  = help: to override `-D warnings` add `#[allow(unused_variables)]`\n",
            );
        }
        stderr.push('\n');
    }
    stderr.push_str(
        "error: could not compile `kronn` (lib test) due to 4 previous errors\n\
         error: command `cargo test --no-run --message-format json-render-diagnostics` exited with code 101\n",
    );
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert!(extracted.failed_tests.is_empty(), "{summary}");
    assert!(summary.starts_with("Failed\n"), "{summary}");
    // The first error is above the kept end of the output: it is there too.
    for index in 0..4 {
        let error = format!(
            "error: unused variable: `attempts_{index}`\n--> src/core/preflight.rs:{}:9",
            12 + index
        );
        assert!(summary.contains(&error), "{error} missing: {summary}");
    }
    assert!(
        summary.contains("error: could not compile `kronn` (lib test) due to 4 previous errors"),
        "{summary}"
    );
    assert!(summary.len() <= SUMMARY_MAX_BYTES, "{}", summary.len());
}

/// With retries nextest prints a signal as its bare name (`TRY 3 ABRT`, not
/// `SIGABRT`): the crashing test is named once with that status and keeps the
/// output of one try.
#[test]
fn a_nextest_test_that_crashes_on_every_try_is_named_with_its_status() {
    let name = "my-crate::bin/my-tool ffi::double_free";
    let attempt = |label: &str, seconds: &str, counter: &str| {
        format!(
            "{}\n\
             \x20 stdout ───\n\
             \n\
             \x20   running 1 test\n\
             \x20 stderr ───\n\
             \n\
             \x20   thread 'ffi::double_free' panicked at src/ffi.rs:40:9:\n\
             \x20   panic in a function that cannot unwind\n\
             \x20   thread caused non-unwinding panic. aborting.\n\
             \n\
             \x20   (test aborted with signal 6: SIGABRT)\n\
             \n",
            nextest_status(label, seconds, &format!("{counter} {name}"))
        )
    };
    let last = nextest_status("TRY 3 ABRT", "0.151", &format!("(1/2) {name}"));
    let stderr = [
        "────────────\n".to_string(),
        " Nextest run ID 7f1c1c0e-6a52-4d0f-9d3c-0c5b6a1f2e11 with nextest profile: ci\n"
            .to_string(),
        "    Starting 2 tests across 1 binary (12 tests skipped)\n".to_string(),
        attempt("TRY 1 ABRT", "0.153", "(───)"),
        attempt("TRY 2 ABRT", "0.150", "(───)"),
        attempt("TRY 3 ABRT", "0.151", "(1/2)"),
        format!(
            "{}\n",
            nextest_status(
                "PASS",
                "0.012",
                "(2/2) my-crate::bin/my-tool cli::parses_flags"
            )
        ),
        "────────────\n".to_string(),
        "     Summary [   0.600s] 2 tests run: 1 passed, 1 failed, 12 skipped\n".to_string(),
        format!("{last}\n"),
        "error: test run failed\n".to_string(),
    ]
    .concat();
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests, vec![format!("{name} (ABRT)")]);
    assert!(
        summary.starts_with(&format!(
            "Failed — 1 failing\nfailure details:\n  TRY 1 ABRT [   0.153s] (───) {name}\n"
        )),
        "{summary}"
    );
    for expected in [
        "panic in a function that cannot unwind",
        "(test aborted with signal 6: SIGABRT)",
        "Summary [   0.600s] 2 tests run: 1 passed, 1 failed, 12 skipped",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    assert_eq!(
        summary.matches("panicked at src/ffi.rs:40:9").count(),
        1,
        "one try's output is enough: {summary}"
    );

    // Every short status of a failed try names its test; a try that is slow,
    // being terminated, or that passed does not.
    let test = "kronn::it api::slow::never_returns";
    for (label, seconds, named) in [
        ("TRY 3 SEGV", "0.153", Some("SEGV")),
        ("TRY 2 KILL", "0.153", Some("KILL")),
        ("TRY 2 SIG 42", "0.153", Some("SIG 42")),
        ("TRY 2 ABORT", "0.153", Some("ABORT")),
        ("TRY 2 FL+LK", "0.153", Some("FL+LK")),
        ("TRY 2 LKFAIL", "0.153", Some("LKFAIL")),
        ("TRY 2 XFAIL", "0.153", Some("XFAIL")),
        ("TRY 2 TMT", "60.004", Some("TIMEOUT")),
        ("TRY 2 SLOW", "> 30.000", None),
        ("TRY 1 TRMNTG", "> 60.000", None),
        ("TRY 2 PASS", "0.153", None),
    ] {
        let stderr = nextest_status(label, seconds, &format!("(───) {test}"));
        let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
        let expected: Vec<String> = named
            .iter()
            .map(|status| format!("{test} ({status})"))
            .collect();
        assert_eq!(extracted.failed_tests, expected, "{label}");
    }
}

/// A test that passed while leaking a handle (`LEAK`) is a pass: a red run
/// neither counts nor names it, and its count agrees with the `Summary` line.
#[test]
fn a_leaked_nextest_test_is_not_a_failure_of_a_red_run() {
    let name = "kronn::it core::preflight::refused";
    let status = |label: &str, name: &str| format!("{}\n", nextest_status(label, "0.120", name));
    let header: String = NEXTEST_HEADER
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    let counts = "4 tests run: 3 passed (2 leaky), 1 failed, 1 skipped";
    let stderr = [
        header.clone(),
        status("LEAK", "(1/4) kronn::it api::server::spawns_a_child"),
        nextest_failure("FAIL", &format!("(2/4) {name}"), NEXTEST_PANIC),
        status("LEAK", "(3/4) kronn::it api::server::spawns_a_watcher"),
        status("PASS", "(4/4) kronn::it core::preflight::accepted"),
        "────────────\n".to_string(),
        format!("     Summary [   0.152s] {counts}\n"),
        status("FAIL", &format!("(2/4) {name}")),
        "error: test run failed\n".to_string(),
    ]
    .concat();
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests, vec![name]);
    assert!(summary.starts_with("Failed — 1 failing\n"), "{summary}");
    assert!(summary.contains(counts), "{summary}");
    assert!(
        summary.contains("panicked at src/core/preflight.rs:12:5"),
        "{summary}"
    );
    for leaked in ["LEAK", "spawns_a_child", "spawns_a_watcher"] {
        assert!(!summary.contains(leaked), "{leaked} is a pass: {summary}");
    }

    // Red with nothing but leaks: no test is named, the `Summary` line stays.
    let counts = "2 tests run: 2 passed (1 leaky), 3 skipped";
    let stderr = [
        header,
        status("LEAK", "(1/2) kronn::it api::server::spawns_a_child"),
        status("PASS", "(2/2) kronn::it core::preflight::accepted"),
        "────────────\n".to_string(),
        format!("     Summary [   0.152s] {counts}\n"),
    ]
    .concat();
    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert!(extracted.failed_tests.is_empty(), "{summary}");
    assert!(summary.starts_with("Failed\n"), "{summary}");
    assert_eq!(summary.matches(counts).count(), 1, "{summary}");
}

/// With retries, a test that failed a try and then passed takes no failure
/// block: the details go to the test that finally failed, once.
#[test]
fn a_flaky_nextest_test_leaves_the_failure_details_to_the_real_failure() {
    let broken = "kronn::it core::preflight::refused";
    let flaky = [
        "api::ws::reconnects",
        "api::sse::resumes",
        "api::poll::retries",
    ];
    let mut stderr = String::from("    Starting 4 tests across 1 binary\n");
    for (index, test) in flaky.iter().enumerate() {
        stderr.push_str(&nextest_failure(
            "TRY 1 FAIL",
            &format!("(───) kronn::it {test}"),
            &format!("thread '{test}' panicked at src/api/stream.rs:8{index}:9:\nthe peer closed the stream early\n"),
        ));
        stderr.push_str(&format!(
            "{}\n",
            nextest_status(
                "TRY 2 PASS",
                "0.005",
                &format!("({}/4) kronn::it {test}", index + 1)
            )
        ));
    }
    for (label, counter) in [
        ("TRY 1 FAIL", "(───)"),
        ("TRY 2 FAIL", "(───)"),
        ("TRY 3 FAIL", "(4/4)"),
    ] {
        stderr.push_str(&nextest_failure(
            label,
            &format!("{counter} {broken}"),
            NEXTEST_PANIC,
        ));
    }
    stderr.push_str("────────────\n");
    stderr.push_str(
        "     Summary [   0.035s] 4 tests run: 3 passed (3 flaky), 1 failed, 0 skipped\n",
    );
    for (index, test) in flaky.iter().enumerate() {
        stderr.push_str(&format!(
            "{}\n",
            nextest_status(
                "FLAKY 2/3",
                "0.005",
                &format!("({}/4) kronn::it {test}", index + 1)
            )
        ));
    }
    stderr.push_str(&format!(
        "{}\n",
        nextest_status("TRY 3 FAIL", "0.004", &format!("(4/4) {broken}"))
    ));
    stderr.push_str("error: test run failed\n");

    let extracted = nextest_summary_of(&stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(extracted.failed_tests, vec![broken]);
    assert!(
        summary.starts_with(&format!(
            "Failed — 1 failing\nfailure details:\n  TRY 1 FAIL [   0.004s] (───) {broken}\n"
        )),
        "{summary}"
    );
    assert_eq!(
        summary
            .matches("panicked at src/core/preflight.rs:12:5")
            .count(),
        1,
        "{summary}"
    );
    assert!(summary.contains("right: 2"), "{summary}");
    for recovered in ["the peer closed the stream early", "api::ws", "api::sse"] {
        assert!(
            !summary.contains(recovered),
            "{recovered} passed on a retry: {summary}"
        );
    }
}

/// A setup script that failed (`SETUP FAIL`) is named with that status and
/// keeps what it printed.
#[test]
fn a_failed_nextest_setup_script_is_named_with_its_status() {
    let stderr = "────────────\n\
                  \x20Nextest run ID 7f1c1c0e-6a52-4d0f-9d3c-0c5b6a1f2e11 with nextest profile: ci\n\
                  \x20      SETUP [      1/1] services: scripts/start-services.sh\n\
                  \x20 SETUP FAIL [   0.412s] services: scripts/start-services.sh\n\
                  \x20 stderr ───\n\
                  \x20   the database did not come up after 30 s\n\
                  \n\
                  \x20   (script failed with exit code 2)\n\
                  \n\
                  \x20 Cancelling due to setup script failure\n";
    let extracted = nextest_summary_of(stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(
        extracted.failed_tests,
        vec!["services: scripts/start-services.sh (SETUP FAIL)"]
    );
    for expected in [
        "Failed — 1 failing",
        "the database did not come up after 30 s",
        "(script failed with exit code 2)",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    assert!(!summary.contains("Cancelling"), "{summary}");
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

// ── a persisted command line is checked when it is written ──────────

#[test]
fn a_command_line_written_for_a_shell_is_refused_with_the_form_that_runs() {
    // KT-830/KT-828/KT-847: these were accepted at launch and refused only at
    // integration, after the worker had delivered and the principal approved.
    for (line, culprit) in [
        ("cd frontend && npx tsc -b --pretty false", "`cd`"),
        ("cd backend && CARGO_TARGET_DIR=/tmp/t cargo test", "`cd`"),
        (
            "CARGO_TARGET_DIR=/tmp/t cargo test",
            "environment assignment",
        ),
        ("cargo test && cargo clippy", "`&&`"),
        ("cargo test || true", "`||`"),
        ("cargo test | tee out.txt", "pipe"),
        ("cargo test; cargo clippy", "separator"),
        ("cargo test > out.txt", "redirection"),
        ("cargo test 2>&1", "redirection"),
        ("cargo test &", "background"),
        ("git log $(git rev-parse HEAD)", "substitution"),
        ("npx tsc -b", "`npx`"),
        ("bash -c true", "`bash`"),
        ("./node_modules/.bin/tsc", "path"),
    ] {
        let rejection = check_command_line(line)
            .expect_err(&format!("`{line}` was accepted but can never run"));
        assert!(
            rejection.0.contains(culprit),
            "`{line}` was refused for the wrong reason (wanted {culprit}): {}",
            rejection.0
        );
        assert!(
            rejection.0.contains("pnpm --dir frontend exec")
                && rejection.0.contains("cargo test --manifest-path"),
            "`{line}`: the refusal must give the form that runs: {}",
            rejection.0
        );
    }
}

#[test]
fn a_command_line_the_runner_can_spawn_is_accepted() {
    for line in [
        "cargo test --manifest-path backend/Cargo.toml --target-dir /tmp/kronn-target",
        "pnpm --dir frontend exec tsc -b --pretty false",
        "rtk cargo clippy --all-targets",
        "git diff --check",
        "python3 scripts/check.py --flag=a<b",
        "  true  ",
    ] {
        check_command_line(line)
            .unwrap_or_else(|rejection| panic!("`{line}` was refused: {rejection}"));
    }
}

#[test]
fn an_empty_command_line_is_refused() {
    assert!(check_command_line("").is_err());
    assert!(check_command_line("   ").is_err());
}

#[test]
fn what_check_command_line_accepts_is_what_validate_accepts() {
    // The two must never drift: `check_command_line` is a promise about the run.
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    for line in [
        "cargo test --manifest-path backend/Cargo.toml",
        "pnpm --dir frontend exec vitest run",
        "curl https://example.com",
        "sh -c true",
        "/bin/true",
    ] {
        let mut words = line.split_whitespace();
        let binary = words.next().unwrap();
        let argv: Vec<&str> = words.collect();
        assert_eq!(
            check_command_line(line).is_ok(),
            validate(&spec(binary, &argv, root.path()), &roots).is_ok(),
            "`{line}`"
        );
    }
}

#[test]
fn a_quick_exec_child_carries_a_built_environment() {
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let validated = validate(&spec("git", &["status"], root.path()), &roots).unwrap();
    let command = crate::core::child_env::with_parent_env(
        &[
            ("PATH", "/usr/bin:/bin"),
            ("KRONN_AUTH_TOKEN", "admin"),
            ("KRONN_ENCRYPTION_KEK", "raw"),
            ("OPENAI_API_KEY", "sk"),
            ("GH_TOKEN", "ghp"),
        ],
        || build_command(&validated, None, Priority::Normal),
    );
    let env: Vec<String> = command
        .as_std()
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    assert!(env.contains(&"PATH".to_string()));
    for gone in [
        "KRONN_AUTH_TOKEN",
        "KRONN_ENCRYPTION_KEK",
        "OPENAI_API_KEY",
        "GH_TOKEN",
    ] {
        assert!(
            !env.contains(&gone.to_string()),
            "{gone} reached Quick Exec"
        );
    }
}

/// KT-1633: a validation starts behind the host's priority launcher and keeps
/// its own argv after the program; a collector keeps the backend's priority.
#[test]
#[serial_test::serial(background_launcher)]
fn a_background_validation_runs_behind_the_priority_launcher() {
    use crate::core::background_priority::{self, HostOs, LauncherCheck, LauncherGuard};
    // The launcher state is process-wide: pin it, so the plan below is the one
    // the spawn uses whatever the host (un)checked.
    background_priority::set_enabled(true);
    let _launcher = LauncherGuard::set(LauncherCheck::Works);
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf()];
    let validated = validate(&spec("git", &["status"], root.path()), &roots).unwrap();
    let plan = background_priority::plan_here(Priority::Background);
    if crate::core::background_priority::enabled()
        && background_priority::launcher_for(HostOs::CURRENT, |path| Path::new(path).is_file())
            .is_some()
    {
        assert!(
            plan.launcher.is_some(),
            "an installed launcher is used: {plan:?}"
        );
    }

    let background = build_command(&validated, None, Priority::Background);
    let normal = build_command(&validated, None, Priority::Normal);
    let program = normal.as_std().get_program().to_os_string();
    assert_eq!(normal.as_std().get_args().count(), 1, "normal: argv only");

    let (expected_program, mut expected_args) = plan.argv(&program);
    expected_args.push("status".into());
    assert_eq!(background.as_std().get_program(), expected_program);
    let args: Vec<_> = background
        .as_std()
        .get_args()
        .map(|arg| arg.to_os_string())
        .collect();
    assert_eq!(args, expected_args);
}

/// A real red run, captured from cargo-nextest 0.9.148 (status lines carry
/// the `( 1/11)` counter): the six failed tests are named once each, with the
/// first panic and the `Summary` line.
#[test]
fn a_captured_nextest_run_names_its_six_failed_tests_and_the_first_panic() {
    let stderr = include_str!("fixtures/nextest_0_9_148_red_run.txt");
    let extracted = nextest_summary_of(stderr, QuickExecStatus::Failed);
    let summary = &extracted.summary;
    assert_eq!(
        extracted.failed_tests,
        vec![
            "kronn-l2 db::orchestration::tests::a_stopped_worker_reason_is_an_undelivered_worker_reason",
            "kronn-l3 api::discussions::messaging::tests::the_room_stop_interrupts_the_working_execution_of_that_room",
            "kronn-l3 api::discussions::runtime::chain_render_tests::a_cancelled_task_worker_dispatch_interrupts_its_execution_when_its_stream_ends",
            "kronn-l3 api::discussions::messaging::tests::stopping_a_task_worker_dispatch_interrupts_its_execution_in_the_same_request",
            "kronn::it api_tests::stop_route_parks_the_task_worker_execution",
            "kronn uptests::api__orchestration__tests::tests::a_restart_keeps_a_stopped_worker_parked",
        ],
        "{summary}"
    );
    for expected in [
        "assertion failed: is_undelivered_worker_reason(\"worker_stopped_without_delivery\")",
        "Summary [   1.155s] 11 tests run: 5 passed, 6 failed, 15711 skipped",
    ] {
        assert!(summary.contains(expected), "{expected} missing: {summary}");
    }
    for noise in ["running 1 test", "test result:", "PASS ["] {
        assert!(!summary.contains(noise), "{noise} is noise: {summary}");
    }
    assert!(summary.len() <= SUMMARY_MAX_BYTES);
}
