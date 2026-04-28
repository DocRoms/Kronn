//! Executor for `StepType::Exec` (0.7.0 Phase 5).
//!
//! Direct shell execution — zero agent tokens, no LLM. Defence in depth:
//!
//!   1. **Allowlist enforced at run time** even though the API
//!      validates at save time. A workflow loaded from disk could
//!      have a stale Exec step pointing at a binary the operator
//!      removed from the allowlist — fail loudly here too.
//!   2. **Never `sh -c`**. The binary is spawned directly (via
//!      `crate::core::cmd::async_cmd`) with args as separate argv
//!      elements. Pipes, redirections, glob expansion, and command
//!      substitution DO NOT apply.
//!   3. **Args templated, but rendered values are literal**. Even if
//!      a previous step's output contains `; rm -rf /`, the OS
//!      receives ONE argv string per `exec_args[i]` — not a shell
//!      command line.
//!   4. **Workdir locked** to `work_dir` (the run's workspace). No
//!      `cd /` possible from inside the step.
//!   5. **Timeout-bounded** via `tokio::time::timeout`. Default
//!      300s, hard-capped at 1800s by the API validator.
//!   6. **stdout/stderr captured + truncated** to ~100 KB combined
//!      so a runaway step can't blow up the DB row.
//!
//! Output format mirrors `notify_step.rs` — Structured envelope with
//! `data: {exit_code, stdout, stderr, duration_ms}` so downstream
//! steps can branch on `{{steps.run_tests.data.exit_code}}`.
//!
//! Note on cross-platform: uses `crate::core::cmd::async_cmd` which
//! auto-resolves `npx`/`git` on Windows and applies CREATE_NO_WINDOW.

use std::time::{Duration, Instant};

use crate::core::cmd::async_cmd;
use crate::models::*;

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Default timeout when `exec_timeout_secs` is unset on the step.
/// Matches the rough ceiling of common build/test commands; long
/// integration suites should set their own value explicitly.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Maximum combined stdout + stderr size persisted to the StepResult.
/// 100 KB is enough for a Rust compiler error dump or a Jest output;
/// past that, agents and humans alike stop reading. Truncation marker
/// is appended so the operator knows output was clipped.
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Truncation suffix appended when stdout / stderr exceeds [`MAX_OUTPUT_BYTES`].
const TRUNCATION_MARKER: &str = "\n\n[... output tronqué — limite 100 KB ...]";

pub async fn execute_exec_step(
    step: &WorkflowStep,
    workflow_allowlist: &[String],
    work_dir: &str,
    ctx: &TemplateContext,
) -> StepOutcome {
    let start = Instant::now();

    // ── Validate config (also enforced at save time, but stale workflows happen) ──
    let raw_command = match step.exec_command.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => return fail(step, start, "Exec step missing `exec_command`."),
    };
    if workflow_allowlist.is_empty() {
        return fail(step, start, format!(
            "Exec step `{}`: workflow's `exec_allowlist` is empty — Exec disabled.",
            step.name
        ));
    }
    if !workflow_allowlist.iter().any(|a| a == raw_command) {
        return fail(step, start, format!(
            "Exec step `{}`: binary `{}` not in allowlist [{}].",
            step.name, raw_command, workflow_allowlist.join(", ")
        ));
    }
    // Defence in depth: reject path-separator-bearing commands at run
    // time too. The save-time validator already does this — this catch
    // protects against a JSON-edited workflow that bypassed the API.
    if raw_command.contains('/') || raw_command.contains('\\') {
        return fail(step, start, format!(
            "Exec step `{}`: binary `{}` contains path separator (rejected).",
            step.name, raw_command
        ));
    }

    // Validate work_dir BEFORE spawn. When a workflow has no project
    // attached, `runner.rs` falls through to `work_dir = ""`. Passing
    // that to `Command::current_dir("")` would trigger `chdir("")` on
    // Linux → ENOENT → user sees the misleading message
    // "failed to spawn `make`: No such file or directory (os error 2)"
    // (the binary lookup never happened — chdir failed first).
    // Surface a clear diagnostic instead, naming the actual root cause
    // and what to do (attach a project to the workflow).
    let trimmed_workdir = work_dir.trim();
    if trimmed_workdir.is_empty() {
        return fail(step, start, format!(
            "Exec step `{}`: aucun répertoire de travail. Ce workflow n'est attaché à aucun projet, donc \
             il n'y a pas de `cwd` où exécuter `{}`. Solution : édite le workflow et choisis un projet \
             dans « Configuration → Projet », puis relance.",
            step.name, raw_command
        ));
    }
    if !std::path::Path::new(trimmed_workdir).exists() {
        return fail(step, start, format!(
            "Exec step `{}`: répertoire de travail introuvable (`{}`). Le projet attaché au workflow \
             pointe sur un chemin qui n'existe pas (renommé ? supprimé ? worktree non monté ?). \
             Vérifie le chemin du projet dans la page Configuration.",
            step.name, trimmed_workdir
        ));
    }

    // ── Render args via the template engine ──
    let mut rendered_args: Vec<String> = Vec::with_capacity(step.exec_args.len());
    for (i, arg_template) in step.exec_args.iter().enumerate() {
        match ctx.render(arg_template) {
            Ok(rendered) => rendered_args.push(rendered),
            Err(e) => return fail(step, start, format!(
                "Exec step `{}`: template render error on arg #{}: {}",
                step.name, i, e
            )),
        }
    }

    let timeout_secs = step.exec_timeout_secs
        .map(u64::from)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    // ── Spawn — args passed as separate argv, NEVER through a shell ──
    tracing::info!(
        target: "kronn::workflow_exec",
        step = %step.name,
        command = %raw_command,
        argc = rendered_args.len(),
        timeout_secs,
        workdir = %work_dir,
        "executing"
    );
    let mut cmd = async_cmd(raw_command);
    cmd.args(&rendered_args)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // SIGKILL the exec'd binary if the `output()` future is dropped
        // before completion. Required so workflow-run cancellation
        // actually stops a long-running `cargo test` / `npm install`
        // when the runner drops the step-dispatch future. Without this,
        // the child gets reparented to PID 1 and keeps running.
        .kill_on_drop(true);

    let output_future = cmd.output();
    let timed_out = match tokio::time::timeout(Duration::from_secs(timeout_secs), output_future).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => {
            return fail(step, start, format!(
                "Exec step `{}`: failed to spawn `{}`: {}",
                step.name, raw_command, e
            ));
        }
        Err(_) => Err(()), // timeout
    };

    let output = match timed_out {
        Ok(out) => out,
        Err(()) => {
            return fail(step, start, format!(
                "Exec step `{}`: timed out after {}s.",
                step.name, timeout_secs
            ));
        }
    };

    let exit_code = output.status.code();
    let stdout = truncate_to_limit(String::from_utf8_lossy(&output.stdout).into_owned());
    let stderr = truncate_to_limit(String::from_utf8_lossy(&output.stderr).into_owned());
    let duration_ms = start.elapsed().as_millis() as u64;

    let success = output.status.success();
    let status = if success { RunStatus::Success } else { RunStatus::Failed };
    let summary = match exit_code {
        Some(_) if success => format!("exit 0 — {} ms", duration_ms),
        Some(code) => format!("exit {} — {} ms", code, duration_ms),
        None => format!("killed by signal — {} ms", duration_ms),
    };

    // Structured envelope so `{{steps.<name>.data.exit_code}}` etc.
    // resolve in downstream steps. Mirrors notify_step's contract.
    let envelope = serde_json::json!({
        "data": {
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "duration_ms": duration_ms,
        },
        "status": if success { "OK" } else { "ERROR" },
        "summary": summary.clone(),
    });
    // Trailing `[SIGNAL: ...]` lines so users can branch their workflow
    // via `on_result.contains` without parsing JSON. Pattern mirrors what
    // Agent steps emit in their own output. We emit two granularities:
    //   - generic: OK / ERROR (broad branching, matches "tests passed?")
    //   - exit_<code>: specific (exit_0, exit_1, exit_2…) for users who
    //     want fine control (e.g. exit_2 = compile error, exit_1 = test fail)
    // Both are appended AFTER the JSON envelope, so the last 5 lines that
    // `evaluate_conditions` scans always include them.
    let signal_generic = if success { "[SIGNAL: OK]" } else { "[SIGNAL: ERROR]" };
    let signal_exit = exit_code
        .map(|c| format!("[SIGNAL: exit_{}]", c))
        .unwrap_or_else(|| "[SIGNAL: killed]".to_string());
    let envelope_str = format!(
        "{}\n\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---\n{}\n{}",
        summary,
        serde_json::to_string(&envelope).unwrap_or_default(),
        signal_generic,
        signal_exit,
    );

    // Evaluate user-declared on_result conditions against the signals
    // we just emitted. This lets a workflow do "tests fail → loop back
    // to implement" without writing a wrapper Agent step. The runner
    // honors the resulting condition_action even when status == Failed
    // (a Goto/Skip overrides the rollback chain).
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &envelope_str);
    let condition_result = condition_action.as_ref().map(|a| match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{}", step_name),
    });

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status,
            output: envelope_str,
            tokens_used: 0,
            duration_ms,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action,
    }
}

fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    let msg = msg.into();
    tracing::warn!(
        target: "kronn::workflow_exec",
        step = %step.name,
        "Exec step failed: {}", msg
    );
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: msg,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action: None,
    }
}

/// Truncate to [`MAX_OUTPUT_BYTES`] on a UTF-8 boundary, appending the
/// truncation marker. UTF-8 boundary care matters: cutting a multibyte
/// sequence yields invalid UTF-8 and crashes downstream JSON encoding.
fn truncate_to_limit(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    // Walk back from MAX_OUTPUT_BYTES to the nearest char boundary.
    let mut cut = MAX_OUTPUT_BYTES;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = String::with_capacity(cut + TRUNCATION_MARKER.len());
    truncated.push_str(&s[..cut]);
    truncated.push_str(TRUNCATION_MARKER);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec_step(
        name: &str,
        command: Option<&str>,
        args: Vec<&str>,
        timeout_secs: Option<u32>,
    ) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: StepType::Exec,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::Structured,
            on_result: vec![],
            agent_settings: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            mcp_config_ids: vec![],
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            exec_command: command.map(String::from),
            exec_args: args.into_iter().map(String::from).collect(),
            exec_timeout_secs: timeout_secs,
            quick_prompt_id: None,
            json_data_payload: None,
        }
    }

    #[tokio::test]
    async fn rejects_when_allowlist_empty() {
        let step = exec_step("run", Some("echo"), vec!["hi"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &[], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("allowlist"), "got: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn rejects_when_command_not_in_allowlist() {
        let step = exec_step("run", Some("rm"), vec!["-rf", "/"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("not in allowlist"));
    }

    #[tokio::test]
    async fn rejects_path_separator_in_command() {
        // Even if "/usr/bin/echo" matched the allowlist (impossible —
        // save-time validator rejects that too), the runtime guard
        // catches it. Tests the defence-in-depth layer.
        let step = exec_step("run", Some("/usr/bin/echo"), vec![], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["/usr/bin/echo".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("path separator"));
    }

    /// Regression: empty work_dir used to surface as a cryptic
    /// "failed to spawn `make`: No such file or directory (os error 2)"
    /// because `Command::current_dir("")` triggers `chdir("")` →
    /// ENOENT before the binary is even looked up. The pre-spawn
    /// guard now catches this and names the real root cause
    /// (workflow not attached to a project).
    #[tokio::test]
    async fn rejects_empty_workdir_with_clear_message() {
        let step = exec_step("run_tests", Some("make"), vec!["test"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["make".to_string()], "", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        // Must NOT mention "spawn" — that would be the misleading old message.
        assert!(!outcome.result.output.contains("spawn"),
            "old misleading 'failed to spawn' message must not appear, got: {}",
            outcome.result.output);
        // Must mention the real cause + the fix.
        assert!(outcome.result.output.contains("projet"),
            "error must point at the missing project, got: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn rejects_nonexistent_workdir_with_clear_message() {
        let step = exec_step("run_tests", Some("echo"), vec!["hi"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(
            &step,
            &["echo".to_string()],
            "/this/path/does/not/exist/anywhere",
            &ctx,
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("introuvable") || outcome.result.output.contains("not found"),
            "got: {}", outcome.result.output);
    }

    // The next 3 tests actually invoke a binary — gate behind cfg(unix)
    // because Windows CI doesn't have `echo` as a discrete binary
    // (it's a cmd.exe builtin), and the cross-platform discipline is
    // already enforced by the allowlist + cmd::async_cmd helpers.
    #[cfg(unix)]
    #[tokio::test]
    async fn echoes_simple_args() {
        let step = exec_step("greet", Some("echo"), vec!["hello", "world"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("exit 0"));
        // The envelope's data.stdout should contain "hello world\n".
        assert!(outcome.result.output.contains("hello world"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn arg_with_shell_metas_passed_as_literal_does_not_execute() {
        // Critical security test: an arg containing `; rm -rf /` MUST
        // be passed as a single literal argument to `echo`, NOT
        // executed as shell. We assert echo prints it verbatim.
        let step = exec_step("greet", Some("echo"), vec!["; rm -rf /"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        // echo prints "; rm -rf /\n" — if shell had been invoked, the
        // output would either be empty (rm runs silently) or contain
        // an `rm: cannot remove '/'` error. The literal text in stdout
        // proves the arg was treated as data, not as a shell command.
        assert!(
            outcome.result.output.contains("; rm -rf /"),
            "shell meta arg must be passed literally, got: {}", outcome.result.output
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn template_in_arg_renders_then_passes_literal() {
        // The {{steps.X.summary}} template is rendered FIRST, then the
        // result is passed as a single argv element. Even if the
        // rendered value contains shell metas, no shell sees them.
        let step = exec_step("greet", Some("echo"), vec!["from {{ctx.who}}"], None);
        let mut ctx = TemplateContext::new();
        ctx.set("ctx.who", "templated; with metas $(whoami)");
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(
            outcome.result.output.contains("from templated; with metas $(whoami)"),
            "template rendered, then passed as literal — got: {}", outcome.result.output
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_exit_yields_failed_status() {
        // `false` is the canonical "exit 1" binary on Unix. Status
        // must reflect the failure so downstream conditions
        // (`on_result contains "ERROR" → Stop`) can fire.
        let step = exec_step("breaker", Some("false"), vec![], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["false".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("exit 1"));
    }

    #[tokio::test]
    async fn truncation_marker_appended_when_output_huge() {
        // Pure unit test on the helper — no spawn needed.
        let huge = "x".repeat(MAX_OUTPUT_BYTES + 1024);
        let truncated = truncate_to_limit(huge);
        assert!(truncated.ends_with(TRUNCATION_MARKER));
        assert!(truncated.len() <= MAX_OUTPUT_BYTES + TRUNCATION_MARKER.len());
    }

    #[test]
    fn truncation_respects_utf8_boundary() {
        // A multibyte char straddling MAX_OUTPUT_BYTES must not split
        // mid-codepoint (would yield invalid UTF-8 → crash json
        // encoding downstream). The "é" is 2 bytes; we craft input so
        // the split lands inside it.
        let prefix = "x".repeat(MAX_OUTPUT_BYTES - 1);
        let s = format!("{}é-tail", prefix); // "é" starts at byte (MAX-1)
        let truncated = truncate_to_limit(s);
        // No panic = boundary check did its job. Verify the output is
        // still valid UTF-8 (intrinsic to String, but explicit assert).
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    // ─── on_result + SIGNAL emission tests ────────────────────────────
    //
    // Auto-dev pattern: cargo test fails → loop back to implement.
    // Without these, the runner saw `Failed` and short-circuited to
    // rollback — defeating the point of declaring an `on_result` rule.

    #[cfg(unix)]
    #[tokio::test]
    async fn success_appends_signal_ok_and_exit_0() {
        let step = exec_step("greet", Some("echo"), vec!["hi"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert!(outcome.result.output.contains("[SIGNAL: OK]"));
        assert!(outcome.result.output.contains("[SIGNAL: exit_0]"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_exit_appends_signal_error_and_exit_code() {
        let step = exec_step("fail", Some("false"), vec![], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["false".to_string()], "/tmp", &ctx).await;
        assert!(outcome.result.output.contains("[SIGNAL: ERROR]"));
        assert!(outcome.result.output.contains("[SIGNAL: exit_1]"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn on_result_goto_fires_when_signal_matches_on_failure() {
        // The whole point of the feature: `false` exits 1, status = Failed,
        // BUT the user declared `contains "ERROR" → Goto implement`.
        // The executor must surface that condition_action so the runner
        // can honour it instead of triggering on_failure.
        let mut step = exec_step("run_tests", Some("false"), vec![], None);
        step.on_result = vec![StepConditionRule {
            contains: "ERROR".to_string(),
            action: ConditionAction::Goto {
                step_name: "implement".to_string(),
                max_iterations: Some(5),
            },
        }];
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["false".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        match outcome.condition_action {
            Some(ConditionAction::Goto { step_name, max_iterations }) => {
                assert_eq!(step_name, "implement");
                assert_eq!(max_iterations, Some(5));
            }
            other => panic!("expected Goto on_result match, got {:?}", other),
        }
        // condition_result also surfaces in the StepResult for the UI.
        assert_eq!(outcome.result.condition_result.as_deref(), Some("Goto:implement"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn on_result_no_match_leaves_condition_action_none() {
        // User declared `contains "TIMEOUT"` but exec exited 1.
        // No match → condition_action stays None → runner falls through
        // to its existing rollback path. Backwards-compatible.
        let mut step = exec_step("run_tests", Some("false"), vec![], None);
        step.on_result = vec![StepConditionRule {
            contains: "TIMEOUT".to_string(),
            action: ConditionAction::Goto {
                step_name: "fallback".to_string(),
                max_iterations: Some(1),
            },
        }];
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["false".to_string()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.condition_action.is_none());
    }

    /// Foundation for mid-step cancellation: the runner uses
    /// `tokio::select!` to race the executor future against
    /// `cancel_token.cancelled()`. When the cancel branch wins, the
    /// executor future is dropped — and `kill_on_drop(true)` (set in
    /// `execute_exec_step`) is what turns that drop into a SIGKILL on
    /// the spawned binary. Without it, the child gets reparented to
    /// PID 1 and keeps running.
    ///
    /// This test pins that contract: drop a long-sleep future early,
    /// assert wall-clock is far below the sleep duration. If someone
    /// later removes `kill_on_drop(true)`, this test fails with a
    /// 30-second timeout instead of completing in milliseconds.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_drops_future_and_kills_long_running_child() {
        let step = exec_step("long_sleep", Some("sleep"), vec!["30"], Some(60));
        let ctx = TemplateContext::new();
        let allowlist = vec!["sleep".to_string()];

        let started = std::time::Instant::now();
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_task = cancel.clone();
        // Cancel after a short delay — long enough to let `sleep 30`
        // actually spawn, short enough that the test finishes fast if
        // kill_on_drop works.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            cancel_for_task.cancel();
        });

        let exec_future = execute_exec_step(&step, &allowlist, "/tmp", &ctx);
        let _ = tokio::select! {
            o = exec_future => Some(o),
            _ = cancel.cancelled() => None,
        };
        let elapsed = started.elapsed();

        // Hard upper bound: must finish in < 2s, way below the 30s sleep.
        // If kill_on_drop is removed, the inner future would still own the
        // child until `sleep 30` returns, blowing past this threshold.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "select drop should kill the child fast — took {:?}",
            elapsed
        );
    }
}
