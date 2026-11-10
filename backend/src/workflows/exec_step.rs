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
//!      command line. The exception is an allowlisted interpreter's
//!      inline code (`bash -c`, `python3 -c…`, `node --eval=…`): the
//!      save-time validator refuses any placeholder there but
//!      `{{run.id}}` / `{{time.now…}}`; values go in later argv entries.
//!   4. **Workdir locked** to `work_dir` (the run's workspace). No
//!      `cd /` possible from inside the step.
//!   5. **Timeout-bounded** via `tokio::time::timeout`. Default
//!      300s, hard-capped at 1800s by the API validator.
//!   6. **stdout/stderr streamed and bounded** to ~100 KB each (2 MiB when
//!      a `PublishPageData` step reads the step) so a runaway step can't blow
//!      up the DB row; a truncated stream is flagged in the envelope.
//!
//! Output format mirrors `notify_step.rs` — Structured envelope with
//! `data: {exit_code, stdout, stderr, duration_ms}` so downstream
//! steps can branch on `{{steps.run_tests.data.exit_code}}`.
//!
//! Note on cross-platform: uses `crate::core::cmd::async_cmd` which
//! auto-resolves `npx`/`git` on Windows and applies CREATE_NO_WINDOW.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::core::background_priority::{self, Priority};
use crate::core::cmd::async_cmd_planned;
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
/// Collectors may legitimately return a larger typed document than a test log.
/// Still bounded so one CLI cannot inflate a workflow run without limit.
pub const MAX_COLLECT_OUTPUT_BYTES: usize = 1024 * 1024;
/// Limit for a step a `PublishPageData` reads: a page needs the whole
/// document, and 2 MiB is also what `POST /api/pages/{id}/publish` accepts.
pub const MAX_PUBLISH_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
/// Start of every truncation marker, whatever the limit.
pub const TRUNCATION_MARKER_PREFIX: &str = "[TRUNCATED: kept ";

/// The marker written before KT-1570; run records from then still carry it.
pub const LEGACY_TRUNCATION_MARKER_PREFIX: &str = "[... output tronqué — limite ";

/// Added to a failed `gh` step whose project gave it no GitHub token.
pub const GH_NOT_CONNECTED_HINT: &str = " — this project is not connected to GitHub in Kronn, so the step received no GitHub token (Projects → Overview → GitHub)";

/// KT-1633 — an Exec step (an autoCode build, `cargo test`, `npm install`) runs
/// in the background: it yields to the owner's interactive applications.
const EXEC_STEP_PRIORITY: Priority = Priority::Background;

/// The command of an Exec step child at [`EXEC_STEP_PRIORITY`], behind the
/// host's priority launcher unless the owner turned that off.
fn step_command(program: &str) -> tokio::process::Command {
    step_command_planned(program, background_priority::plan_here(EXEC_STEP_PRIORITY))
}

fn step_command_planned(program: &str, plan: background_priority::Plan) -> tokio::process::Command {
    async_cmd_planned(
        program.as_ref(),
        crate::core::child_env::ChildRoute::WorkflowExec,
        plan,
    )
}

/// An Exec step child (setup or main command): the program with its built
/// environment (KT-1006) and the project's GitHub variables only when the
/// project is connected (D2); callers only add argv, cwd and stdio.
/// `refs` are the workflow's `KRONN_REF_<NAME>` ids (KT-1448).
fn exec_child(
    program: &str,
    github_env: &[(String, String)],
    refs: &[(String, String)],
) -> tokio::process::Command {
    isolated_exec_child(step_command(program), github_env, refs)
}

fn isolated_exec_child(
    mut command: tokio::process::Command,
    github_env: &[(String, String)],
    refs: &[(String, String)],
) -> tokio::process::Command {
    crate::core::child_env::isolate_with_github_values_and_refs(
        command.as_std_mut(),
        crate::core::child_env::ChildRoute::WorkflowExec,
        github_env,
        &[],
        refs,
    );
    command
}

/// KT-1492 — the program and arguments a step starts. A line a trust level
/// approved (not a human) runs under that level's OS sandbox, which closes
/// Kronn's data directory to it (the network and the API port stay open). When
/// the host has no sandbox, or the owner turned "Protect the Kronn vault from
/// agents" off, it runs as it would for a human: a level refuses nothing for
/// that. Any other step is unchanged.
pub(crate) fn confined_for_level(
    step: &WorkflowStep,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    if step.exec_approved_by_trust == Some(true) {
        if let Some(confinement) = crate::core::trust_sandbox::confinement() {
            return confinement.wrap(program, args);
        }
    }
    (program.to_string(), args.to_vec())
}

/// The main command of a step running approved repository scripts (KT-918):
/// it is told where the worktree is, since its cwd is the approved copy.
fn approved_script_child(
    program: &str,
    github_env: &[(String, String)],
    refs: &[(String, String)],
    work_dir: &str,
    copy_dir: &Path,
) -> tokio::process::Command {
    use crate::core::approved_scripts::{SCRIPTS_DIR_ENV, WORKTREE_ENV};
    let mut command = step_command(program);
    crate::core::child_env::isolate_with_github_values_and_refs(
        command.as_std_mut(),
        crate::core::child_env::ChildRoute::WorkflowExec,
        github_env,
        &[
            (WORKTREE_ENV, std::ffi::OsStr::new(work_dir)),
            (SCRIPTS_DIR_ENV, copy_dir.as_os_str()),
        ],
        refs,
    );
    command
}

/// Verifies the step's declared files against their approved hashes and
/// copies them into the run's artifacts directory; the copy is the cwd.
fn prepare_approved_scripts(
    step: &WorkflowStep,
    carrying_repository: Option<&str>,
    ctx: &TemplateContext,
) -> Result<PathBuf, String> {
    let root = carrying_repository
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .ok_or_else(|| {
            "declares repository scripts but its workflow has no home project to read them from"
                .to_string()
        })?;
    let artifacts = ctx
        .get("run.artifacts_dir")
        .filter(|dir| !dir.trim().is_empty())
        .ok_or_else(|| {
            "the run has no artifacts directory to hold the approved copy of its scripts"
                .to_string()
        })?;
    let dest = crate::core::approved_scripts::copy_dir(Path::new(artifacts), &step.name);
    crate::core::approved_scripts::prepare_copy(Path::new(root), &step.exec_script_files, &dest)?;
    Ok(dest)
}

pub async fn execute_exec_step(
    step: &WorkflowStep,
    workflow_allowlist: &[String],
    work_dir: &str,
    ctx: &TemplateContext,
) -> StepOutcome {
    execute_exec_step_with_output_limit(
        step,
        workflow_allowlist,
        work_dir,
        ctx,
        MAX_OUTPUT_BYTES,
        None,
    )
    .await
}

pub async fn execute_exec_step_with_output_limit(
    step: &WorkflowStep,
    workflow_allowlist: &[String],
    work_dir: &str,
    ctx: &TemplateContext,
    output_limit_bytes: usize,
    project_id: Option<&str>,
) -> StepOutcome {
    let github_env = crate::core::github_connection::env_for_launch(project_id).await;
    execute_exec_step_inner(
        step,
        workflow_allowlist,
        work_dir,
        ctx,
        OutputPolicy::plain(output_limit_bytes),
        &github_env,
        None,
    )
    .await
}

/// The steps around a publish, read before the run: `sequence` is the list
/// the publish runs in, `other` the workflow's other list.
#[derive(Clone, Copy)]
struct StepLineage<'a> {
    sequence: &'a [WorkflowStep],
    other: &'a [WorkflowStep],
}

/// The step a typed source most likely reads: `steps.<name>.…` by name,
/// `previous_step.…` as the step listed just before `consumer`.
fn source_producer<'a>(
    key: &str,
    consumer: &str,
    lineage: StepLineage<'a>,
) -> Option<&'a WorkflowStep> {
    let path = key.split_once("??").map_or(key, |(path, _)| path).trim();
    if path.starts_with("previous_step.") {
        let index = lineage
            .sequence
            .iter()
            .position(|step| step.name == consumer)?;
        return index.checked_sub(1).map(|before| &lineage.sequence[before]);
    }
    let rest = path.strip_prefix("steps.")?;
    lineage
        .sequence
        .iter()
        .chain(lineage.other)
        .filter(|step| {
            rest.strip_prefix(step.name.as_str())
                .is_some_and(|tail| tail.starts_with('.'))
        })
        .max_by_key(|step| step.name.len())
}

/// Output limit for `step` in a workflow made of `steps` and `on_failure`:
/// raised to [`MAX_PUBLISH_OUTPUT_BYTES`] when a `PublishPageData` write reads
/// it, by name or as `previous_step`. Best effort, from list order: a jump can
/// change what `previous_step` holds, and the publish then refuses a cut
/// output from what the run recorded, never publishes it.
pub fn output_limit_for(
    step: &WorkflowStep,
    steps: &[WorkflowStep],
    on_failure: &[WorkflowStep],
) -> usize {
    use super::publish_page_step::typed_source_key;
    let read_in = |sequence: &[WorkflowStep], other: &[WorkflowStep]| {
        let lineage = StepLineage { sequence, other };
        sequence
            .iter()
            .filter(|publish| publish.step_type == StepType::PublishPageData)
            .any(|publish| {
                publish
                    .page_publish
                    .iter()
                    .flat_map(|config| &config.writes)
                    .any(|write| {
                        typed_source_key(&write.value_from)
                            .ok()
                            .and_then(|key| source_producer(key, &publish.name, lineage))
                            .is_some_and(|producer| producer.name == step.name)
                    })
            })
    };
    if read_in(steps, on_failure) || read_in(on_failure, steps) {
        MAX_PUBLISH_OUTPUT_BYTES
    } else {
        MAX_OUTPUT_BYTES
    }
}

/// A workflow Exec step: the workflow author chose the command, so it gets
/// the project's GitHub token when the project is connected, and none
/// otherwise (D2). Other callers keep the inherited environment.
/// `carrying_repository` is the home project's checkout, where declared
/// repository scripts are read from (KT-918).
pub async fn execute_exec_step_for_project(
    step: &WorkflowStep,
    workflow_allowlist: &[String],
    work_dir: &str,
    ctx: &TemplateContext,
    project_id: Option<&str>,
    carrying_repository: Option<&str>,
    output_limit_bytes: usize,
) -> StepOutcome {
    let github_env = crate::core::github_connection::env_for_launch(project_id).await;
    execute_exec_step_inner(
        step,
        workflow_allowlist,
        work_dir,
        ctx,
        OutputPolicy::workflow_step(output_limit_bytes),
        &github_env,
        carrying_repository,
    )
    .await
}

/// The refusals of an Exec step's saved configuration, checked before any
/// value is rendered; the readiness diagnostic reads the same rules (KT-1138).
pub fn exec_config_refusal(step: &WorkflowStep, workflow_allowlist: &[String]) -> Option<String> {
    let raw_command = match step.exec_command.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => return Some("Exec step missing `exec_command`.".to_string()),
    };
    if workflow_allowlist.is_empty() {
        return Some(format!(
            "Exec step `{}`: workflow's `exec_allowlist` is empty — Exec disabled.",
            step.name
        ));
    }
    if !workflow_allowlist.iter().any(|a| a == raw_command) {
        return Some(format!(
            "Exec step `{}`: binary `{}` not in allowlist [{}].",
            step.name,
            raw_command,
            workflow_allowlist.join(", ")
        ));
    }
    // Defence in depth: a JSON-edited workflow may have bypassed the API.
    if raw_command.contains('/') || raw_command.contains('\\') {
        return Some(format!(
            "Exec step `{}`: binary `{}` contains path separator (rejected).",
            step.name, raw_command
        ));
    }
    None
}

/// The setup line's allowlist and path rules, the same as the main command's.
pub fn setup_config_refusal(step: &str, setup_cmd: &str, allowlist: &[String]) -> Option<String> {
    if !allowlist.iter().any(|a| a == setup_cmd) {
        return Some(format!(
            "Exec step `{step}`: setup binary `{setup_cmd}` not in allowlist [{}].",
            allowlist.join(", ")
        ));
    }
    if setup_cmd.contains('/') || setup_cmd.contains('\\') {
        return Some(format!(
            "Exec step `{step}`: setup binary `{setup_cmd}` contains path separator (rejected)."
        ));
    }
    None
}

/// The refusal of an irreversible invocation, on the rendered arguments.
pub fn destructive_refusal(step: &str, cmd: &str, args: &[String], setup: bool) -> Option<String> {
    let reason = destructive_reason(cmd, args)?;
    Some(if setup {
        format!(
            "Exec step `{step}`: refused setup `{cmd} {}` — {reason}.",
            args.join(" ")
        )
    } else {
        format!(
            "Exec step `{step}`: refused `{cmd} {}` — {reason}. Reformule l'étape sans cette opération destructive.",
            args.join(" ")
        )
    })
}

/// Every refusal the executor would give before spawning that does not depend
/// on a run value, as (phase, message). A line with a template is left to the
/// run, which checks it once rendered (KT-1138).
pub fn static_refusals(step: &WorkflowStep, allowlist: &[String]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    if let Some(refusal) = exec_config_refusal(step, allowlist) {
        found.push(("main".to_string(), refusal));
    }
    let literal = |args: &[String]| !args.iter().any(|a| a.contains("{{"));
    if let Some(cmd) = step
        .exec_command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        if literal(&step.exec_args) {
            if let Some(refusal) = destructive_refusal(&step.name, cmd, &step.exec_args, false) {
                found.push(("main".to_string(), refusal));
            }
        }
    }
    if let Some(setup) = step
        .exec_setup_command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        if let Some(refusal) = setup_config_refusal(&step.name, setup, allowlist) {
            found.push(("setup".to_string(), refusal));
        } else if literal(&step.exec_setup_args) {
            if let Some(refusal) =
                destructive_refusal(&step.name, setup, &step.exec_setup_args, true)
            {
                found.push(("setup".to_string(), refusal));
            }
        }
    }
    found
}

/// What a step that could not start says. A missing `python3` (the shipped
/// autoCode Issue steps are `python3` scripts, KT-1643) gets the prerequisite
/// message that names what to install, not a raw OS error; any other failure
/// keeps the raw spawn error.
pub(crate) fn spawn_failure(step_name: &str, command: &str, error: &std::io::Error) -> String {
    if command == crate::core::contribution::PYTHON_PROGRAM
        && error.kind() == std::io::ErrorKind::NotFound
    {
        return format!(
            "Exec step `{step_name}`: {}",
            crate::core::contribution::python_missing_message()
        );
    }
    format!("Exec step `{step_name}`: failed to spawn `{command}`: {error}")
}

fn needs_gh_hint(raw_command: &str, success: bool, github_env: &[(String, String)]) -> bool {
    !success && raw_command == "gh" && github_env.is_empty()
}

async fn execute_exec_step_inner(
    step: &WorkflowStep,
    workflow_allowlist: &[String],
    work_dir: &str,
    ctx: &TemplateContext,
    output: OutputPolicy,
    github_env: &[(String, String)],
    carrying_repository: Option<&str>,
) -> StepOutcome {
    let start = Instant::now();
    let output_limit_bytes = output.limit.clamp(1, MAX_PUBLISH_OUTPUT_BYTES);
    // The step's only credential is the project's GitHub token: never let an
    // echo of it reach the run record or a published page.
    let mut secrets = crate::core::secret_scrub::SecretSet::new();
    for (_, value) in github_env {
        secrets.add(value);
    }

    // ── Validate config (also enforced at save time, but stale workflows happen) ──
    if let Some(refusal) = exec_config_refusal(step, workflow_allowlist) {
        return fail(step, start, refusal);
    }
    let raw_command = step
        .exec_command
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    // A saved step that interpolates a value into inline code never runs,
    // whatever triggered the run (KT-1017); the editor offers the rewrite.
    if let Some(refusal) = crate::core::inline_code::runtime_refusal(step) {
        return fail(step, start, refusal);
    }

    // Validate work_dir BEFORE spawn. When a workflow has no project
    // attached, `runner.rs` falls through to `work_dir = ""`. Passing
    // that to `Command::current_dir("")` would trigger `chdir("")` on
    // Linux → ENOENT → user sees the misleading message
    // "failed to spawn `make`: No such file or directory (os error 2)"
    // (the binary lookup never happened — chdir failed first).
    // Surface a clear diagnostic instead, naming the actual root cause
    // and what to do (attach a project to the workflow).
    // 0.8.2 — Translate the workspace path from container-view
    // (`/host-home/...`) to host-view (`${KRONN_HOST_HOME}/...`) when both
    // exist. Required for docker-in-docker workflows: when the Exec step
    // shells out to `docker compose run/exec`, the host docker daemon
    // resolves volume sources against its OWN filesystem, where only the
    // host path exists. Self-mount at the docker-compose.yml level makes
    // both paths point to the same bind, so either chdir works inside the
    // container — but only the host-view path makes downstream docker
    // calls resolve volumes correctly. Falls through to the original path
    // when the env var is unset (non-Docker dev setups) or the host path
    // isn't reachable (self-mount not configured).
    let host_workdir: String = match crate::core::child_env::var("KRONN_HOST_HOME") {
        Ok(host_home) if work_dir.starts_with("/host-home") && !host_home.is_empty() => {
            let candidate = work_dir.replacen("/host-home", host_home.trim_end_matches('/'), 1);
            if std::path::Path::new(&candidate).exists() {
                candidate
            } else {
                work_dir.to_string()
            }
        }
        _ => work_dir.to_string(),
    };
    let work_dir = host_workdir.as_str();
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

    // KT-918 — declared repository files run from a verified copy; a file
    // that changed since approval stops the step before anything runs.
    let approved_copy = if step.exec_script_files.is_empty() {
        None
    } else {
        match prepare_approved_scripts(step, carrying_repository, ctx) {
            Ok(dir) => Some(dir),
            Err(error) => {
                return fail(step, start, format!("Exec step `{}`: {error}", step.name));
            }
        }
    };

    // ── Render args via the template engine ──
    let mut rendered_args: Vec<String> = Vec::with_capacity(step.exec_args.len());
    for (i, arg_template) in step.exec_args.iter().enumerate() {
        match ctx.render_strict(arg_template) {
            Ok(rendered) => rendered_args.push(rendered),
            Err(e) => {
                return fail(
                    step,
                    start,
                    format!(
                        "Exec step `{}`: template render error on arg #{}: {}",
                        step.name, i, e
                    ),
                )
            }
        }
    }
    // The rendered argv is checked again with each field's provenance: an
    // outside value must not have become an option or code (KT-1017).
    if let Some(refusal) = crate::core::inline_code::rendered_refusal(
        &step.name,
        raw_command,
        &step.exec_args,
        &rendered_args,
        crate::core::inline_code::line_trust(step, "main"),
    ) {
        return fail(step, start, refusal);
    }
    // 2026-06-11 — defence-in-depth: the allowlist authorises a BINARY, but
    // `git`/`rm` carry irreversible foot-guns in their ARGS. Refuse the few
    // unambiguously-destructive invocations (force-push rewrites shared
    // history; rm -rf is unrecoverable) AFTER templating, so a `{{var}}` that
    // renders into `--force` can't sneak past. This is NOT a sandbox — just a
    // guardrail against the obvious irreversible ones.
    if let Some(refusal) = destructive_refusal(&step.name, raw_command, &rendered_args, false) {
        return fail(step, start, refusal);
    }

    let timeout_secs = step
        .exec_timeout_secs
        .map(u64::from)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    // ── 0.8.2 — Setup phase (worktree dep install) ──
    // When `exec_setup_command` is set, run it FIRST. Same allowlist +
    // path-validation rules. Same workdir. If it fails (non-zero exit
    // or timeout), the step fails immediately with a setup-scoped error
    // message so the operator can fix the install without thinking it's
    // the main command's fault. Output is prepended to the main command's
    // output for traceability.
    let mut setup_output_prefix = String::new();
    if let Some(setup_cmd) = step
        .exec_setup_command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        // Allowlist + path-separator check, same as the main command.
        if let Some(refusal) = setup_config_refusal(&step.name, setup_cmd, workflow_allowlist) {
            return fail(step, start, refusal);
        }
        // Render setup args.
        let mut setup_args: Vec<String> = Vec::with_capacity(step.exec_setup_args.len());
        for (i, arg_template) in step.exec_setup_args.iter().enumerate() {
            match ctx.render_strict(arg_template) {
                Ok(rendered) => setup_args.push(rendered),
                Err(e) => {
                    return fail(
                        step,
                        start,
                        format!(
                            "Exec step `{}`: setup template render error on arg #{}: {}",
                            step.name, i, e
                        ),
                    )
                }
            }
        }
        if let Some(refusal) = crate::core::inline_code::rendered_refusal(
            &step.name,
            setup_cmd,
            &step.exec_setup_args,
            &setup_args,
            crate::core::inline_code::line_trust(step, "setup"),
        ) {
            return fail(step, start, format!("{refusal} (setup)"));
        }
        // Same destructive-arg guard as the main command (2026-06-11).
        if let Some(refusal) = destructive_refusal(&step.name, setup_cmd, &setup_args, true) {
            return fail(step, start, refusal);
        }
        tracing::info!(
            target: "kronn::workflow_exec",
            step = %step.name,
            setup_command = %setup_cmd,
            setup_argc = setup_args.len(),
            "executing setup phase"
        );
        let (setup_program, setup_launch_args) = confined_for_level(step, setup_cmd, &setup_args);
        let mut sc = exec_child(&setup_program, github_env, &ctx.refs_env());
        sc.args(&setup_launch_args)
            .current_dir(work_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let setup_output = match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            run_bounded(&mut sc, None, MAX_OUTPUT_BYTES, MAX_OUTPUT_BYTES),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return fail(
                    step,
                    start,
                    format!(
                        "Exec step `{}`: failed to spawn setup `{}`: {}",
                        step.name, setup_cmd, e
                    ),
                )
            }
            Err(_) => {
                return fail(
                    step,
                    start,
                    format!(
                        "Exec step `{}`: setup `{}` timed out after {}s.",
                        step.name, setup_cmd, timeout_secs
                    ),
                )
            }
        };
        let setup_stdout = plain_text(setup_output.stdout, &secrets, MAX_OUTPUT_BYTES);
        let setup_stderr = plain_text(setup_output.stderr, &secrets, MAX_OUTPUT_BYTES);
        // Surface the setup head so the operator can scroll through it
        // in the run-detail panel — useful when install logs reveal a
        // missing tool or auth issue.
        setup_output_prefix = format!(
            "── Setup: `{} {}` ──\n{}{}\n",
            setup_cmd,
            setup_args.join(" "),
            setup_stdout,
            if setup_stderr.is_empty() {
                String::new()
            } else {
                format!("\n[stderr]\n{}", setup_stderr)
            },
        );
        if !setup_output.status.success() {
            let code = setup_output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "killed".into());
            return fail(step, start, format!(
                "Exec step `{}`: setup `{} {}` failed (exit {}). \
                 stderr (tail):\n{}\n\n— le main `{}` n'a PAS été exécuté. \
                 Vérifie que les dépendances peuvent s'installer dans le worktree (réseau, lockfile présent, scopes d'auth).",
                step.name,
                setup_cmd,
                setup_args.join(" "),
                code,
                setup_stderr,
                raw_command,
            ));
        }
    }

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
    // ── 0.8.8 — optional stdin payload (bypasses the argv ARG_MAX ceiling) ──
    // Rendered through the template engine like `exec_args`, but fed to the
    // MAIN command's stdin instead of the argument list — so a large reshaped
    // payload (e.g. an enriched Jira backlog, which blows the ~128 KB ARG_MAX
    // past ~16 tickets) can reach a `jq` / reshape script. `None` (or an
    // all-whitespace template) → stdin stays `/dev/null`, unchanged behaviour.
    let stdin_bytes: Option<Vec<u8>> = match step.exec_stdin.as_deref() {
        Some(t) if !t.trim().is_empty() => match ctx.render_strict(t) {
            Ok(rendered) => Some(rendered.into_bytes()),
            Err(e) => {
                return fail(
                    step,
                    start,
                    format!(
                        "Exec step `{}`: template render error on `exec_stdin`: {}",
                        step.name, e
                    ),
                )
            }
        },
        _ => None,
    };

    // A script shape is trusted for its script path: it must name a regular
    // file, never a device or a link to one (`tool.py -> /dev/stdin`).
    let stdin_script = step.exec_stdin.as_deref().and_then(|stdin| {
        crate::core::inline_code::stdin_fed_script(raw_command, &step.exec_args, stdin)
    });
    if let Some(script) =
        crate::core::inline_code::trusted_script(raw_command, &step.exec_args).or(stdin_script)
    {
        let base = approved_copy.as_deref().unwrap_or(Path::new(work_dir));
        let regular = std::fs::canonicalize(base.join(&script)).is_ok_and(|real| {
            real.is_file() && !real.starts_with("/dev") && !real.starts_with("/proc")
        });
        if !regular {
            return fail(
                step,
                start,
                format!(
                    "Exec step `{}`: le script `{script}` n'est pas un fichier ordinaire \
                     (introuvable, périphérique ou lien vers un périphérique) ; refusé avant \
                     exécution.",
                    step.name
                ),
            );
        }
    }
    let (program, launch_args) = confined_for_level(step, raw_command, &rendered_args);
    let mut cmd = match approved_copy.as_deref() {
        Some(copy) => approved_script_child(&program, github_env, &ctx.refs_env(), work_dir, copy),
        None => exec_child(&program, github_env, &ctx.refs_env()),
    };
    cmd.args(&launch_args)
        .current_dir(approved_copy.as_deref().unwrap_or(Path::new(work_dir)))
        .stdin(if stdin_bytes.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // SIGKILL the exec'd binary if the output future is dropped
        // before completion. Required so workflow-run cancellation
        // actually stops a long-running `cargo test` / `npm install`
        // when the runner drops the step-dispatch future. Without this,
        // the child gets reparented to PID 1 and keeps running.
        .kill_on_drop(true);

    // 2026-06-11 — setup + main share ONE deadline. The setup phase above
    // consumed `start.elapsed()` of the budget; the main command gets the
    // REMAINDER (floored at 1s), so a step with `exec_timeout_secs = T` can't
    // run for up to 2×T (setup T + main T) as it could before.
    let main_timeout_secs = timeout_secs
        .saturating_sub(start.elapsed().as_secs())
        .max(1);
    // A cut stdout stays whole on disk for the next steps (KT-1570); without
    // an artifacts directory nothing past the limit is held.
    let full_copy = ctx
        .get("run.artifacts_dir")
        .filter(|dir| output.template_facing && !dir.trim().is_empty())
        .map(|dir| full_output_path(Path::new(dir), &step.name));
    let stdout_kept = if full_copy.is_some() {
        output_limit_bytes.max(MAX_FULL_OUTPUT_BYTES)
    } else {
        output_limit_bytes
    };
    // When stdin is piped, stream it on a DETACHED task so stdout/stderr are
    // drained concurrently — a write-all-then-read sequence would deadlock the
    // moment the payload + the child's own output exceed the pipe buffers
    // (~64 KB), i.e. exactly the large-backlog case this field exists for.
    let output_future = run_bounded(&mut cmd, stdin_bytes, output_limit_bytes, stdout_kept);
    let timed_out =
        match tokio::time::timeout(Duration::from_secs(main_timeout_secs), output_future).await {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => {
                return fail(step, start, spawn_failure(&step.name, raw_command, &e));
            }
            Err(_) => Err(()), // timeout
        };

    let child = match timed_out {
        Ok(out) => out,
        Err(()) => {
            return fail(
                step,
                start,
                format!(
                    "Exec step `{}`: timed out after {}s (shared setup+main budget {}s).",
                    step.name, main_timeout_secs, timeout_secs
                ),
            );
        }
    };

    let exit_code = child.status.code();
    let CapturedText {
        text: stdout,
        truncated: stdout_truncated,
        full_output: stdout_file,
    } = captured_text(
        child.stdout,
        &secrets,
        output_limit_bytes,
        if output.template_facing {
            CutMarker::Explicit(full_copy.as_deref())
        } else {
            CutMarker::Legacy
        },
    );
    let CapturedText {
        text: stderr,
        truncated: stderr_truncated,
        ..
    } = captured_text(
        child.stderr,
        &secrets,
        output_limit_bytes,
        CutMarker::Legacy,
    );
    let duration_ms = start.elapsed().as_millis() as u64;

    let success = child.status.success();
    let status = if success {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };
    let mut summary = match exit_code {
        Some(_) if success => format!("exit 0 — {} ms", duration_ms),
        Some(code) => format!("exit {} — {} ms", code, duration_ms),
        None => format!("killed by signal — {} ms", duration_ms),
    };
    if needs_gh_hint(raw_command, success, github_env) {
        summary.push_str(GH_NOT_CONNECTED_HINT);
    }
    if stdout_truncated && output.template_facing {
        let warning = stdout_truncation_warning(output_limit_bytes, stdout_file.is_some());
        tracing::warn!(target: "kronn::workflow_exec", step = %step.name, "{warning}");
        summary.push_str(" — ");
        summary.push_str(&warning);
    }

    // Structured envelope so `{{steps.<name>.data.exit_code}}` etc.
    // resolve in downstream steps. Mirrors notify_step's contract.
    let envelope = serde_json::json!({
        "data": {
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
            "stdout_file": stdout_file,
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
    let signal_generic = if success {
        "[SIGNAL: OK]"
    } else {
        "[SIGNAL: ERROR]"
    };
    let signal_exit = exit_code
        .map(|c| format!("[SIGNAL: exit_{}]", c))
        .unwrap_or_else(|| "[SIGNAL: killed]".to_string());
    let envelope_str = format!(
        "{}{}\n\n---STEP_OUTPUT---\n{}\n---END_STEP_OUTPUT---\n{}\n{}",
        // 0.8.2 — Setup output (if any) lands ABOVE the main summary so
        // operators inspecting the step see the install logs first when
        // expanding the row. Empty string when no setup ran.
        setup_output_prefix,
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
            tokens_used: Some(0),
            duration_ms,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
            output_bytes: None,
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
            tokens_used: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result: None,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            agent_provenance: None,
            native_tool_calls: Box::default(),
            cached_prompt_tokens: None,
            cache_write_prompt_tokens: None,
            last_activity: None,
            quota_wait: None,
            terminal_stop: None,
            output_bytes: None,
        },
        condition_action: None,
    }
}

/// Bytes read past the limit are counted, not kept, so a chatty command
/// cannot grow the backend's memory; the margin keeps a secret straddling
/// the cut whole for the scrubber.
const CAPTURE_MARGIN_BYTES: usize = 4096;

/// What a child stream left: at most the limit plus the margin, and how many
/// bytes were dropped past that.
struct Captured {
    bytes: Vec<u8>,
    dropped: u64,
}

struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Captured,
    stderr: Captured,
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    keep: usize,
) -> std::io::Result<Captured> {
    let mut bytes = Vec::with_capacity(keep.min(64 * 1024));
    let mut dropped = 0u64;
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buf).await?;
        if read == 0 {
            return Ok(Captured { bytes, dropped });
        }
        let take = keep.saturating_sub(bytes.len()).min(read);
        bytes.extend_from_slice(&buf[..take]);
        dropped += (read - take) as u64;
    }
}

/// Spawns `cmd` and drains stdout and stderr concurrently with the stdin
/// feed, keeping at most `stdout_kept` / `limit` of each plus the margin.
async fn run_bounded(
    cmd: &mut tokio::process::Command,
    stdin: Option<Vec<u8>>,
    limit: usize,
    stdout_kept: usize,
) -> std::io::Result<BoundedOutput> {
    let keep = limit.saturating_add(CAPTURE_MARGIN_BYTES);
    let stdout_keep = stdout_kept.max(limit).saturating_add(CAPTURE_MARGIN_BYTES);
    let mut child = cmd.spawn()?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&bytes).await;
            let _ = pipe.shutdown().await;
        });
    }
    let missing = || std::io::Error::other("child stream was not piped");
    let stdout = child.stdout.take().ok_or_else(missing)?;
    let stderr = child.stderr.take().ok_or_else(missing)?;
    let (stdout, stderr, status) = tokio::try_join!(
        read_bounded(stdout, stdout_keep),
        read_bounded(stderr, keep),
        child.wait()
    )?;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

/// Most of a cut stdout kept on disk for the next steps (`data.stdout_file`).
const MAX_FULL_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Where cut stdouts are kept, inside the run's artifacts directory.
const FULL_OUTPUT_DIR: &str = "exec-output";

/// A stream as the step records it: scrubbed, cut to the limit with a marker
/// when it overflowed, and where its fuller copy was kept, if any.
struct CapturedText {
    text: String,
    truncated: bool,
    full_output: Option<String>,
}

/// How an Exec step keeps its output: the limit, and whether its stdout is
/// read by templates (KT-1570: explicit marker and full copy when cut).
#[derive(Clone, Copy)]
struct OutputPolicy {
    limit: usize,
    template_facing: bool,
}

impl OutputPolicy {
    /// A workflow Exec step, whose stdout later steps interpolate.
    fn workflow_step(limit: usize) -> Self {
        Self {
            limit,
            template_facing: true,
        }
    }

    /// Collectors and Quick Exec runs: the historical suffix, nothing on disk.
    fn plain(limit: usize) -> Self {
        Self {
            limit,
            template_facing: false,
        }
    }
}

/// A stream cut with the historical suffix: setup output.
fn plain_text(
    captured: Captured,
    secrets: &crate::core::secret_scrub::SecretSet,
    limit: usize,
) -> String {
    captured_text(captured, secrets, limit, CutMarker::Legacy).text
}

/// What a cut stream ends with.
#[derive(Clone, Copy)]
enum CutMarker<'a> {
    /// The historical suffix: stderr, setup output, collectors.
    Legacy,
    /// Sizes and, when a path is given, the full copy written there.
    Explicit(Option<&'a Path>),
}

/// Scrubbed text of a captured stream cut to `limit`. Owned so the raw bytes
/// are freed before the scrubbed copy is made.
fn captured_text(
    captured: Captured,
    secrets: &crate::core::secret_scrub::SecretSet,
    limit: usize,
    marker: CutMarker<'_>,
) -> CapturedText {
    let Captured { bytes, dropped } = captured;
    let total = (bytes.len() as u64).saturating_add(dropped);
    let text = {
        let raw = String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        secrets.scrub(&raw)
    };
    if text.len() <= limit && dropped == 0 {
        return CapturedText {
            text,
            truncated: false,
            full_output: None,
        };
    }
    let (saved, suffix_for) = match marker {
        CutMarker::Legacy => (None, None),
        CutMarker::Explicit(full_copy) => (
            full_copy.and_then(|path| save_full_output(path, &text, dropped > 0)),
            Some(total),
        ),
    };
    let mut cut = text;
    cut.truncate(char_boundary_at_most(&cut, limit));
    let suffix = match suffix_for {
        Some(total) => truncation_marker(cut.len(), total, saved.as_ref()),
        None => format!(
            "\n\n{LEGACY_TRUNCATION_MARKER_PREFIX}{} KB ...]",
            limit / 1024
        ),
    };
    cut.push_str(&suffix);
    cut.shrink_to_fit();
    CapturedText {
        text: cut,
        truncated: true,
        full_output: saved.map(|saved| saved.path),
    }
}

/// A cut stdout written to disk: its path and whether it holds everything.
struct SavedOutput {
    path: String,
    complete: bool,
}

/// Writes the already-scrubbed `text` owner-only at `path`; a failure only
/// costs the copy, never the step.
fn save_full_output(path: &Path, text: &str, source_cut: bool) -> Option<SavedOutput> {
    let end = char_boundary_at_most(text, MAX_FULL_OUTPUT_BYTES);
    match write_without_following_links(path, &text.as_bytes()[..end]) {
        Ok(()) => Some(SavedOutput {
            path: path.to_string_lossy().into_owned(),
            complete: !source_cut && end == text.len(),
        }),
        Err(error) => {
            tracing::warn!(
                target: "kronn::workflow_exec",
                path = %path.display(),
                "full Exec output not kept: {error}"
            );
            None
        }
    }
}

/// Exec steps write in the artifacts directory too, so the write goes
/// through the run directory's fd (KT-1585): a link planted at `exec-output`
/// is refused and one at the file is replaced, never written through.
fn write_without_following_links(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let missing = || std::io::Error::from(std::io::ErrorKind::NotFound);
    let name = path.file_name().ok_or_else(missing)?;
    let artifacts_dir = path.parent().and_then(Path::parent).ok_or_else(missing)?;
    crate::workflows::run_artifacts::open_run_dir(artifacts_dir)?
        .create_child(FULL_OUTPUT_DIR)?
        .write_file(name, bytes)
}

/// Where a step's cut stdout goes in the run's artifacts directory; the hash
/// keeps two step names that sanitize alike apart.
fn full_output_path(artifacts_dir: &Path, step_name: &str) -> PathBuf {
    let readable: String = step_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let key = crate::core::repository_resources::sha256(step_name.as_bytes());
    artifacts_dir
        .join(FULL_OUTPUT_DIR)
        .join(format!("{readable}-{}.stdout.txt", &key[..8]))
}

/// The step summary's warning for a cut stdout, so the run says it too.
fn stdout_truncation_warning(limit: usize, kept_on_disk: bool) -> String {
    let limit = kib(limit as u64);
    if kept_on_disk {
        format!("WARNING: stdout cut at {limit} KB, full output in data.stdout_file")
    } else {
        format!("WARNING: stdout cut at {limit} KB, full output not kept")
    }
}

fn kib(bytes: u64) -> u64 {
    bytes.div_ceil(1024)
}

/// The line closing a cut stream, readable by an agent that only sees the
/// interpolated text.
fn truncation_marker(kept: usize, total: u64, saved: Option<&SavedOutput>) -> String {
    let head = format!(
        "\n\n{TRUNCATION_MARKER_PREFIX}{} KB of {} KB",
        kib(kept as u64),
        kib(total)
    );
    match saved {
        Some(saved) if saved.complete => format!("{head} — full output: {}]", saved.path),
        Some(saved) => format!(
            "{head} — first {} KB: {}]",
            kib(MAX_FULL_OUTPUT_BYTES as u64),
            saved.path
        ),
        None => format!("{head}]"),
    }
}

fn char_boundary_at_most(s: &str, limit: usize) -> usize {
    let mut cut = limit.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    cut
}

/// 2026-06-11 — returns `Some(reason)` when a binary+args invocation is one
/// of the few UNAMBIGUOUSLY irreversible foot-guns the allowlist can't catch
/// (it only authorises the binary). Deliberately narrow: force-push (rewrites
/// shared history) and recursive-force `rm` (unrecoverable deletion). Not a
/// sandbox — a guardrail against the obvious irreversible operations. Args
/// are matched AFTER templating (so a `{{var}}` rendering to `--force` is
/// caught). Everything else (cargo test, npm install, git diff/status/add,
/// make…) passes untouched.
fn destructive_reason(cmd: &str, args: &[String]) -> Option<String> {
    let a: Vec<&str> = args.iter().map(|s| s.trim()).collect();
    let has = |needle: &str| a.contains(&needle);
    // A short-flag bundle like `-rf` / `-fr` that contains a given letter.
    let short_flag_has = |letter: char| {
        a.iter().any(|x| {
            x.starts_with('-') && !x.starts_with("--") && x.chars().skip(1).any(|c| c == letter)
        })
    };
    match cmd {
        "git"
            if has("push")
                && (has("--force")
                    || has("-f")
                    || has("--force-with-lease")
                    || a.iter().any(|x| x.starts_with("--force-with-lease="))) =>
        {
            Some("git push --force rewrites shared remote history".to_string())
        }
        "rm" if (has("--recursive") || short_flag_has('r'))
            && (has("--force") || short_flag_has('f')) =>
        {
            Some("rm -rf is an irreversible recursive delete".to_string())
        }
        _ => None,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    /// A run directory laid out like a real one (`<base>/<own>/<key>/<run>`),
    /// which Kronn opens from its root fd (KT-1585).
    fn run_dir() -> (tempfile::TempDir, PathBuf) {
        let base = tempfile::tempdir().unwrap();
        let run = base.path().join("kronn-run-artifacts/key/run");
        std::fs::create_dir_all(&run).unwrap();
        (base, run)
    }

    #[test]
    fn an_exec_step_child_gets_a_built_environment() {
        let command = crate::core::child_env::with_parent_env(
            &[
                ("PATH", "/usr/bin"),
                ("CARGO_HOME", "/home/u/.cargo"),
                ("KRONN_AUTH_TOKEN", "admin"),
                ("KRONN_ENCRYPTION_KEK", "raw"),
                ("ANTHROPIC_API_KEY", "sk-ant"),
                ("GH_TOKEN", "ghp"),
            ],
            || exec_child("cargo", &[], &[]),
        );
        let env: Vec<String> = command
            .as_std()
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for kept in ["PATH", "CARGO_HOME"] {
            assert!(env.iter().any(|name| name == kept), "{kept} missing");
        }
        assert!(
            !env.iter().any(|name| name == "GH_TOKEN"),
            "an unconnected project's step never inherits the backend's GitHub token"
        );
        let connected = exec_child("gh", &[("GH_TOKEN".into(), "project-token".into())], &[]);
        assert!(
            connected
                .as_std()
                .get_envs()
                .any(|(name, value)| name == "GH_TOKEN"
                    && value.is_some_and(|v| v == "project-token"))
        );
        for gone in [
            "KRONN_AUTH_TOKEN",
            "KRONN_ENCRYPTION_KEK",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(
                !env.iter().any(|name| name == gone),
                "{gone} reached the step"
            );
        }
    }

    /// KT-1633: an Exec step starts at background priority — behind the host's
    /// launcher, whichever of the main command and the approved-script command
    /// it is — and keeps its built environment. The owner's switch and a host
    /// without the launcher both leave the program itself.
    #[cfg(unix)] // asserts the taskpolicy/nice launchers, POSIX only
    #[test]
    fn an_exec_step_child_runs_behind_the_priority_launcher() {
        use background_priority::{plan, HostOs};
        assert_eq!(EXEC_STEP_PRIORITY, Priority::Background);
        for (os, launcher, flags) in [
            (HostOs::MacOs, "/usr/sbin/taskpolicy", vec!["-b"]),
            (HostOs::Linux, "/usr/bin/nice", vec!["-n", "10"]),
        ] {
            let background = plan(EXEC_STEP_PRIORITY, true, os, |_| true);
            let command = isolated_exec_child(step_command_planned("cargo", background), &[], &[]);
            let command = command.as_std();
            assert_eq!(command.get_program(), launcher, "{os:?}");
            let args: Vec<_> = command.get_args().collect();
            let mut expected = flags;
            expected.push("cargo");
            assert_eq!(args, expected, "{os:?}");
            assert!(
                command.get_envs().any(|(name, _)| name == "PATH"),
                "the launcher must not cost the step its built environment"
            );

            let off = plan(EXEC_STEP_PRIORITY, false, os, |_| true);
            let command = step_command_planned("cargo", off);
            assert_eq!(command.as_std().get_program(), "cargo");
            assert_eq!(command.as_std().get_args().count(), 0);
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // ── destructive_reason (2026-06-11 allowlist arg-hardening) ──
    #[test]
    fn destructive_blocks_force_push_all_spellings() {
        assert!(destructive_reason("git", &s(&["push", "--force"])).is_some());
        assert!(destructive_reason("git", &s(&["push", "-f"])).is_some());
        assert!(
            destructive_reason("git", &s(&["push", "origin", "main", "--force-with-lease"]))
                .is_some()
        );
    }

    #[test]
    fn destructive_blocks_rm_rf_bundled_and_split() {
        assert!(destructive_reason("rm", &s(&["-rf", "/tmp/x"])).is_some());
        assert!(destructive_reason("rm", &s(&["-fr", "x"])).is_some());
        assert!(destructive_reason("rm", &s(&["-r", "-f", "x"])).is_some());
        assert!(destructive_reason("rm", &s(&["--recursive", "--force", "x"])).is_some());
    }

    #[test]
    fn destructive_allows_legit_invocations() {
        assert!(destructive_reason("git", &s(&["push", "origin", "main"])).is_none());
        assert!(destructive_reason("git", &s(&["diff", "--stat"])).is_none());
        assert!(destructive_reason("git", &s(&["add", "-f", "ignored.txt"])).is_none()); // -f on add ≠ push
        assert!(destructive_reason("rm", &s(&["file.txt"])).is_none()); // non-recursive
        assert!(destructive_reason("cargo", &s(&["test", "--", "--nocapture"])).is_none());
        assert!(destructive_reason("npm", &s(&["install"])).is_none());
    }

    fn exec_step(
        name: &str,
        command: Option<&str>,
        args: Vec<&str>,
        timeout_secs: Option<u32>,
    ) -> WorkflowStep {
        WorkflowStep {
            exec_approved_by_trust: None,
            id: None,
            name: name.into(),
            step_type: StepType::Exec,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::Structured,
            on_result: vec![],
            on_timeout: None,
            agent_settings: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            mcp_config_ids: vec![],
            skill_ids: vec![],
            external_effect: false,
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
            api_response: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: command.map(String::from),
            exec_args: args.into_iter().map(String::from).collect(),
            exec_timeout_secs: timeout_secs,
            exec_setup_command: None,
            exec_setup_args: vec![],
            exec_stdin: None,
            quick_prompt_id: None,
            quick_prompt_version: None,
            quick_prompt_variables: std::collections::HashMap::new(),
            json_data_payload: None,
            collect_api_data: None,
            transform_data: None,
            page_publish: None,
            task_board: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
            room_id: None,
            read_only_repos: vec![],
            delegate_subtasks: None,
            exec_script_files: vec![],
            exec_unmodelled_args_approved: None,
            exec_agent_written: None,
            exec_agent_lines: vec![],
            sub_workflow_variables: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    #[serial_test::serial(github_machine_token)]
    async fn a_workflow_exec_step_gets_github_only_from_a_connected_project() {
        use crate::core::github_connection::{override_machine_token_for_tests, set_grant};
        use crate::models::GithubConnectionMode;
        override_machine_token_for_tests(Some(Some("gho_machineTOKEN")));
        let step = exec_step("token", Some("printenv"), vec!["GH_TOKEN"], None);
        let allow = vec!["printenv".to_string()];
        let ctx = TemplateContext::new();
        let dir = tempfile::tempdir().unwrap();
        let work_dir = dir.path().to_str().unwrap();

        let off = execute_exec_step_for_project(
            &step,
            &allow,
            work_dir,
            &ctx,
            Some("r16-exec-off"),
            None,
            MAX_OUTPUT_BYTES,
        )
        .await;
        assert_eq!(
            off.result.status,
            RunStatus::Failed,
            "{}",
            off.result.output
        );
        assert!(!off.result.output.contains("gho_machineTOKEN"));

        set_grant("r16-exec-on", GithubConnectionMode::GhLogin, None);
        let on = execute_exec_step_for_project(
            &step,
            &allow,
            work_dir,
            &ctx,
            Some("r16-exec-on"),
            None,
            MAX_OUTPUT_BYTES,
        )
        .await;
        // `printenv` exits 0 only when the variable is set; its echo is scrubbed.
        assert_eq!(on.result.status, RunStatus::Success, "{}", on.result.output);
        assert!(!on.result.output.contains("gho_machineTOKEN"));
        assert!(on.result.output.contains("***"), "{}", on.result.output);

        let none = execute_exec_step_for_project(
            &step,
            &allow,
            work_dir,
            &ctx,
            None,
            None,
            MAX_OUTPUT_BYTES,
        )
        .await;
        assert_eq!(
            none.result.status,
            RunStatus::Failed,
            "a project-less run gets no token"
        );
        override_machine_token_for_tests(None);
    }

    #[test]
    fn the_output_limit_rises_only_for_a_step_a_publish_reads() {
        let exec = exec_step("agrege", Some("cat"), vec![], None);
        let other = exec_step("other", Some("cat"), vec![], None);
        let publish = WorkflowStep {
            name: "publish".into(),
            step_type: StepType::PublishPageData,
            page_publish: Some(PublishPageDataConfig {
                page_id: "errors".into(),
                writes: vec![PublishPageDataWrite {
                    dataset: "errors".into(),
                    operation: LivePageWriteOperation::Replace,
                    value_from: "{{steps.agrege.data.stdout}}".into(),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            }),
            ..WorkflowStep::default()
        };
        let steps = [exec.clone(), other.clone(), publish];
        assert_eq!(
            output_limit_for(&exec, &steps, &[]),
            MAX_PUBLISH_OUTPUT_BYTES
        );
        assert_eq!(output_limit_for(&other, &steps, &[]), MAX_OUTPUT_BYTES);
        // `previous_step` is the step listed just before the publish.
        let mut alias = steps[2].clone();
        alias.page_publish.as_mut().unwrap().writes[0].value_from =
            "previous_step.data.stdout".into();
        let steps = [exec.clone(), other.clone(), alias];
        assert_eq!(
            output_limit_for(&other, &steps, &[]),
            MAX_PUBLISH_OUTPUT_BYTES
        );
        assert_eq!(output_limit_for(&exec, &steps, &[]), MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn a_github_token_echoed_in_a_large_json_stdout_is_scrubbed() {
        let token = "ghp_kt1105SecretTokenValue";
        let mut step = exec_step("agrege", Some("cat"), vec![], Some(30));
        let padding = "x".repeat(150 * 1024);
        step.exec_stdin = Some(format!(r#"{{"token":"{token}","pad":"{padding}"}}"#));
        let dir = tempfile::tempdir().unwrap();
        let outcome = execute_exec_step_inner(
            &step,
            &["cat".into()],
            dir.path().to_str().unwrap(),
            &TemplateContext::new(),
            OutputPolicy::workflow_step(MAX_PUBLISH_OUTPUT_BYTES),
            &[("GH_TOKEN".into(), token.into())],
            None,
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(!outcome.result.output.contains(token));
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("agrege", &outcome.result.output);
        let stdout = ctx.resolve_value("steps.agrege.data.stdout").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(stdout.as_str().unwrap()).unwrap();
        assert_eq!(parsed["token"], "***");
        assert_eq!(parsed["pad"].as_str().unwrap().len(), padding.len());
        assert_eq!(
            ctx.resolve_value("steps.agrege.data.stdout_truncated"),
            Some(serde_json::Value::Bool(false))
        );
    }

    /// KT-1570 — a 150 KB stdout read by a later step's template says it was
    /// cut, warns on the step, and its full scrubbed copy is readable.
    #[tokio::test]
    async fn a_cut_stdout_is_marked_in_templates_and_kept_whole_on_disk() {
        let token = "ghp_kt1570SecretTokenValue";
        let body = format!(
            "{}\n---TICKET---\nlate section {token}\n",
            "a".repeat(150 * 1024)
        );
        let mut step = exec_step("ctx_review", Some("cat"), vec![], Some(30));
        step.exec_stdin = Some(body.clone());
        let work = tempfile::tempdir().unwrap();
        let (_artifacts_base, artifacts) = run_dir();
        let artifacts_dir = artifacts.as_path().to_string_lossy().into_owned();
        let mut ctx = TemplateContext::new();
        ctx.set_builtin("run.artifacts_dir", artifacts_dir.clone());
        let outcome = execute_exec_step_inner(
            &step,
            &["cat".into()],
            work.path().to_str().unwrap(),
            &ctx,
            OutputPolicy::workflow_step(MAX_OUTPUT_BYTES),
            &[("GH_TOKEN".into(), token.into())],
            None,
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(!outcome.result.output.contains(token));
        ctx.set_step_output("ctx_review", &outcome.result.output);

        let summary = ctx.resolve_value("steps.ctx_review.summary").unwrap();
        assert!(
            summary
                .as_str()
                .unwrap()
                .contains("WARNING: stdout cut at 100 KB, full output in data.stdout_file"),
            "{summary}"
        );
        let file = ctx
            .render_strict("{{steps.ctx_review.data.stdout_file}}")
            .unwrap();
        assert!(file.starts_with(&artifacts_dir), "{file}");
        let prompt = ctx
            .render_strict("Review:\n{{steps.ctx_review.data.stdout}}")
            .unwrap();
        assert!(
            prompt.ends_with(&format!(
                "[TRUNCATED: kept 100 KB of 151 KB — full output: {file}]"
            )),
            "{}",
            &prompt[prompt.len().saturating_sub(300)..]
        );
        assert!(!prompt.contains("---TICKET---"));
        // The marker names the artifacts directory, so the reading agent gets it.
        let reader = exec_step("review", None, vec![], None);
        assert_eq!(
            crate::workflows::steps::artifacts_read_only_dirs(&reader, &ctx, &prompt),
            vec![artifacts_dir.clone()]
        );

        let full = std::fs::read_to_string(&file).unwrap();
        assert_eq!(full, body.replace(token, "***"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // An output under the limit keeps no file and says nothing.
        step.exec_stdin = Some("short".into());
        let small =
            execute_exec_step(&step, &["cat".into()], work.path().to_str().unwrap(), &ctx).await;
        ctx.set_step_output("ctx_review", &small.result.output);
        assert_eq!(
            ctx.resolve_value("steps.ctx_review.data.stdout_file"),
            Some(serde_json::Value::Null)
        );
        assert!(!small.result.output.contains("WARNING"));
    }

    /// KT-1570 — a collector keeps the historical suffix and writes nothing:
    /// a `Lines` item must never carry a marker naming a local path.
    #[tokio::test]
    async fn a_cut_collector_output_keeps_the_old_suffix_and_no_file() {
        let mut step = exec_step("source", Some("cat"), vec![], Some(30));
        step.exec_stdin = Some("line\n".repeat(250_000));
        let work = tempfile::tempdir().unwrap();
        let (_artifacts_base, artifacts) = run_dir();
        let mut ctx = TemplateContext::new();
        ctx.set_builtin("run.artifacts_dir", artifacts.as_path().to_string_lossy());
        let outcome = execute_exec_step_with_output_limit(
            &step,
            &["cat".into()],
            work.path().to_str().unwrap(),
            &ctx,
            MAX_COLLECT_OUTPUT_BYTES,
            None,
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(!outcome.result.output.contains(TRUNCATION_MARKER_PREFIX));
        assert!(!outcome.result.output.contains("WARNING"));
        ctx.set_step_output("source", &outcome.result.output);
        let data = ctx.resolve_value("steps.source.data").unwrap();
        assert_eq!(data["stdout_truncated"], true);
        assert_eq!(data["stdout_file"], serde_json::Value::Null);
        let lines = crate::workflows::collect_api_data_step::quick_exec_value(
            &data,
            crate::models::CollectQuickExecOutputFormat::Lines,
        )
        .unwrap();
        let lines = lines.as_array().unwrap();
        assert_eq!(
            lines.last().unwrap(),
            "[... output tronqué — limite 1024 KB ...]"
        );
        let artifacts_dir = artifacts.as_path().to_string_lossy().into_owned();
        assert!(lines
            .iter()
            .all(|line| !line.as_str().unwrap().contains(&artifacts_dir)));
        assert!(!artifacts.as_path().join(FULL_OUTPUT_DIR).exists());
    }

    /// A link planted where the copy goes is replaced, never written through;
    /// a linked output directory is refused.
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn the_full_copy_never_writes_through_a_planted_link() {
        let (_artifacts_base, artifacts) = run_dir();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("victim");
        std::fs::write(&target, "untouched").unwrap();
        let path = full_output_path(artifacts.as_path(), "ctx_review");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        write_without_following_links(&path, b"full").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
        assert!(!std::fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "full");

        let (_linked_base, linked) = run_dir();
        std::os::unix::fs::symlink(outside.path(), linked.as_path().join(FULL_OUTPUT_DIR)).unwrap();
        let path = full_output_path(linked.as_path(), "ctx_review");
        assert!(write_without_following_links(&path, b"full").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    }

    /// KT-1047 — a stream of tens of MB is read through, never held.
    #[tokio::test]
    async fn a_bounded_read_keeps_the_limit_and_counts_the_rest() {
        let total = 40 * 1024 * 1024u64;
        let source = tokio::io::AsyncReadExt::take(tokio::io::repeat(b'x'), total);
        let captured = read_bounded(source, 1000).await.unwrap();
        assert_eq!(captured.bytes.len(), 1000);
        assert_eq!(captured.dropped, total - 1000);
    }

    #[tokio::test]
    async fn a_command_writing_tens_of_mb_is_cut_with_a_marker() {
        let step = exec_step(
            "chatty",
            Some("head"),
            vec!["-c", "31457280", "/dev/zero"],
            Some(60),
        );
        let dir = tempfile::tempdir().unwrap();
        let outcome = execute_exec_step(
            &step,
            &["head".into()],
            dir.path().to_str().unwrap(),
            &TemplateContext::new(),
        )
        .await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "{}",
            &outcome.result.output[..200.min(outcome.result.output.len())]
        );
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("chatty", &outcome.result.output);
        let stdout = ctx.resolve_value("steps.chatty.data.stdout").unwrap();
        let stdout = stdout.as_str().unwrap();
        let legacy = "\n\n[... output tronqué — limite 100 KB ...]";
        assert!(stdout.ends_with(legacy));
        assert!(stdout.len() <= MAX_OUTPUT_BYTES + legacy.len());
        assert_eq!(
            ctx.resolve_value("steps.chatty.data.stdout_truncated"),
            Some(serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn a_failed_gh_step_without_a_token_says_why() {
        let token = vec![("GH_TOKEN".to_string(), "t".to_string())];
        assert!(needs_gh_hint("gh", false, &[]));
        assert!(!needs_gh_hint("gh", true, &[]));
        assert!(!needs_gh_hint("gh", false, &token));
        assert!(!needs_gh_hint("git", false, &[]));
    }

    /// KT-1138: the readiness verdict reports exactly the refusal the
    /// executor gives before spawning, on the main and the setup line.
    #[tokio::test]
    async fn readiness_reports_the_executor_s_static_refusals() {
        let allow = vec!["git".to_string(), "echo".to_string()];
        let mut main = exec_step(
            "push",
            Some("git"),
            vec!["push", "--force-with-lease"],
            None,
        );
        let mut setup = exec_step("prep", Some("echo"), vec!["ok"], None);
        setup.exec_setup_command = Some("git".into());
        setup.exec_setup_args = vec!["push".into(), "-f".into()];
        let mut templated = exec_step("later", Some("git"), vec!["push", "{{mode}}"], None);
        templated.exec_unmodelled_args_approved = Some(true);
        for (step, phase) in [(&mut main, "main"), (&mut setup, "setup")] {
            let outcome = execute_exec_step(step, &allow, "/tmp", &TemplateContext::new()).await;
            assert_eq!(outcome.result.status, RunStatus::Failed);
            let mut wf: Workflow = serde_json::from_value(serde_json::json!({
                "id": "w", "name": "w", "project_id": null, "trigger": {"type": "Manual"},
                "steps": [], "actions": [],
                "safety": {"sandbox": false, "max_files": null, "max_lines": null, "require_approval": false},
                "enabled": true, "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap();
            wf.exec_allowlist = allow.clone();
            wf.steps = vec![step.clone()];
            let verdict = crate::workflows::readiness::assess(&wf, &Default::default());
            assert!(!verdict.ready, "{phase}");
            let blocker = &verdict.blockers[0];
            assert_eq!(blocker.phase.as_deref(), Some(phase));
            assert_eq!(blocker.message, outcome.result.output, "{phase}");
        }
        // A template is decided by the run once rendered, not guessed here.
        assert!(static_refusals(&templated, &allow).is_empty());
    }

    #[tokio::test]
    async fn rejects_when_allowlist_empty() {
        let step = exec_step("run", Some("echo"), vec!["hi"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &[], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("allowlist"),
            "got: {}",
            outcome.result.output
        );
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
        assert!(
            !outcome.result.output.contains("spawn"),
            "old misleading 'failed to spawn' message must not appear, got: {}",
            outcome.result.output
        );
        // Must mention the real cause + the fix.
        assert!(
            outcome.result.output.contains("projet"),
            "error must point at the missing project, got: {}",
            outcome.result.output
        );
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
        )
        .await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("introuvable")
                || outcome.result.output.contains("not found"),
            "got: {}",
            outcome.result.output
        );
    }

    // The next 3 tests actually invoke a binary — gate behind cfg(unix)
    // because Windows CI doesn't have `echo` as a discrete binary
    // (it's a cmd.exe builtin), and the cross-platform discipline is
    // already enforced by the allowlist + cmd::async_cmd helpers.
    #[cfg(unix)] // runs `echo` as a program, which Windows lacks (a cmd.exe builtin there)
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

    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
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
            "shell meta arg must be passed literally, got: {}",
            outcome.result.output
        );
    }

    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
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
            outcome
                .result
                .output
                .contains("from templated; with metas $(whoami)"),
            "template rendered, then passed as literal — got: {}",
            outcome.result.output
        );
    }

    #[cfg(unix)] // relies on POSIX process groups and signals
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

    fn captured(text: &str, dropped: u64) -> Captured {
        Captured {
            bytes: text.as_bytes().to_vec(),
            dropped,
        }
    }

    #[test]
    fn truncation_marker_appended_when_output_huge() {
        let huge = "x".repeat(MAX_OUTPUT_BYTES + 1024);
        let secrets = crate::core::secret_scrub::SecretSet::new();
        let explicit = CutMarker::Explicit(None);
        let cut = captured_text(captured(&huge, 0), &secrets, MAX_OUTPUT_BYTES, explicit);
        assert!(cut.truncated);
        assert!(cut.text.ends_with("\n\n[TRUNCATED: kept 100 KB of 101 KB]"));
        assert_eq!(cut.full_output, None);
    }

    #[test]
    fn truncation_respects_utf8_boundary() {
        // A multibyte char straddling the limit must not split mid-codepoint
        // (invalid UTF-8 crashes JSON encoding downstream).
        let s = format!("{}é-tail", "x".repeat(MAX_OUTPUT_BYTES - 1));
        let secrets = crate::core::secret_scrub::SecretSet::new();
        let cut = captured_text(
            captured(&s, 0),
            &secrets,
            MAX_OUTPUT_BYTES,
            CutMarker::Legacy,
        );
        assert!(cut.text.starts_with(&"x".repeat(MAX_OUTPUT_BYTES - 1)));
        assert!(!cut.text.contains("é-tail"));
    }

    #[test]
    fn collector_limit_keeps_medium_json_that_normal_exec_truncates() {
        let medium = "x".repeat(MAX_OUTPUT_BYTES + 1024);
        let secrets = crate::core::secret_scrub::SecretSet::new();
        let legacy = CutMarker::Legacy;
        assert!(captured_text(captured(&medium, 0), &secrets, MAX_OUTPUT_BYTES, legacy).truncated);
        let kept = captured_text(
            captured(&medium, 0),
            &secrets,
            MAX_COLLECT_OUTPUT_BYTES,
            legacy,
        );
        assert_eq!(kept.text, medium);
        assert!(!kept.truncated);
    }

    /// Past the on-disk cap too, the marker says the file is partial.
    #[test]
    fn a_stdout_past_the_disk_cap_says_its_file_is_partial() {
        let (_dir_base, dir) = run_dir();
        let path = full_output_path(dir.as_path(), "big step/1");
        let text = "y".repeat(MAX_FULL_OUTPUT_BYTES + 10);
        let secrets = crate::core::secret_scrub::SecretSet::new();
        let marker = CutMarker::Explicit(Some(&path));
        let cut = captured_text(captured(&text, 1024), &secrets, MAX_OUTPUT_BYTES, marker);
        let path_text = path.to_string_lossy();
        assert!(cut.text.ends_with(&format!(
            "[TRUNCATED: kept 100 KB of 8194 KB — first 8192 KB: {path_text}]"
        )));
        assert_eq!(cut.full_output.as_deref(), Some(path_text.as_ref()));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            MAX_FULL_OUTPUT_BYTES as u64
        );
        assert!(path.starts_with(dir.as_path().join(FULL_OUTPUT_DIR)));
        assert!(path_text.contains("big_step_1-"));
    }

    // ─── on_result + SIGNAL emission tests ────────────────────────────
    //
    // Auto-dev pattern: cargo test fails → loop back to implement.
    // Without these, the runner saw `Failed` and short-circuited to
    // rollback — defeating the point of declaring an `on_result` rule.

    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
    #[tokio::test]
    async fn success_appends_signal_ok_and_exit_0() {
        let step = exec_step("greet", Some("echo"), vec!["hi"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".to_string()], "/tmp", &ctx).await;
        assert!(outcome.result.output.contains("[SIGNAL: OK]"));
        assert!(outcome.result.output.contains("[SIGNAL: exit_0]"));
    }

    #[cfg(unix)] // relies on POSIX process groups and signals
    #[tokio::test]
    async fn nonzero_exit_appends_signal_error_and_exit_code() {
        let step = exec_step("fail", Some("false"), vec![], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["false".to_string()], "/tmp", &ctx).await;
        assert!(outcome.result.output.contains("[SIGNAL: ERROR]"));
        assert!(outcome.result.output.contains("[SIGNAL: exit_1]"));
    }

    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
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
            Some(ConditionAction::Goto {
                step_name,
                max_iterations,
            }) => {
                assert_eq!(step_name, "implement");
                assert_eq!(max_iterations, Some(5));
            }
            other => panic!("expected Goto on_result match, got {:?}", other),
        }
        // condition_result also surfaces in the StepResult for the UI.
        assert_eq!(
            outcome.result.condition_result.as_deref(),
            Some("Goto:implement")
        );
    }

    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
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
    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
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

    // ─── 0.8.2 — Setup-phase tests ────────────────────────────────────────

    #[tokio::test]
    async fn setup_phase_runs_before_main_command_on_success() {
        // Setup `true` (always exits 0) → main `echo` should run.
        let mut step = exec_step("run", Some("echo"), vec!["main-output"], None);
        step.exec_setup_command = Some("true".into());
        step.exec_setup_args = vec![];
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".into(), "true".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "setup `true` should succeed and main `echo` should run; got: {}",
            outcome.result.output
        );
        // Setup output prefix lands BEFORE the main summary.
        assert!(
            outcome.result.output.contains("── Setup: `true"),
            "setup header missing from output: {}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("main-output"),
            "main `echo` output missing: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn setup_phase_failure_short_circuits_main_command() {
        // Setup `false` (always exits 1) → main MUST NOT RUN, step fails.
        // This is THE killer feature: if the dep install fails, we don't
        // run tests against a broken worktree and produce confusing output.
        let mut step = exec_step("run", Some("echo"), vec!["main-NEVER-printed"], None);
        step.exec_setup_command = Some("false".into());
        step.exec_setup_args = vec![];
        let ctx = TemplateContext::new();
        let outcome =
            execute_exec_step(&step, &["echo".into(), "false".into()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("setup"),
            "failure message must mention setup: {}",
            outcome.result.output
        );
        assert!(
            !outcome.result.output.contains("main-NEVER-printed"),
            "main echo must NOT have run after setup failure: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn setup_binary_not_in_allowlist_rejects_before_main() {
        // Setup uses a binary NOT in the allowlist → reject. Main shouldn't
        // even be attempted. Mirrors the same check as `exec_command`.
        let mut step = exec_step("run", Some("echo"), vec!["main"], None);
        step.exec_setup_command = Some("rm".into());
        step.exec_setup_args = vec![];
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".into()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.contains("setup binary"),
            "should name the setup binary as the culprit: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn setup_optional_when_none_keeps_legacy_behavior() {
        // No setup → behaves like the pre-0.8.2 executor (just runs main).
        let step = exec_step("run", Some("echo"), vec!["only-main"], None);
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["echo".into()], "/tmp", &ctx).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("only-main"));
        assert!(
            !outcome.result.output.contains("── Setup"),
            "no setup → no setup header: {}",
            outcome.result.output
        );
    }

    // ── 0.8.8 — exec_stdin (pipe input past the argv ARG_MAX ceiling) ──

    #[tokio::test]
    async fn stdin_piped_reaches_command() {
        // `cat` with no args echoes its stdin → proves the payload is piped.
        let mut step = exec_step("pipe", Some("cat"), vec![], None);
        step.exec_stdin = Some("HELLO-FROM-STDIN-42".into());
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["cat".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "got: {}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("HELLO-FROM-STDIN-42"),
            "stdin payload must reach the command's stdout: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn stdin_delivers_full_payload_past_argv_limit() {
        // 200 KB exceeds Linux's per-arg ceiling (MAX_ARG_STRLEN ~128 KB):
        // this CANNOT be passed as a single argv element, only via stdin.
        // `wc -c` counts the bytes it reads → proves EVERY byte arrived.
        let mut step = exec_step("count", Some("wc"), vec!["-c"], None);
        step.exec_stdin = Some("y".repeat(200_000));
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["wc".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "got: {}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("200000"),
            "wc -c must report the full 200000-byte payload: {}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn stdin_large_bidirectional_does_not_deadlock() {
        // `cat` reads 300 KB stdin AND writes 300 KB stdout. A
        // write-all-then-read sequence would deadlock once both pipes fill
        // (~64 KB). With concurrent streaming it completes fast; the bounded
        // timeout makes a regression fail in seconds instead of hanging.
        let mut step = exec_step("echo_big", Some("cat"), vec![], Some(30));
        step.exec_stdin = Some("x".repeat(300_000));
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["cat".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "large bidirectional pipe must not deadlock/timeout: {}",
            outcome.result.output
        );
        // >100 KB came back → stdout truncation marker proves the big echo ran.
        assert!(
            outcome.result.output.contains("tronqué"),
            "expected truncation marker after a 300 KB echo: {}",
            outcome.result.output
        );
    }

    /// R5-04: a trusted script shape runs only a regular file, never a link
    /// to a device such as stdin.
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[tokio::test]
    async fn a_script_linked_to_a_device_is_refused_at_run_time() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/dev/stdin", dir.path().join("tool.py")).unwrap();
        let mut step = exec_step("linked", Some("python3"), vec!["tool.py", "{{x}}"], None);
        step.exec_unmodelled_args_approved = Some(true);
        step.exec_stdin = Some("print('stdin ran')".into());
        let mut ctx = TemplateContext::new();
        ctx.set("x", "a");
        let outcome = execute_exec_step(
            &step,
            &["python3".into()],
            &dir.path().to_string_lossy(),
            &ctx,
        )
        .await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Failed,
            "{}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("fichier ordinaire"),
            "{}",
            outcome.result.output
        );
    }

    /// R6-08: an approved line whose stdin feeds a script runs only a regular
    /// file, never a link to stdin.
    #[cfg(unix)] // creates symlinks through std::os::unix
    #[tokio::test]
    async fn an_approved_stdin_fed_script_linked_to_stdin_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/dev/stdin", dir.path().join("x.py")).unwrap();
        let mut step = exec_step("linked", Some("python3"), vec!["-X", "dev", "x.py"], None);
        step.exec_unmodelled_args_approved = Some(true);
        step.exec_stdin = Some("{{x}}".into());
        let mut ctx = TemplateContext::new();
        ctx.set("x", "print('stdin ran')");
        let outcome = execute_exec_step(
            &step,
            &["python3".into()],
            &dir.path().to_string_lossy(),
            &ctx,
        )
        .await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Failed,
            "{}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("fichier ordinaire"),
            "{}",
            outcome.result.output
        );
    }

    /// F-03: a program file read by `awk -f` while a templated stdin feeds
    /// it must be a regular file too.
    #[cfg(unix)] // creates symlinks through std::os::unix; relies on POSIX process groups and signals
    #[tokio::test]
    async fn an_awk_program_file_linked_to_stdin_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/dev/stdin", dir.path().join("p.awk")).unwrap();
        let mut step = exec_step("linked", Some("awk"), vec!["-f", "p.awk"], None);
        step.exec_unmodelled_args_approved = Some(true);
        step.exec_stdin = Some("{{x}}".into());
        let mut ctx = TemplateContext::new();
        ctx.set("x", "BEGIN { print \"stdin ran\" }");
        let outcome =
            execute_exec_step(&step, &["awk".into()], &dir.path().to_string_lossy(), &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Failed,
            "{}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("fichier ordinaire"),
            "{}",
            outcome.result.output
        );
    }

    #[tokio::test]
    async fn stdin_is_templated_before_piping() {
        let mut step = exec_step("pipe", Some("cat"), vec![], None);
        step.exec_stdin = Some("{{steps.fetch.output}}".into());
        // `cat` reads paths, so a human approves the value it receives.
        step.exec_unmodelled_args_approved = Some(true);
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("fetch", "RENDERED-PAYLOAD-FROM-PREVIOUS-STEP");
        let outcome = execute_exec_step(&step, &["cat".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "got: {}",
            outcome.result.output
        );
        assert!(
            outcome
                .result
                .output
                .contains("RENDERED-PAYLOAD-FROM-PREVIOUS-STEP"),
            "exec_stdin must be template-rendered, not piped verbatim: {}",
            outcome.result.output
        );
    }

    /// The documented recipe for a templated value next to inline code: a
    /// separate argv entry the interpreter never parses. A hostile issue
    /// title must come back verbatim and nothing in it may run.
    #[cfg(unix)] // runs a POSIX shell script fixture; relies on POSIX process groups and signals
    #[tokio::test]
    async fn a_hostile_issue_title_never_executes_through_the_documented_recipes() {
        let title = "a'b\"c $(touch pwned1) ; touch pwned2 `touch pwned3` \\ é🦀";
        let recipes: [Vec<&str>; 2] = [
            vec![
                "-c",
                "printf -- '---STATE:title=%s---' \"$1\"",
                "_",
                "{{issue.title}}",
            ],
            vec![
                "-c",
                "set -e\nprintf -- '---STATE:title=%s---' \"$1\"",
                "_",
                "{{issue.title}}",
            ],
        ];
        for args in recipes {
            let dir = tempfile::tempdir().unwrap();
            let step = exec_step("safe", Some("bash"), args.clone(), None);
            let mut ctx = TemplateContext::new();
            ctx.set_issue(title, "", "1", "https://tracker.test/1", &[]);
            let outcome = execute_exec_step(
                &step,
                &["bash".to_string()],
                &dir.path().to_string_lossy(),
                &ctx,
            )
            .await;
            assert_eq!(
                outcome.result.status,
                RunStatus::Success,
                "{args:?}: {}",
                outcome.result.output
            );
            let (_, state) =
                crate::workflows::template::extract_step_markers(&outcome.result.output);
            assert_eq!(
                state.get("title").map(String::as_str),
                Some(title),
                "{args:?}"
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "{args:?}: the title ran as code"
            );
        }
    }

    /// A saved step that interpolates a value into inline code fails before
    /// spawning anything, main command or setup, with the step, the
    /// placeholder and the fix in its error.
    #[cfg(unix)] // runs a POSIX shell script fixture
    #[tokio::test]
    async fn a_step_interpolating_into_inline_code_is_refused_before_running() {
        let mut ctx = TemplateContext::new();
        ctx.set_issue("$(touch pwned)", "", "1", "https://tracker.test/1", &[]);
        let main = exec_step(
            "greet",
            Some("bash"),
            vec!["-c", "touch ran; echo {{issue.title}}"],
            None,
        );
        let mut setup = exec_step("greet", Some("bash"), vec!["-c", "touch ran"], None);
        setup.exec_setup_command = Some("bash".into());
        setup.exec_setup_args = vec!["-c".into(), "touch ran; echo {{issue.title}}".into()];
        for step in [main, setup] {
            let dir = tempfile::tempdir().unwrap();
            let outcome = execute_exec_step(
                &step,
                &["bash".to_string()],
                &dir.path().to_string_lossy(),
                &ctx,
            )
            .await;
            assert_eq!(outcome.result.status, RunStatus::Failed);
            let output = &outcome.result.output;
            assert!(
                output.contains("greet") && output.contains("{{issue.title}}"),
                "{output}"
            );
            assert!(
                output.contains("applique la correction proposée"),
                "{output}"
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "nothing ran"
            );
        }
    }

    /// Review findings, executed: a shell script after `-cx`, and a
    /// templated stdin given to a program that runs stdin, never run.
    #[cfg(unix)] // runs a POSIX shell script fixture; relies on POSIX process groups and signals
    #[tokio::test]
    async fn a_clustered_c_script_and_a_stdin_program_are_refused_before_running() {
        let mut ctx = TemplateContext::new();
        ctx.set_issue("touch pwned", "", "1", "https://tracker.test/1", &[]);
        let clustered = exec_step("greet", Some("bash"), vec!["-cx", "{{issue.title}}"], None);
        let mut stdin = exec_step("greet", Some("bash"), vec![], None);
        stdin.exec_stdin = Some("{{issue.title}}".into());
        let mut python_stdin = exec_step("greet", Some("python3"), vec!["-"], None);
        python_stdin.exec_stdin = Some("import os; os.system('{{issue.title}}')".into());
        for step in [clustered, stdin, python_stdin] {
            let dir = tempfile::tempdir().unwrap();
            let outcome = execute_exec_step(
                &step,
                &["bash".to_string(), "python3".to_string()],
                &dir.path().to_string_lossy(),
                &ctx,
            )
            .await;
            assert_eq!(
                outcome.result.status,
                RunStatus::Failed,
                "{}",
                outcome.result.output
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "nothing ran"
            );
        }
    }

    /// A value reaching a program Kronn does not model is refused before it
    /// runs, unless a human approved the step.
    #[cfg(unix)] // relies on POSIX process groups and signals
    #[tokio::test]
    async fn an_unmodelled_program_runs_only_with_a_human_approval() {
        let mut ctx = TemplateContext::new();
        ctx.set_issue(
            "--compress-program=touch",
            "",
            "1",
            "https://tracker.test/1",
            &[],
        );
        let mut step = exec_step("sorted", Some("sort"), vec!["{{issue.title}}"], None);
        let dir = tempfile::tempdir().unwrap();
        let workdir = dir.path().to_string_lossy().to_string();
        let refused = execute_exec_step(&step, &["sort".to_string()], &workdir, &ctx).await;
        assert_eq!(refused.result.status, RunStatus::Failed);
        assert!(
            refused.result.output.contains("refusé avant exécution"),
            "{}",
            refused.result.output
        );
        assert!(
            refused.result.output.contains("sort"),
            "{}",
            refused.result.output
        );
        step.exec_unmodelled_args_approved = Some(true);
        let approved = execute_exec_step(&step, &["sort".to_string()], &workdir, &ctx).await;
        assert!(
            !approved.result.output.contains("refusé avant exécution"),
            "{}",
            approved.result.output
        );
    }

    /// The envelope escapes stdout; markers must still come back as printed.
    #[cfg(unix)] // runs POSIX utilities (echo, false, sleep, cat) that Windows lacks as programs
    #[tokio::test]
    async fn multi_line_markers_come_back_without_literal_escapes() {
        let mut step = exec_step("sortie", Some("cat"), vec![], None);
        step.exec_stdin = Some(
            "log\n---STATE:plan=first line\nsecond line---\n\
             ---ARTIFACT:notes---\nline A\nline B\n---END_ARTIFACT---\n"
                .into(),
        );
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["cat".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "{}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("first line\\nsecond line"),
            "the envelope itself still carries the escaped stdout"
        );
        let (artifacts, state) =
            crate::workflows::template::extract_step_markers(&outcome.result.output);
        assert_eq!(
            state.get("plan").map(String::as_str),
            Some("first line\nsecond line")
        );
        assert_eq!(
            artifacts.get("notes").map(String::as_str),
            Some("line A\nline B")
        );
        let mut downstream = TemplateContext::new();
        downstream.set_step_output("sortie", &outcome.result.output);
        assert_eq!(
            downstream
                .render_strict("{{state.plan}}|{{artifacts.notes}}")
                .unwrap(),
            "first line\nsecond line|line A\nline B"
        );
    }

    #[tokio::test]
    async fn stdin_absent_keeps_null_no_hang() {
        // No exec_stdin → stdin is /dev/null → `cat` gets immediate EOF and
        // exits instead of blocking forever. The short timeout guards against
        // a regression that left stdin a dangling pipe.
        let step = exec_step("nopipe", Some("cat"), vec![], Some(15));
        let ctx = TemplateContext::new();
        let outcome = execute_exec_step(&step, &["cat".into()], "/tmp", &ctx).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "cat with null stdin must EOF immediately, not hang: {}",
            outcome.result.output
        );
    }

    // KT-918 — the approved copy is the module root for Python too: a helper
    // found through the cwd is the approved one, never the worktree's.
    #[tokio::test]
    async fn a_python_script_imports_its_approved_helper_from_the_copy() {
        let home = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let (_artifacts_base, artifacts) = run_dir();
        let entry = "import os, sys\nsys.path.insert(0, os.getcwd())\nimport helper\n\
            print('SENTINEL=' + helper.V + ' WT=' + os.environ['KRONN_WORKTREE'])\n";
        std::fs::write(home.path().join("entry.py"), entry).unwrap();
        std::fs::write(home.path().join("helper.py"), "V = 'APPROVED'\n").unwrap();
        std::fs::write(worktree.path().join("entry.py"), entry).unwrap();
        std::fs::write(worktree.path().join("helper.py"), "V = 'MUTATED'\n").unwrap();
        let mut step = exec_step("py", Some("python3"), vec!["entry.py"], None);
        step.exec_script_files = vec![
            ExecScriptFile {
                path: "entry.py".into(),
                sha256: String::new(),
            },
            ExecScriptFile {
                path: "helper.py".into(),
                sha256: String::new(),
            },
        ];
        crate::core::approved_scripts::validate_and_pin(home.path(), &mut step.exec_script_files)
            .unwrap();
        let mut ctx = TemplateContext::new();
        ctx.set_builtin("run.artifacts_dir", artifacts.as_path().to_string_lossy());
        let allow = vec!["python3".to_string()];
        let work_dir = worktree.path().to_string_lossy().to_string();
        let home_dir = home.path().to_string_lossy().to_string();

        let outcome = execute_exec_step_for_project(
            &step,
            &allow,
            &work_dir,
            &ctx,
            None,
            Some(&home_dir),
            MAX_OUTPUT_BYTES,
        )
        .await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "{}",
            outcome.result.output
        );
        assert!(
            outcome.result.output.contains("SENTINEL=APPROVED"),
            "{}",
            outcome.result.output
        );
        assert!(outcome.result.output.contains(&format!("WT={work_dir}")));

        // Without a carrying repository the declared scripts cannot be verified.
        let refused = execute_exec_step_for_project(
            &step,
            &allow,
            &work_dir,
            &ctx,
            None,
            None,
            MAX_OUTPUT_BYTES,
        )
        .await;
        assert_eq!(refused.result.status, RunStatus::Failed);
        assert!(refused.result.output.contains("no home project"));
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "exec_step_trust_tests.rs"]
mod exec_step_trust_tests;
