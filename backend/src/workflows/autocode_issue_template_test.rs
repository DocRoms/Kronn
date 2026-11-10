use super::*;
use crate::core::contribution::{
    evaluate, Facts, GhAuth, PreflightReport, RepoProjectFact, DEFAULT_REPOSITORY,
};
use crate::models::{AiAuditStatus, AiConfigStatus, Project, Workflow};
use crate::workflows::template::TemplateContext;
use axum::Json;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn template(project_id: Option<&str>) -> AutocodeIssueTemplate {
    build_autocode_issue(&AutocodeIssueParams {
        project_id: project_id.map(String::from),
    })
}

fn all_steps(template: &AutocodeIssueTemplate) -> Vec<&WorkflowStep> {
    template
        .contribute
        .steps
        .iter()
        .chain(template.publish.steps.iter())
        .collect()
}

/// Everything a step can carry that a run reads: its program, its arguments and
/// the stdin it is fed.
fn step_text(step: &WorkflowStep) -> String {
    format!(
        "{}\n{}\n{}",
        step.exec_command.clone().unwrap_or_default(),
        step.exec_args.join("\n"),
        step.exec_stdin.clone().unwrap_or_default()
    )
}

fn looks_like_a_uuid(text: &str) -> bool {
    regex_lite::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .unwrap()
        .is_match(text)
}

// ─── DoD 1: no hard-coded value ─────────────────────────────────────────────

#[test]
fn the_rendered_steps_hold_no_instance_specific_value() {
    let template = template(Some("p-other-9"));
    for step in all_steps(&template) {
        let text = step_text(step);
        for forbidden in [
            "localhost",
            "127.0.0.1",
            "3140",
            "p-other-9",
            "a0f959a8",
            "ead3191a",
            "__DEFAULT_",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} carries `{forbidden}`",
                step.name
            );
        }
        assert!(!looks_like_a_uuid(&text), "{} carries an id", step.name);
    }
}

#[test]
fn every_call_back_to_kronn_uses_the_runtime_url_and_the_project_comes_from_the_preflight() {
    let template = template(None);
    // `take` talks to GitHub only; every other step calls Kronn back.
    for step in all_steps(&template)
        .into_iter()
        .filter(|s| s.name != "take")
    {
        let stdin = step.exec_stdin.clone().unwrap();
        assert!(stdin.contains("{{run.kronn_url}}"), "{}", step.name);
    }
    let orchestrate = &template.contribute.steps[2];
    let code = &orchestrate.exec_args[1];
    assert!(code.contains("report.get('project_id')"));
    assert!(
        orchestrate
            .exec_stdin
            .as_ref()
            .unwrap()
            .contains("{{steps.preflight.data_json}}"),
        "the project is the one the preflight resolved from the repository"
    );
}

#[test]
fn the_repository_the_target_branch_and_the_review_rounds_are_workflow_variables() {
    let template = template(None);
    for workflow in [&template.contribute, &template.publish] {
        let by_name = |name: &str| {
            workflow
                .variables
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("variable {name}"))
        };
        assert!(by_name("issueNumber").required);
        // The defaults are what the board shows in the empty field.
        assert_eq!(by_name("repo").placeholder, DEFAULT_REPOSITORY);
        assert_eq!(by_name("targetBranch").placeholder, "main");
        assert_eq!(by_name("maxReviewRounds").placeholder, "10");
        for name in ["repo", "targetBranch", "maxReviewRounds"] {
            assert!(!by_name(name).required, "{name} has a default");
            assert!(by_name(name).description.is_some(), "{name} is explained");
        }
    }
    // The scripts apply the same defaults when a variable is left empty.
    let code = &template.contribute.steps[0].exec_args[1];
    assert!(code.contains("repo or 'DocRoms/Kronn'"), "{code}");
    assert!(code.contains("branch or 'main'"));
    assert!(code.contains("rounds or '10'"));
}

#[test]
fn the_reviewer_is_an_optional_workflow_variable_the_preflight_receives() {
    let template = template(None);
    for workflow in [&template.contribute, &template.publish] {
        let reviewer = workflow
            .variables
            .iter()
            .find(|v| v.name == "reviewer")
            .expect("the reviewer variable");
        assert!(!reviewer.required);
        let description = reviewer.description.clone().unwrap();
        assert!(description.contains("litellm:<model>") && description.contains("`human`"));
        let preflight = &workflow.steps[0];
        assert!(preflight
            .exec_stdin
            .as_ref()
            .unwrap()
            .ends_with("{{reviewer}}"));
        assert!(preflight.exec_args[1].contains("'reviewer': reviewer"));
    }
    // The campaign is created with the reviewer the preflight announced.
    let orchestrate = &template.contribute.steps[2];
    assert!(orchestrate.exec_args[1].contains("report['suggested_reviewer']"));
}

// ─── DoD 3: the first step is the preflight ─────────────────────────────────

#[test]
fn the_first_step_of_each_workflow_is_the_preflight() {
    let template = template(None);
    for workflow in [&template.contribute, &template.publish] {
        let first = &workflow.steps[0];
        assert_eq!(first.name, "preflight");
        assert!(first.exec_args[1].contains("/api/contribution/preflight"));
    }
    assert_eq!(template.contribute.steps[1].name, "take");
    assert_eq!(template.contribute.steps[2].name, "orchestrate");
    assert_eq!(template.publish.steps[1].name, "publish");
}

#[test]
fn the_steps_render_with_the_values_of_a_run() {
    let template = template(None);
    let mut ctx = TemplateContext::new();
    ctx.set_builtin("run.kronn_url", "http://127.0.0.1:23140");
    ctx.set("issueNumber", "7");
    ctx.set("repo", "acme/widgets");
    ctx.set("targetBranch", "develop");
    ctx.set("maxReviewRounds", "4");
    ctx.set("reviewer", "nvidia:meta/llama-3");
    let preflight = ctx
        .render_strict(template.contribute.steps[0].exec_stdin.as_ref().unwrap())
        .unwrap();
    assert_eq!(
        preflight.split(SEP).collect::<Vec<_>>(),
        vec![
            "http://127.0.0.1:23140",
            "acme/widgets",
            "develop",
            "4",
            "7",
            "nvidia:meta/llama-3"
        ]
    );
    let take = ctx
        .render_strict(template.contribute.steps[1].exec_stdin.as_ref().unwrap())
        .unwrap();
    assert_eq!(take, format!("7{SEP}acme/widgets"));
}

// ─── python3 is a prerequisite ──────────────────────────────────────────────

/// A directory holding an executable `python3` (and nothing else): the PATH of
/// a machine that has Python 3, whatever the host running the test has.
#[cfg(unix)] // a #!/bin/sh python3 made executable by mode bits
struct FakePython {
    dir: tempfile::TempDir,
}

#[cfg(unix)]
impl FakePython {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python3");
        std::fs::write(&python, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn path(&self) -> &std::ffi::OsStr {
        self.dir.path().as_os_str()
    }
}

#[test]
fn every_step_runs_python3_and_the_allowlist_names_it() {
    let template = template(None);
    for step in all_steps(&template) {
        assert_eq!(
            step.exec_command.as_deref(),
            Some("python3"),
            "{}",
            step.name
        );
    }
    for workflow in [&template.contribute, &template.publish] {
        assert!(workflow.exec_allowlist.iter().any(|p| p == "python3"));
    }
}

#[tokio::test]
async fn the_install_is_refused_with_the_fix_when_python3_is_not_on_the_path() {
    let state = fresh_state();
    // A machine whose PATH holds no python3 (a `python` alone is not enough).
    let no_python = tempfile::tempdir().unwrap();
    std::fs::write(no_python.path().join("python"), "#!/bin/sh\n").unwrap();
    for path in [Some(no_python.path().as_os_str()), None] {
        let Json(refused) = crate::api::workflows::install_autocode_issue(
            state.clone(),
            AutocodeIssueParams { project_id: None },
            path,
        )
        .await;
        assert!(!refused.success);
        assert_eq!(refused.error_code.as_deref(), Some("validation"));
        let message = refused.error.unwrap();
        assert_eq!(message, crate::core::contribution::python_missing_message());
        // It names what to install, and what Windows offers instead.
        for needle in ["python3", "https://www.python.org", "Microsoft Store"] {
            assert!(message.contains(needle), "{message}");
        }
    }
    let stored = state
        .db
        .with_conn(crate::db::workflows::list_workflows)
        .await
        .unwrap();
    assert!(stored.is_empty(), "nothing was installed");
}

#[test]
fn a_run_whose_python3_disappeared_says_what_to_install() {
    // A real spawn of `python3` with no python3 on the child's PATH.
    let empty = tempfile::tempdir().unwrap();
    let error = Command::new("python3")
        .env("PATH", empty.path())
        .spawn()
        .expect_err("no python3 on that PATH");
    let step = &template(None).contribute.steps[0];
    let message = crate::workflows::exec_step::spawn_failure(&step.name, "python3", &error);
    assert_eq!(
        message,
        format!(
            "Exec step `preflight`: {}",
            crate::core::contribution::python_missing_message()
        )
    );
    assert!(message.contains("Microsoft Store") && !message.contains("failed to spawn"));
    // Any other program keeps the raw error.
    let other = crate::workflows::exec_step::spawn_failure("s", "make", &error);
    assert!(other.contains("failed to spawn `make`"), "{other}");
}

// ─── Installing on another instance ─────────────────────────────────────────

fn project(id: &str) -> Project {
    Project {
        id: id.into(),
        name: "widgets".into(),
        path: "/work/widgets".into(),
        repo_url: Some("https://github.com/acme/widgets".into()),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        write_access: None,
        mcp_sync_report: None,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        workspace: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn fresh_state() -> crate::AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

#[tokio::test]
#[cfg(unix)] // installs over a fake python3 that is a #!/bin/sh script
async fn the_template_installs_on_a_fresh_data_dir_with_another_project_id() {
    let state = fresh_state();
    // Not the id of the instance the workflow was written on.
    let local = project("0f8e6a1c-5b7d-4c11-9a52-3d2f6e8b7a90");
    let local_id = local.id.clone();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &local))
        .await
        .unwrap();

    let python = FakePython::new();
    let Json(installed) = crate::api::workflows::install_autocode_issue(
        state.clone(),
        AutocodeIssueParams {
            project_id: Some(local_id.clone()),
        },
        Some(python.path()),
    )
    .await;
    assert!(installed.success, "{:?}", installed.error);
    let installed = installed.data.unwrap();
    assert_eq!(installed.contribute.name, CONTRIBUTE_NAME);
    assert_eq!(installed.publish.name, PUBLISH_NAME);
    assert_eq!(installed.contribute.project_id.as_deref(), Some(&*local_id));

    // What was stored (not only what was built) has no instance value.
    let stored: Vec<Workflow> = state
        .db
        .with_conn(crate::db::workflows::list_workflows)
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    for workflow in &stored {
        assert_eq!(workflow.steps[0].name, "preflight");
        for step in &workflow.steps {
            let text = step_text(step);
            assert!(!text.contains(&local_id), "{}", step.name);
            assert!(!text.contains("127.0.0.1") && !looks_like_a_uuid(&text));
            // Static program text: no inline script takes a template value.
            assert!(
                crate::core::inline_code::classify_step(step, false).is_empty(),
                "{} is an unsafe line",
                step.name
            );
        }
    }
}

#[tokio::test]
#[cfg(unix)] // installs over a fake python3 that is a #!/bin/sh script
async fn installing_twice_is_refused_instead_of_duplicating_the_workflows() {
    let state = fresh_state();
    let python = FakePython::new();
    let params = || AutocodeIssueParams { project_id: None };
    let Json(first) =
        crate::api::workflows::install_autocode_issue(state.clone(), params(), Some(python.path()))
            .await;
    assert!(first.success, "{:?}", first.error);
    let Json(second) =
        crate::api::workflows::install_autocode_issue(state.clone(), params(), Some(python.path()))
            .await;
    assert!(!second.success);
    assert!(second.error.unwrap().contains("already installed"));
    let count = state
        .db
        .with_conn(crate::db::workflows::list_workflows)
        .await
        .unwrap()
        .len();
    assert_eq!(count, 2);
}

// ─── Dry runs: the scripts against a stub Kronn, a stub gh and local remotes ─

/// A Kronn that answers every request with `body`; returns its address.
fn stub_kronn(body: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    address
}

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn run_script(step: &WorkflowStep, stdin: &str, env: &[(&str, String)]) -> Output {
    let mut command = Command::new("python3");
    command
        .args(&step.exec_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn facts(can_push: bool) -> Facts {
    Facts {
        gh_installed: true,
        gh_auth: Some(GhAuth {
            login: Some("octo".into()),
            scopes: Some(vec!["repo".into()]),
        }),
        can_push: Some(can_push),
        fork_exists: false,
        project: Some(RepoProjectFact::Linked {
            id: "p1".into(),
            name: "widgets".into(),
            path_exists: true,
        }),
        local_workers: vec![crate::models::AgentType::ClaudeCode],
    }
}

fn envelope<T: serde::Serialize>(data: &T) -> String {
    serde_json::json!({ "success": true, "data": data }).to_string()
}

#[test]
fn the_preflight_step_fails_with_the_actionable_message_and_passes_when_ready() {
    if !python_available() {
        eprintln!("python3 missing: dry run skipped");
        return;
    }
    let template = template(None);
    let step = &template.contribute.steps[0];

    let mut not_ready = facts(true);
    not_ready.gh_auth = None;
    let report = evaluate("acme/widgets", "main", 10, "autocode/issue-7", &not_ready);
    let kronn = stub_kronn(envelope(&report));
    let input = format!("{kronn}{SEP}acme/widgets{SEP}{SEP}{SEP}7");
    let out = run_script(step, &input, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[FAIL] gh_auth") && stderr.contains("gh auth login"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty(), "nothing flows on to the next step");

    let ready = evaluate("acme/widgets", "main", 10, "autocode/issue-7", &facts(true));
    let kronn = stub_kronn(envelope(&ready));
    let out = run_script(
        step,
        &format!("{kronn}{SEP}acme/widgets{SEP}{SEP}{SEP}7"),
        &[],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: PreflightReport = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report.ok);

    let bad_issue = run_script(
        step,
        &format!("{kronn}{SEP}acme/widgets{SEP}{SEP}{SEP}x"),
        &[],
    );
    assert!(String::from_utf8_lossy(&bad_issue.stderr).contains("Invalid issue number"));
}

#[cfg(unix)] // only the gh dry runs use it
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t.test"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `<root>/<owner>/<name>.git`, a bare repository whose path reads like the
/// slug of a remote.
#[cfg(unix)] // only the gh dry runs use it
fn bare(root: &Path, slug: &str) -> std::path::PathBuf {
    let path = root.join(format!("{slug}.git"));
    std::fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "--bare", "--initial-branch=main"]);
    path
}

#[cfg(unix)] // only the gh dry runs use it
struct Dry {
    _root: tempfile::TempDir,
    checkout: std::path::PathBuf,
    upstream: std::path::PathBuf,
    fork: std::path::PathBuf,
    gh_log: std::path::PathBuf,
    path_env: String,
}

/// A checkout whose `origin` is the repository (`acme/widgets`) and whose
/// `fork` remote is `octo/widgets`, holding `autocode/issue-7` one commit
/// ahead of `main`; a `gh` that logs what it is asked and answers the calls the
/// publish step makes.
#[cfg(unix)] // only the gh dry runs use it
fn dry_run_setup() -> Dry {
    let root = tempfile::tempdir().unwrap();
    let upstream = bare(root.path(), "acme/widgets");
    let fork = bare(root.path(), "octo/widgets");
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    git(&checkout, &["init", "--initial-branch=main"]);
    git(&checkout, &["commit", "--allow-empty", "-m", "base"]);
    git(
        &checkout,
        &["remote", "add", "origin", upstream.to_str().unwrap()],
    );
    git(
        &checkout,
        &["remote", "add", "fork", fork.to_str().unwrap()],
    );
    git(&checkout, &["push", "origin", "main"]);
    git(&checkout, &["checkout", "-b", "autocode/issue-7"]);
    git(&checkout, &["commit", "--allow-empty", "-m", "the change"]);

    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\n\
         echo \"$@\" >> \"$GH_LOG\"\n\
         case \"$1 $2\" in\n\
           \"api repos/acme/widgets/issues/7\") echo 'Fix the thing' ;;\n\
           \"pr create\") echo 'https://github.com/acme/widgets/pull/9' ;;\n\
         esac\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path_env = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Dry {
        gh_log: root.path().join("gh.log"),
        _root: root,
        checkout,
        upstream,
        fork,
        path_env,
    }
}

#[cfg(unix)] // only the gh dry runs use it
fn branch_exists(bare: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(bare)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .unwrap()
        .success()
}

#[cfg(unix)] // only the gh dry runs use it
fn publish(dry: &Dry, can_push: bool, fork_exists: bool) -> (Output, String) {
    let mut facts = facts(can_push);
    facts.fork_exists = fork_exists;
    let report = evaluate("acme/widgets", "main", 10, "autocode/issue-7", &facts);
    let kronn = stub_kronn(envelope(&serde_json::json!({ "path": dry.checkout })));
    let template = template(None);
    let input = format!(
        "{}{SEP}{kronn}{SEP}acme/widgets{SEP}{SEP}7",
        serde_json::to_string(&report).unwrap()
    );
    let out = run_script(
        &template.publish.steps[1],
        &input,
        &[
            ("PATH", dry.path_env.clone()),
            ("GH_LOG", dry.gh_log.display().to_string()),
        ],
    );
    let log = std::fs::read_to_string(&dry.gh_log).unwrap_or_default();
    (out, log)
}

#[test]
#[cfg(unix)] // stubs gh with a #!/bin/sh script made executable by mode bits
fn a_maintainer_pushes_the_branch_to_the_repository_and_opens_the_pull_request() {
    if !python_available() {
        eprintln!("python3 missing: dry run skipped");
        return;
    }
    let dry = dry_run_setup();
    let (out, log) = publish(&dry, true, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(branch_exists(&dry.upstream, "autocode/issue-7"));
    assert!(!branch_exists(&dry.fork, "autocode/issue-7"));
    assert!(!log.contains("repo fork"), "{log}");
    assert!(
        log.contains(
            "pr create --repo acme/widgets --base main --head autocode/issue-7 --title Issue #7 - Fix the thing"
        ),
        "{log}"
    );
    let done: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(done["mode"], "maintainer");
    assert_eq!(
        done["pull_request"],
        "https://github.com/acme/widgets/pull/9"
    );
}

#[test]
#[cfg(unix)] // stubs gh with a #!/bin/sh script made executable by mode bits
fn a_contributor_forks_pushes_to_the_fork_and_opens_the_pull_request_against_the_repository() {
    if !python_available() {
        eprintln!("python3 missing: dry run skipped");
        return;
    }
    let dry = dry_run_setup();
    let (out, log) = publish(&dry, false, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(branch_exists(&dry.fork, "autocode/issue-7"));
    assert!(
        !branch_exists(&dry.upstream, "autocode/issue-7"),
        "a contributor never pushes to the repository"
    );
    let fork_at = log.find("repo fork acme/widgets").expect(&log);
    let pr_at = log
        .find("pr create --repo acme/widgets --base main --head octo:autocode/issue-7")
        .expect(&log);
    assert!(
        fork_at < pr_at,
        "the fork exists before the pull request: {log}"
    );
    let done: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(done["mode"], "fork");

    // An existing fork is reused: no second `gh repo fork`.
    let dry = dry_run_setup();
    let (out, log) = publish(&dry, false, true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!log.contains("repo fork"), "{log}");
    assert!(branch_exists(&dry.fork, "autocode/issue-7"));
}

#[test]
#[cfg(unix)] // stubs gh with a #!/bin/sh script made executable by mode bits
fn nothing_is_published_before_the_campaign_delivered() {
    if !python_available() {
        eprintln!("python3 missing: dry run skipped");
        return;
    }
    let dry = dry_run_setup();
    git(&dry.checkout, &["reset", "--hard", "main"]);
    let (out, log) = publish(&dry, true, false);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("holds no commit beyond"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!log.contains("pr create"), "{log}");
}
