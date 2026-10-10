//! KT-1130 — a re-audit of an up-to-date repository ends Completed with its
//! documentary steps `unchanged`. Real Full pipeline, real templates, a scripted
//! `claude` (no socket): it fills each step's target on the first run and
//! touches nothing on the second.
use super::*;
use axum::response::IntoResponse;
use std::path::PathBuf;

struct TemplatesEnv(Option<std::ffi::OsString>);
impl Drop for TemplatesEnv {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_TEMPLATES_DIR"),
        }
    }
}

/// The scripted agent. Only step prompts (they carry the project's briefing)
/// count: the n-th one works on line n of `targets`, and `steps.log` records
/// whether that prompt was the fill or the re-verification one. In `fill` mode
/// it fills that target's placeholders, or appends a line when there is
/// nothing to fill; in `noop` mode it changes nothing. In `reverify` mode it
/// does only what the prompt names: it drops the claims whose citation the
/// documentary gate lists as stale, and it tries to rewrite a human-owned rule.
fn scripted_agent(tools: &Path, project: &Path) -> PathBuf {
    let state = tools.display();
    let project = project.display();
    let script = format!(
        r#"prompt=$(cat)
case "$prompt" in
  *{BRIEFING_MARKER}*)
    n=$(( $(cat '{state}/counter' 2>/dev/null || echo 0) + 1 ))
    echo "$n" > '{state}/counter'
    target=$(sed -n "${{n}}p" '{state}/targets')
    case "$prompt" in
      *"RE-VERIFICATION MODE"*) kind=reverify ;;
      *) kind=fill ;;
    esac
    echo "$n $target $kind" >> '{state}/steps.log'
    if [ "$(cat '{state}/mode')" = reverify ]; then
      printf '%s\n' "$prompt" | sed -n 's/^- `\([^`]*\)`: citation path does not exist `\([^`]*\)`$/\1 [src: file: \2:/p' > '{state}/drop'
      printf '%s\n' "$prompt" | sed -n 's/^- `\([^`]*\)`: citation line `\([^`]*\)` is outside `\([^`]*\)`$/\1 [src: file: \3:\2]/p' >> '{state}/drop'
      while read -r doc needle; do
        d='{project}'/"$doc"
        grep -vF "$needle" "$d" > "$d.next"
        mv "$d.next" "$d"
      done < '{state}/drop'
      f='{project}'/"$target"
      if [ -f "$f" ] && grep -q 'Human-owned rule' "$f"; then
        sed 's/Human-owned rule/Agent rewrote the human rule/' "$f" > "$f.next"
        mv "$f.next" "$f"
      fi
    fi
    if [ "$(cat '{state}/mode')" = fill ] && [ -n "$target" ] && [ "$target" != REVIEW ]; then
      f='{project}'/"$target"
      mkdir -p "$(dirname "$f")"
      touch "$f"
      sed -e 's/{{{{[A-Z0-9_]*_OUTCOME}}}}/scanned — nothing substantiable/g' \
          -e 's/{{{{[A-Z0-9_]*}}}}/verified in the fixture repository/g' "$f" > "$f.next"
      if cmp -s "$f" "$f.next"; then
        printf '\nChecked by the audit: scanned — nothing substantiable.\n' >> "$f.next"
      fi
      mv "$f.next" "$f"
    fi
    ;;
esac
printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Done."}}}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":1}}}}'"#
    );
    crate::acp::test_support::write_fixture_script(tools, &script)
}

/// Point the scripted agent at `targets` in `mode`, from its first step prompt.
fn arm(tools: &Path, mode: &str, targets: &[&str]) {
    std::fs::write(tools.join("mode"), mode).unwrap();
    std::fs::write(tools.join("targets"), targets.join("\n") + "\n").unwrap();
    std::fs::write(tools.join("counter"), "0").unwrap();
}

async fn run_full(state: &AppState, resume_run_id: Option<String>) -> String {
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::LaunchAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id,
        }),
    )
    .await
    .into_response();
    sse_body(response).await
}

/// The run row, its steps and its validation prompt.
async fn persisted(
    state: &AppState,
    run_id: &str,
) -> (
    crate::models::AuditRun,
    Vec<crate::models::AuditRunStep>,
    Option<String>,
) {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            let run = crate::db::audit_runs::get_by_id(conn, &run_id)?
                .ok_or_else(|| anyhow::anyhow!("run {run_id} not recorded"))?;
            let steps = crate::db::audit_runs::list_audit_steps(conn, &run_id)?;
            let prompt = match run.validation_discussion_id.as_deref() {
                Some(id) => crate::db::discussions::get_discussion(conn, id)?
                    .and_then(|d| d.messages.first().map(|m| m.content.clone())),
                None => None,
            };
            Ok((run, steps, prompt))
        })
        .await
        .unwrap()
}

/// The validation agent a run spawns runs detached and goes through the same
/// scripted agent: let it answer before the next run re-arms the script.
async fn validation_settled(state: &AppState, done: &Value) {
    let Some(id) = done["discussion_id"].as_str().map(str::to_string) else {
        return;
    };
    for _ in 0..600 {
        let id = id.clone();
        let answered = state
            .db
            .with_conn(move |conn| {
                Ok(crate::db::discussions::get_discussion(conn, &id)?
                    .is_some_and(|d| d.messages.len() >= 2))
            })
            .await
            .unwrap();
        if answered {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the validation agent of discussion {id} never answered");
}

fn outcomes(run: &crate::models::AuditRun) -> Value {
    serde_json::from_str(
        run.step_outcomes_json
            .as_deref()
            .expect("step outcomes recorded"),
    )
    .unwrap()
}

/// A git repository registered as the project, its `claude` routed to the
/// scripted agent, with the real templates.
struct Harness {
    tools: tempfile::TempDir,
    state: AppState,
    project: tempfile::TempDir,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    _restore: TemplatesEnv,
}

async fn harness() -> Harness {
    let restore = TemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
    crate::core::child_env::set_var(
        "KRONN_TEMPLATES_DIR",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates"),
    );
    let tools = tempfile::tempdir().unwrap();
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), false).await;
    let initialized = crate::core::cmd::git_cmd()
        .args(["init", "--quiet"])
        .current_dir(project.path())
        .output()
        .unwrap();
    assert!(initialized.status.success());
    std::fs::write(project.path().join("README.md"), "# Fixture\n").unwrap();
    let fixture = scripted_agent(tools.path(), project.path());
    let route = route_claude(project.path(), &fixture);
    Harness {
        tools,
        state,
        project,
        _route: route,
        _restore: restore,
    }
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_second_full_audit_of_an_up_to_date_repo_completes_with_unchanged_steps() {
    let h = harness().await;
    let (tools, state, project) = (&h.tools, &h.state, &h.project);

    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let targets: Vec<&str> = chain.iter().map(|s| s.target_file).collect();
    let total = chain.len() as u32;
    let review = targets
        .iter()
        .position(|t| *t == "REVIEW")
        .map(|i| i as u32 + 1);
    let documents: Vec<u32> = (1..=total).filter(|s| Some(*s) != review).collect();

    // First audit of a blank repository: every step writes its document.
    arm(tools.path(), "fill", &targets);
    let first = run_full(state, None).await;
    let done = sse_events(&first, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{first}");
    assert!(sse_events(&first, "step_unchanged").is_empty(), "{first}");
    assert!(sse_events(&first, "step_warning").is_empty(), "{first}");

    // Second audit with an agent that changes nothing: Completed, every
    // documentary step `unchanged`, none failed.
    arm(tools.path(), "noop", &targets);
    let second = run_full(state, None).await;
    let done = sse_events(&second, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{second}");
    assert_eq!(done["steps_to_redo"], json!([]), "{second}");
    assert!(sse_events(&second, "step_warning").is_empty(), "{second}");
    let unchanged_events: Vec<u64> = sse_events(&second, "step_unchanged")
        .iter()
        .map(|e| e["step"].as_u64().unwrap())
        .collect();
    assert_eq!(
        unchanged_events,
        documents.iter().map(|s| *s as u64).collect::<Vec<_>>()
    );
    for step_done in sse_events(&second, "step_done") {
        let expected = if Some(step_done["step"].as_u64().unwrap() as u32) == review {
            "succeeded"
        } else {
            "unchanged"
        };
        assert_eq!(step_done["success"], true, "{step_done}");
        assert_eq!(step_done["outcome"], expected, "{step_done}");
    }
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(state, &run_id).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, total);
    let recorded = outcomes(&run);
    assert_eq!(recorded["unchanged"], json!(documents));
    assert_eq!(
        recorded["succeeded"],
        json!(review.into_iter().collect::<Vec<_>>())
    );
    assert_eq!(recorded["failed"], json!([]));
    for step in &steps {
        assert!(step.cli_success, "{step:?}");
        assert_eq!(step.unchanged, Some(step.step_index) != review, "{step:?}");
        assert!(step.step_warning.is_none(), "{step:?}");
    }
    // The validation discussion lists the unchanged steps apart.
    let prompt = prompt.expect("a Completed run has its validation discussion");
    let section = prompt
        .split("## ÉTAPES INCHANGÉES")
        .nth(1)
        .unwrap_or_else(|| panic!("no unchanged section: {prompt}"));
    let section = section.split("\n## ").next().unwrap();
    for step in &documents {
        let file = crate::api::audit::full::step_file_label(&chain[*step as usize - 1]);
        assert!(
            section.contains(&format!("{step}/{total} — `{file}`")),
            "{section}"
        );
    }

    // A target that still holds a placeholder fails, never `unchanged`.
    let broken = targets
        .iter()
        .position(|t| *t == "docs/coding-rules.md")
        .unwrap() as u32
        + 1;
    let path = project.path().join("docs/coding-rules.md");
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("\n{{LEFTOVER}}\n");
    std::fs::write(&path, content).unwrap();
    arm(tools.path(), "noop", &targets);
    let third = run_full(state, None).await;
    let done = sse_events(&third, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "interrupted", "{third}");
    assert_eq!(done["steps_to_redo"], json!([broken]), "{third}");
    assert!(
        !sse_events(&third, "step_unchanged")
            .iter()
            .any(|e| e["step"] == broken),
        "{third}"
    );
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, _) = persisted(state, &interrupted).await;
    assert_eq!(run.status, "Interrupted");
    assert_eq!(outcomes(&run)["failed"], json!([broken]));

    // The resume replays only that step: the unchanged ones are skipped, and
    // the run they are carried into still knows them as unchanged.
    arm(tools.path(), "fill", &["docs/coding-rules.md"]);
    let fourth = run_full(state, Some(interrupted)).await;
    let done = sse_events(&fourth, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{fourth}");
    let started: Vec<Value> = sse_events(&fourth, "step_start");
    assert_eq!(started.len(), 1, "{fourth}");
    assert_eq!(started[0]["step"], broken);
    assert_eq!(sse_events(&fourth, "step_skipped").len() as u32, total - 1);
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(state, &run_id).await;
    assert_eq!(run.status, "Completed");
    let carried_unchanged: Vec<u32> = documents.iter().copied().filter(|s| *s != broken).collect();
    assert_eq!(outcomes(&run)["unchanged"], json!(carried_unchanged));
    assert_eq!(
        crate::api::audit::full::already_unchanged_step_indices(&steps)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        carried_unchanged.iter().copied().collect()
    );
    assert!(prompt.unwrap().contains("## ÉTAPES INCHANGÉES"));
}

/// The step prompts the scripted agent received since `arm`, in order: their
/// target and whether they were the re-verification prompt.
fn prompt_kinds(tools: &Path) -> Vec<(String, bool)> {
    std::fs::read_to_string(tools.join("steps.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split(' ');
            let _n = parts.next()?;
            let target = parts.next()?.to_string();
            Some((target, parts.next()? == "reverify"))
        })
        .collect()
}

/// KT-1132 — a re-audit re-checks a filled document against the current code:
/// a claim whose citation no longer resolves is corrected in the same run,
/// a true document stays byte-identical, a human-owned section stays intact,
/// a dead citation inside a human section becomes a to-do for its owner
/// instead of a blocker, and the operator's `.gitignore` is never written.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_reaudit_corrects_a_stale_citation_and_leaves_true_docs_unchanged() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    let gitignore = b"target/\n# operator rules\n*.log";
    std::fs::write(project.join(".gitignore"), gitignore).unwrap();
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/kept.rs"), "fn main() {}\n".repeat(40)).unwrap();
    std::fs::write(
        project.join("src/gone.rs"),
        "fn gone() {}\nfn errors() {}\n",
    )
    .unwrap();
    std::fs::write(project.join("src/retired.rs"), "// billing\n".repeat(5)).unwrap();

    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let targets: Vec<&str> = chain.iter().map(|s| s.target_file).collect();
    let rules_step = targets
        .iter()
        .position(|t| *t == "docs/coding-rules.md")
        .unwrap() as u32
        + 1;

    arm(tools, "fill", &targets);
    let first = run_full(state, None).await;
    let done = sse_events(&first, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{first}");
    assert!(
        prompt_kinds(tools).iter().all(|(_, reverify)| !reverify),
        "a first audit fills templates: {:?}",
        prompt_kinds(tools)
    );

    // The first audit documented code that has changed since: one file is
    // gone, the other shrank. The human-owned rule belongs to a person.
    let human = "<!-- kronn:section name=\"team-rule\" owner=\"human\" -->\n\
                 ## Team rule\n\nHuman-owned rule: reviews happen on Tuesday.\n\
                 <!-- kronn:section:end -->\n";
    let rules = project.join("docs/coding-rules.md");
    let mut content = std::fs::read_to_string(&rules).unwrap();
    content.push_str("\n## Verified facts\n\n");
    content.push_str("- The entry point is `main` [src: file: src/kept.rs:1]\n");
    content.push_str("- Errors are mapped in one place [src: file: src/gone.rs:2]\n");
    content.push_str("- The last handler sits at the end [src: file: src/kept.rs:40]\n\n");
    content.push_str(human);
    std::fs::write(&rules, &content).unwrap();
    // A human wrote a section of the glossary that cites a file since removed.
    let glossary = project.join("docs/glossary.md");
    let mut content = std::fs::read_to_string(&glossary).unwrap();
    content.push_str(
        "\n<!-- kronn:section name=\"vocabulary-owner\" owner=\"human\" -->\n\
         ## Owner notes\n\nThe billing term comes from the old module [src: file: src/retired.rs:3].\n\
         <!-- kronn:section:end -->\n",
    );
    std::fs::write(&glossary, &content).unwrap();
    std::fs::remove_file(project.join("src/gone.rs")).unwrap();
    std::fs::remove_file(project.join("src/retired.rs")).unwrap();
    std::fs::write(project.join("src/kept.rs"), "fn main() {}\n".repeat(10)).unwrap();
    let documents_before: Vec<(String, Vec<u8>)> = targets
        .iter()
        .filter(|t| **t != "REVIEW" && **t != "docs/coding-rules.md")
        .map(|t| (t.to_string(), std::fs::read(project.join(t)).unwrap()))
        .collect();

    std::fs::remove_file(tools.join("steps.log")).unwrap();
    arm(tools, "reverify", &targets);
    let second = run_full(state, None).await;
    let done = sse_events(&second, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{second}");
    assert_eq!(done["steps_to_redo"], json!([]), "{second}");

    // Steps 1 to 7 got the re-verification prompt, the others did not.
    let kinds = prompt_kinds(tools);
    for (index, (target, reverify)) in kinds.iter().enumerate() {
        assert_eq!(
            *reverify,
            index < crate::api::audit::REVERIFIED_STEPS,
            "step {} ({target})",
            index + 1
        );
    }

    // The stale claims are gone, the true one and the human rule stay.
    let corrected = std::fs::read_to_string(&rules).unwrap();
    assert!(!corrected.contains("src/gone.rs"), "{corrected}");
    assert!(!corrected.contains("src/kept.rs:40"), "{corrected}");
    assert!(corrected.contains("- The entry point is `main` [src: file: src/kept.rs:1]\n"));
    assert!(corrected.contains(human), "{corrected}");
    assert!(!corrected.contains("Agent rewrote"), "{corrected}");
    let rules_done = sse_events(&second, "step_done")
        .into_iter()
        .find(|e| e["step"] == rules_step)
        .unwrap();
    assert_eq!(rules_done["outcome"], "succeeded", "{rules_done}");
    let optimization = sse_events(&second, "documentary_optimization")
        .pop()
        .unwrap();
    assert_eq!(optimization["blocking_count"], 0, "{optimization}");

    // The human section's dead citation: never handed to an agent, never a
    // blocker, reported for its owner and listed in the validation discussion.
    assert!(
        !sse_events(&second, "step_retry")
            .iter()
            .any(|e| e.to_string().contains("retired.rs")),
        "{second}"
    );
    let glossary_step = targets
        .iter()
        .position(|t| *t == "docs/glossary.md")
        .unwrap() as u64
        + 1;
    let glossary_done = sse_events(&second, "step_done")
        .into_iter()
        .find(|e| e["step"] == glossary_step)
        .unwrap();
    assert_eq!(glossary_done["outcome"], "unchanged", "{glossary_done}");
    let message = "human-owned section `vocabulary-owner`: citation path does not exist \
                   `src/retired.rs`; only its owner can fix it";
    let reported = optimization["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "human_section_reference")
        .unwrap_or_else(|| panic!("{optimization}"));
    assert_eq!(reported["blocking"], false, "{reported}");
    assert_eq!(reported["path"], "docs/glossary.md", "{reported}");
    assert_eq!(reported["message"], message, "{reported}");
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (_, _, prompt) = persisted(state, &run_id).await;
    let prompt = prompt.expect("a Completed run has its validation discussion");
    let section = prompt
        .split("## RÉFÉRENCES MORTES DANS DES SECTIONS HUMAINES")
        .nth(1)
        .unwrap_or_else(|| panic!("no human references section: {prompt}"));
    assert!(
        section.contains(&format!("- `docs/glossary.md` — {message}")),
        "{section}"
    );

    // Every other document was true: untouched, its step `unchanged`.
    for (target, before) in &documents_before {
        assert_eq!(
            &std::fs::read(project.join(target)).unwrap(),
            before,
            "{target}"
        );
    }
    let unchanged: Vec<u64> = sse_events(&second, "step_unchanged")
        .iter()
        .map(|e| e["step"].as_u64().unwrap())
        .collect();
    assert!(!unchanged.contains(&(rules_step as u64)), "{second}");
    assert_eq!(unchanged.len(), documents_before.len(), "{second}");

    // Two Full audits never wrote the operator's `.gitignore`; Kronn's own
    // path is excluded locally instead.
    assert_eq!(
        std::fs::read(project.join(".gitignore")).unwrap(),
        gitignore
    );
    let exclude = std::fs::read_to_string(project.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.lines().any(|l| l.trim() == "docs/var/"),
        "{exclude}"
    );
}

/// A Full audit of a repository without a `.gitignore` does not create one.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_full_audit_never_creates_a_gitignore() {
    let h = harness().await;
    let (tools, state, project) = (h.tools.path(), &h.state, h.project.path());
    let chain = crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full);
    let targets: Vec<&str> = chain.iter().map(|s| s.target_file).collect();
    arm(tools, "fill", &targets);
    let body = run_full(state, None).await;
    let done = sse_events(&body, "done").pop().expect("terminal done");
    validation_settled(state, &done).await;
    assert_eq!(done["status"], "complete", "{body}");
    assert!(!project.join(".gitignore").exists());
    assert!(std::fs::symlink_metadata(project.join(".gitignore")).is_err());
}
