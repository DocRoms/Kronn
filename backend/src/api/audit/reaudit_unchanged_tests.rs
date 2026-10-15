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
/// A step whose number is listed in `fail` ends in a CLI error, its target
/// untouched (KT-1131). Any other prompt — the validation — ends on the
/// terminal signal, after settling the documents' leftover unknowns when
/// `settle` exists. A step prompt waits while `hold` exists.
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
    printf '%s' "$prompt" > '{state}/prompt.'"$n"
    if [ -f '{state}/undo' ]; then sh '{state}/undo'; rm -f '{state}/undo'; fi
    while [ -f '{state}/hold' ]; do sleep 0.1; done
    if grep -qx "$n" '{state}/fail' 2>/dev/null; then
      printf '%s\n' '{{"type":"result","subtype":"error_during_execution","is_error":true,"usage":{{"input_tokens":1,"output_tokens":1}}}}'
      exit 1
    fi
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
    if [ "$(cat '{state}/mode')" = revalidate ]; then
      f='{project}'/"$target"
      printf '%s\n' "$prompt" | sed -n 's/^- `\[src: file: \([^]]*\)\]` .*changed.*$/\1/p' > '{state}/fix'
      while read -r cite; do
        awk -v c="[src: file: $cite]" '{{ if (index($0, c)) print "- Revalidated against the current code " c; else print }}' "$f" > "$f.next"
        mv "$f.next" "$f"
      done < '{state}/fix'
      if grep -q 'Human-owned rule' "$f"; then
        sed 's/Human-owned rule/Agent rewrote the human rule/' "$f" > "$f.next"
        mv "$f.next" "$f"
      fi
    fi
    if [ "$(cat '{state}/mode')" = fill ] && [ -n "$target" ] && [ "$target" != REVIEW ]; then
      f='{project}'/"$target"
      mkdir -p "$(dirname "$f")"
      touch "$f"
      sed -e 's/{{{{[A-Z0-9_]*_OUTCOME}}}}/scanned — nothing substantiable/g' \
          -e 's/{{{{DEP_EVIDENCE}}}}/not verified: no source/g' \
          -e 's/{{{{[A-Z0-9_]*}}}}/verified in the fixture repository/g' "$f" > "$f.next"
      if cmp -s "$f" "$f.next"; then
        printf '\nChecked by the audit: scanned — nothing substantiable.\n' >> "$f.next"
      fi
      mv "$f.next" "$f"
    fi
    ;;
  *)
    while [ -f '{state}/validation-hold' ]; do sleep 0.1; done
    if [ -f '{state}/settle' ]; then
      find '{project}/docs' '{project}/.ai' '{project}/doc' -name '*.md' -not -path '*/tech-debt/*' 2>/dev/null | while read -r f; do
        sed -e 's/{{{{[A-Z0-9_]*}}}}/settled by the validation/g' -e 's/<!-- TODO:[^>]*-->//g' "$f" > "$f.next"
        mv "$f.next" "$f"
      done
    fi
    printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"KRONN:VALIDATION_COMPLETE"}}}}}}'
    printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":1,"output_tokens":1}}}}'
    exit 0
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
    std::fs::write(tools.join("fail"), "").unwrap();
}

/// Let the validation agent settle the unknowns a human validation would.
fn settle_on_validation(tools: &Path) {
    std::fs::write(tools.join("settle"), "").unwrap();
}

/// Make the listed step prompts (their order in this run) end in a CLI error.
fn fail_steps(tools: &Path, prompts: &[u32]) {
    let lines: Vec<String> = prompts.iter().map(u32::to_string).collect();
    std::fs::write(tools.join("fail"), lines.join("\n") + "\n").unwrap();
}

async fn run_full(state: &AppState, resume_run_id: Option<String>) -> String {
    run_full_retrying(state, resume_run_id, None).await
}

async fn run_full_retrying(
    state: &AppState,
    resume_run_id: Option<String>,
    retry_steps: Option<Vec<u32>>,
) -> String {
    launch_full(state, resume_run_id, retry_steps, None).await
}

async fn launch_full(
    state: &AppState,
    resume_run_id: Option<String>,
    retry_steps: Option<Vec<u32>>,
    fresh: Option<bool>,
) -> String {
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
            retry_steps,
            redo_steps: None,
            fresh,
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
/// scripted agent: wait for its reply and dispatch cleanup before the next run
/// validates the docs or re-arms the script.
async fn validation_settled(state: &AppState, done: &Value) {
    let Some(id) = done["discussion_id"].as_str().map(str::to_string) else {
        return;
    };
    for _ in 0..600 {
        let id = id.clone();
        let cancel_registry = state.cancel_registry.clone();
        let answered = state
            .db
            .with_conn(move |conn| {
                let discussion = crate::db::discussions::get_discussion(conn, &id)?;
                Ok(
                    discussion.is_some_and(|d| d.messages.len() >= 2 && !d.awaiting_agent)
                        && !crate::db::agent_dispatch::has_active_for_discussion(conn, &id)?
                        && !cancel_registry
                            .lock()
                            .map_err(|_| anyhow::anyhow!("cancel registry poisoned"))?
                            .contains_key(&id),
                )
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
/// a true document stays byte-identical, a rewrite of a human-owned section
/// is kept and marked to reconfirm (KT-1079), a dead citation inside a human
/// section becomes a to-do for its owner
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
        prompt_kinds(tools)
            .iter()
            .take(crate::api::audit::REVERIFIED_STEPS)
            .all(|(_, reverify)| !reverify),
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

    // Every step preserves statements that remain true.
    let kinds = prompt_kinds(tools);
    for (index, (target, reverify)) in kinds.iter().enumerate() {
        assert!(*reverify, "step {} ({target})", index + 1);
    }

    // The stale claims are gone, the true one stays; the agent's rewrite of
    // the human rule is kept, and the doc waits for its owner to reconfirm.
    let corrected = std::fs::read_to_string(&rules).unwrap();
    assert!(!corrected.contains("src/gone.rs"), "{corrected}");
    assert!(!corrected.contains("src/kept.rs:40"), "{corrected}");
    assert!(corrected.contains("- The entry point is `main` [src: file: src/kept.rs:1]\n"));
    assert!(
        corrected.contains("Agent rewrote the human rule"),
        "{corrected}"
    );
    let provenance = crate::core::doc_provenance::report(project, true).unwrap();
    let marked = provenance
        .docs
        .iter()
        .find(|d| d.doc == "docs/coding-rules.md")
        .unwrap_or_else(|| panic!("{provenance:?}"));
    assert_eq!(
        marked.state,
        crate::core::doc_provenance::ProvenanceState::ToReconfirm
    );
    assert!(marked.changes[0]
        .before
        .as_deref()
        .unwrap()
        .contains("Human-owned rule"));
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
                   `src/retired.rs`; reported for its owner, never blocking";
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
    // KT-1187 — only the corrected doc has its citations recorded afresh; the
    // glossary a human edited keeps its record.
    let citations = crate::core::citation_drift::check(project);
    assert!(
        !citations
            .section("docs/coding-rules.md")
            .unwrap()
            .doc_changed
    );
    assert!(citations.section("docs/glossary.md").unwrap().doc_changed);

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

// ── KT-1131 — a failed step retried alone, or accepted by a human ────────────

const ACCEPT_ROUTE: &str = "/api/projects/{id}/audit-runs/{run_id}/steps/{step}/accept";

/// A git repository the Full audit has documented once, every step green.
struct Audited {
    tools: tempfile::TempDir,
    project: tempfile::TempDir,
    state: AppState,
    targets: Vec<&'static str>,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    _restore: TemplatesEnv,
}

impl Audited {
    async fn new() -> Self {
        let Harness {
            tools,
            state,
            project,
            _route: route,
            _restore: restore,
        } = harness().await;
        let targets: Vec<&'static str> =
            crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
                .iter()
                .map(|s| s.target_file)
                .collect();
        arm(tools.path(), "fill", &targets);
        let first = run_full(&state, None).await;
        let done = sse_events(&first, "done").pop().expect("terminal done");
        validation_settled(&state, &done).await;
        assert_eq!(done["status"], "complete", "{first}");
        Self {
            tools,
            project,
            state,
            targets,
            _route: route,
            _restore: restore,
        }
    }

    fn step_of(&self, target: &str) -> u32 {
        self.targets.iter().position(|t| *t == target).unwrap() as u32 + 1
    }

    fn total(&self) -> u32 {
        self.targets.len() as u32
    }

    /// A target that keeps a placeholder: its step fails the next audit.
    fn break_target(&self, target: &str) {
        let path = self.project.path().join(target);
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("\n{{LEFTOVER}}\n");
        std::fs::write(&path, content).unwrap();
    }

    async fn accept(
        &self,
        run_id: &str,
        step: u32,
        reason: Option<&str>,
        agent: bool,
    ) -> crate::models::ApiResponse<crate::api::audit::AcceptAuditStepResponse> {
        self.accept_with(run_id, step, reason, agent, None).await
    }

    async fn accept_with(
        &self,
        run_id: &str,
        step: u32,
        reason: Option<&str>,
        agent: bool,
        override_unsound: Option<bool>,
    ) -> crate::models::ApiResponse<crate::api::audit::AcceptAuditStepResponse> {
        let bridge = agent.then(|| {
            axum::Extension(crate::core::bridge_token::BridgeCaller {
                token_id: "t".into(),
                project: Some(PROJECT_ID.into()),
                own_discussions: vec![],
                own_run: None,
                agent: None,
            })
        });
        crate::api::audit::accept_audit_step(
            axum::extract::State(self.state.clone()),
            axum::extract::Path((PROJECT_ID.to_string(), run_id.to_string(), step)),
            bridge,
            axum::Json(crate::api::audit::AcceptAuditStepRequest {
                reason: reason.map(str::to_string),
                override_unsound,
            }),
        )
        .await
        .0
    }

    async fn validate(&self) -> crate::models::ApiResponse<crate::models::AiAuditStatus> {
        crate::api::audit::validate_audit(
            axum::extract::State(self.state.clone()),
            axum::extract::Path(PROJECT_ID.to_string()),
        )
        .await
        .0
    }
}

fn started_steps(body: &str) -> Vec<u64> {
    sse_events(body, "step_start")
        .iter()
        .map(|e| e["step"].as_u64().unwrap())
        .collect()
}

/// KT-1211 (M4) — a step that failed for truncation leaves a document that
/// passes the old accept re-check (present, not empty, no placeholder): it is
/// accepted only with an explicit override, and the refusal shows the diff.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_truncated_step_is_accepted_only_with_an_explicit_override() {
    let fx = Audited::new().await;
    let rules = fx.step_of("docs/coding-rules.md");
    std::fs::write(
        fx.project.path().join("docs/coding-rules.md"),
        "# Coding rules\n",
    )
    .unwrap();
    arm(fx.tools.path(), "noop", &fx.targets);
    let broken = run_full(&fx.state, None).await;
    let done = sse_events(&broken, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{broken}");
    assert_eq!(done["steps_to_redo"], json!([rules]), "{broken}");
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    let failed = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert!(
        failed.step_warning.as_deref().unwrap_or("").contains("truncated"),
        "{:?}",
        failed.step_warning
    );

    // Without the override: refused, nothing recorded, the diff is shown.
    let refused = fx.accept(&interrupted, rules, Some("looks fine"), false).await;
    assert!(!refused.success);
    let error = refused.error.unwrap();
    assert!(
        error.starts_with(crate::api::audit::OVERRIDE_REQUIRED_PREFIX),
        "{error}"
    );
    assert!(error.contains("truncated"), "{error}");
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    assert!(steps
        .iter()
        .find(|s| s.step_index == rules)
        .unwrap()
        .accepted_at
        .is_none());

    // An explicit `false` is no override either.
    let still = fx
        .accept_with(&interrupted, rules, None, false, Some(false))
        .await;
    assert!(!still.success);

    // With it: accepted, and the override says what it overrode.
    let accepted = fx
        .accept_with(&interrupted, rules, Some("looks fine"), false, Some(true))
        .await;
    assert!(accepted.success, "{:?}", accepted.error);
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    let row = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert!(row.accepted_at.is_some());
    let reason = row.accepted_reason.as_deref().unwrap_or("");
    assert!(reason.contains("override") && reason.contains("looks fine"), "{reason}");
}

/// KT-1211 — a step an agent never finished (its row has no `ended_at` and no
/// failure reason: the backend died with it) is not accepted as it stands
/// either: the document it left is unvetted, so it needs the override too.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_step_that_never_ended_is_accepted_only_with_an_explicit_override() {
    let fx = Audited::new().await;
    let rules = fx.step_of("docs/coding-rules.md");
    std::fs::write(
        fx.project.path().join("docs/coding-rules.md"),
        "# Coding rules\n",
    )
    .unwrap();
    arm(fx.tools.path(), "noop", &fx.targets);
    let broken = run_full(&fx.state, None).await;
    let done = sse_events(&broken, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    // The state a crash leaves: started, never ended, no reason, no snapshot.
    {
        let run_id = interrupted.clone();
        fx.state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE audit_run_steps
                        SET ended_at = NULL, step_warning = NULL, cli_success = 0
                      WHERE audit_run_id = ?1 AND step_index = ?2",
                    rusqlite::params![run_id, rules as i64],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    let refused = fx.accept(&interrupted, rules, None, false).await;
    assert!(!refused.success);
    let error = refused.error.unwrap();
    assert!(
        error.starts_with(crate::api::audit::OVERRIDE_REQUIRED_PREFIX),
        "{error}"
    );
    assert!(error.contains("never ended"), "{error}");
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    assert!(steps
        .iter()
        .find(|s| s.step_index == rules)
        .unwrap()
        .accepted_at
        .is_none());

    let accepted = fx
        .accept_with(&interrupted, rules, Some("checked by hand"), false, Some(true))
        .await;
    assert!(accepted.success, "{:?}", accepted.error);
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    let row = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert!(row.accepted_at.is_some());
    assert!(
        row.accepted_reason.as_deref().unwrap_or("").contains("override"),
        "{:?}",
        row.accepted_reason
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_failed_step_retried_alone_completes_the_run_and_the_project_validates() {
    let fx = Audited::new().await;
    let rules = fx.step_of("docs/coding-rules.md");
    let testing = fx.step_of("docs/testing-quality.md");
    fx.break_target("docs/coding-rules.md");
    fx.break_target("docs/testing-quality.md");
    arm(fx.tools.path(), "noop", &fx.targets);
    let broken = run_full(&fx.state, None).await;
    let done = sse_events(&broken, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{broken}");
    assert_eq!(done["steps_to_redo"], json!([rules, testing]), "{broken}");
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();

    // A target still at the template stage, or gone, cannot be accepted.
    let refused = fx.accept(&interrupted, rules, None, false).await;
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("template placeholders"));
    let testing_path = fx.project.path().join("docs/testing-quality.md");
    let kept = std::fs::read_to_string(&testing_path).unwrap();
    std::fs::remove_file(&testing_path).unwrap();
    let refused = fx.accept(&interrupted, testing, None, false).await;
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("missing or empty"));
    std::fs::write(&testing_path, kept).unwrap();

    // Retrying one step runs that step only; the other failure stays failed.
    arm(fx.tools.path(), "fill", &["docs/coding-rules.md"]);
    let first_retry = run_full_retrying(&fx.state, Some(interrupted), Some(vec![rules])).await;
    let done = sse_events(&first_retry, "done")
        .pop()
        .expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{first_retry}");
    assert_eq!(started_steps(&first_retry), vec![rules as u64]);
    assert_eq!(done["steps_to_redo"], json!([testing]), "{first_retry}");
    let not_retried: Vec<Value> = sse_events(&first_retry, "step_skipped")
        .into_iter()
        .filter(|e| e["reason"] == "not_retried")
        .collect();
    assert_eq!(not_retried.len(), 1, "{first_retry}");
    assert_eq!(not_retried[0]["step"], testing);
    let still_interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, _) = persisted(&fx.state, &still_interrupted).await;
    assert_eq!(run.status, "Interrupted");
    let carried_failure = steps
        .iter()
        .find(|s| s.step_index == testing)
        .expect("the failure the retry left is still on record");
    assert!(!carried_failure.cli_success);
    assert!(carried_failure.step_warning.is_some());

    // The last failed step, retried alone, completes the run.
    arm(fx.tools.path(), "fill", &["docs/testing-quality.md"]);
    settle_on_validation(fx.tools.path());
    let last_retry =
        run_full_retrying(&fx.state, Some(still_interrupted), Some(vec![testing])).await;
    let done = sse_events(&last_retry, "done")
        .pop()
        .expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{last_retry}");
    assert_eq!(started_steps(&last_retry), vec![testing as u64]);
    let completed = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, _) = persisted(&fx.state, &completed).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(run.last_completed_step, fx.total());

    // Validated, without any global audit after the failure.
    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(
        crate::core::scanner::detect_audit_status(&fx.project.path().display().to_string()),
        crate::models::AiAuditStatus::Validated
    );
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn an_accepted_step_completes_the_run_and_the_project_validates() {
    let fx = Audited::new().await;
    let rules = fx.step_of("docs/coding-rules.md");
    arm(fx.tools.path(), "noop", &fx.targets);
    fail_steps(fx.tools.path(), &[rules]);
    let broken = run_full(&fx.state, None).await;
    let done = sse_events(&broken, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{broken}");
    assert_eq!(done["steps_to_redo"], json!([rules]), "{broken}");
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();

    // Never an agent's decision: off the bridge list, and refused in-handler.
    assert!(crate::core::bridge_token::route_for("POST", ACCEPT_ROUTE).is_none());
    let refused = fx.accept(&interrupted, rules, Some("agent"), true).await;
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("human decision"));

    let before = chrono::Utc::now();
    let accepted = fx
        .accept(
            &interrupted,
            rules,
            Some("  kept as written\nby hand "),
            false,
        )
        .await;
    assert!(accepted.success, "{:?}", accepted.error);
    let accepted = accepted.data.unwrap();
    assert_eq!(accepted.step, rules);
    assert_eq!(accepted.remaining_failed, Vec::<u32>::new());
    let (_, steps, _) = persisted(&fx.state, &interrupted).await;
    let row = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert_eq!(row.accepted_by.as_deref(), Some("operator"));
    assert_eq!(
        row.accepted_reason.as_deref(),
        Some("kept as written by hand")
    );
    assert!(row.accepted_at.unwrap() >= before - chrono::Duration::seconds(1));
    assert!(!row.cli_success, "the failure stays on record");
    let again = fx.accept(&interrupted, rules, None, false).await;
    assert!(!again.success);
    assert!(again.error.unwrap().contains("already done"));

    // The resume now only finalizes: nothing runs, the run completes.
    arm(fx.tools.path(), "noop", &[]);
    settle_on_validation(fx.tools.path());
    let finalized = run_full(&fx.state, Some(interrupted)).await;
    let done = sse_events(&finalized, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "complete", "{finalized}");
    assert!(started_steps(&finalized).is_empty(), "{finalized}");
    let completed = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(&fx.state, &completed).await;
    assert_eq!(run.status, "Completed");
    assert_eq!(outcomes(&run)["accepted"], json!([rules]));
    assert_eq!(outcomes(&run)["failed"], json!([]));
    let carried = steps.iter().find(|s| s.step_index == rules).unwrap();
    assert_eq!(carried.accepted_by.as_deref(), Some("operator"));
    assert!(!carried.cli_success);
    let prompt = prompt.expect("a Completed run has its validation discussion");
    let section = prompt
        .split("## ÉTAPES ACCEPTÉES")
        .nth(1)
        .unwrap_or_else(|| panic!("no accepted section: {prompt}"));
    let section = section.split("\n## ").next().unwrap();
    assert!(section.contains(&format!("{rules}/{} — `docs/coding-rules.md`", fx.total())));
    assert!(section.contains("kept as written by hand"), "{section}");
    assert!(section.contains("operator"), "{section}");

    let validated = fx.validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(
        crate::core::scanner::detect_audit_status(&fx.project.path().display().to_string()),
        crate::models::AiAuditStatus::Validated
    );
}

// ── KT-1169 — a launch never ignores an audit it could resume ────────────────

/// Every file under `docs/`, by path, so a refused launch can prove it wrote nothing.
fn docs_snapshot(project: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let key = path.strip_prefix(root).unwrap().display().to_string();
                out.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&project.join("docs"), project, &mut out);
    out
}

async fn latest_resumable_id(state: &AppState) -> Option<String> {
    state
        .db
        .with_conn(|conn| crate::db::audit_runs::latest_resumable(conn, PROJECT_ID))
        .await
        .unwrap()
        .map(|run| run.id)
}

async fn run_count(state: &AppState) -> usize {
    state
        .db
        .with_conn(|conn| Ok(crate::db::audit_runs::list_recent(conn, PROJECT_ID, 100)?.len()))
        .await
        .unwrap()
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_launch_that_would_ignore_a_resumable_run_is_refused_unless_fresh() {
    let fx = Audited::new().await;
    fx.break_target("docs/coding-rules.md");
    arm(fx.tools.path(), "noop", &fx.targets);
    let broken = run_full(&fx.state, None).await;
    let done = sse_events(&broken, "done").pop().expect("terminal done");
    validation_settled(&fx.state, &done).await;
    assert_eq!(done["status"], "interrupted", "{broken}");
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    assert_eq!(
        latest_resumable_id(&fx.state).await.as_deref(),
        Some(interrupted.as_str())
    );
    let docs_before = docs_snapshot(fx.project.path());
    let runs_before = run_count(&fx.state).await;

    // A plain launch is refused: no run, no agent, no document touched, and
    // the interrupted run stays the one to resume.
    arm(fx.tools.path(), "fill", &fx.targets);
    std::fs::write(fx.tools.path().join("steps.log"), "").unwrap();
    let refused = run_full(&fx.state, None).await;
    let error = sse_events(&refused, "error")
        .pop()
        .unwrap_or_else(|| panic!("{refused}"));
    let message = error["error"].as_str().unwrap();
    assert!(message.contains(&interrupted), "{message}");
    assert!(message.contains("resume_run_id"), "{message}");
    assert!(sse_events(&refused, "accepted").is_empty(), "{refused}");
    assert!(sse_events(&refused, "step_start").is_empty(), "{refused}");

    // A refresh of stale sections would supersede it too.
    let partial = crate::api::audit::drift::partial_audit(
        axum::extract::State(fx.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![1],
        }),
    )
    .await
    .into_response();
    let partial = sse_body(partial).await;
    let error = sse_events(&partial, "error")
        .pop()
        .unwrap_or_else(|| panic!("{partial}"));
    assert!(
        error["error"].as_str().unwrap().contains("resume it"),
        "{partial}"
    );

    assert_eq!(run_count(&fx.state).await, runs_before);
    assert_eq!(
        latest_resumable_id(&fx.state).await.as_deref(),
        Some(interrupted.as_str())
    );
    assert_eq!(docs_snapshot(fx.project.path()), docs_before);
    assert!(prompt_kinds(fx.tools.path()).is_empty(), "no agent ran");

    // The explicit choice replays every step on the existing documents: the
    // template is not installed again, and each filled document is re-verified.
    arm(fx.tools.path(), "noop", &fx.targets);
    let fresh = launch_full(&fx.state, None, None, Some(true)).await;
    assert!(sse_events(&fresh, "error").is_empty(), "{fresh}");
    assert_eq!(
        started_steps(&fresh),
        (1..=fx.total() as u64).collect::<Vec<_>>()
    );
    for event in sse_events(&fresh, "template_installed") {
        assert_eq!(event["installed"], false, "{fresh}");
    }
    let kinds = prompt_kinds(fx.tools.path());
    assert!(
        kinds.contains(&("docs/AGENTS.md".to_string(), true)),
        "a filled document is re-verified, not filled again: {kinds:?}"
    );
    let done = sse_events(&fresh, "done").pop().expect("terminal done");
    assert_ne!(done["audit_run_id"], json!(interrupted));
}

#[path = "revalidate_e2e_tests.rs"]
mod revalidate_e2e_tests;

#[path = "relocation_e2e_tests.rs"]
mod relocation_e2e_tests;

/// KT-1208 — a re-audit cannot undo a human TD decision, a sheet that leaves
/// the active list closes its task, and a partial refresh says it is
/// single-agent when the briefing asks for the cross-agent audit.
#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_reaudit_keeps_human_td_decisions_and_a_refresh_says_it_is_single_agent() {
    let fx = Audited::new().await;
    let td = fx.project.path().join("docs/tech-debt");
    std::fs::create_dir_all(&td).unwrap();
    let sheet = |status: &str, title: &str| {
        format!("---\ndescription: {title}\nmetadata:\n  type: tech-debt\n---\n# {title}\n\n- **Area**: `src/a.rs`\n- **Severity**: High\n- **Status**: {status}\n")
    };
    std::fs::write(
        td.join("TD-20261001-rej.md"),
        sheet("Rejected", "Unsafe call"),
    )
    .unwrap();
    std::fs::write(
        td.join("TD-20261001-adr.md"),
        sheet("Accepted decision", "Global state"),
    )
    .unwrap();
    // A Kronn task linked to a sheet the user rejected.
    let source = crate::core::td_tasks::source_from(
        "TD-20261001-rej",
        &sheet("Verified in source", "Unsafe call"),
        "docs/tech-debt/TD-20261001-rej.md".into(),
    );
    fx.state
        .db
        .with_conn(move |conn| {
            crate::core::td_tasks::convert(conn, PROJECT_ID, &[source], &Default::default())
        })
        .await
        .unwrap();
    // The agent reverts the rejection, deletes the accepted sheet and
    // recreates the rejected finding under a new id.
    let rej = td.join("TD-20261001-rej.md");
    let dup = fx.tools.path().join("dup.md");
    std::fs::write(&dup, sheet("Verified in source", "Unsafe call")).unwrap();
    let undo = format!(
        "sed 's/Status\\*\\*: Rejected/Status**: Verified in source/' '{r}' > '{r}.n' && mv '{r}.n' '{r}'\nrm -f '{a}'\ncp '{dup}' '{d}'\n",
        r = rej.display(),
        a = td.join("TD-20261001-adr.md").display(),
        d = td.join("TD-20261011-again.md").display(),
        dup = dup.display(),
    );
    std::fs::write(fx.tools.path().join("undo"), undo).unwrap();
    arm(fx.tools.path(), "noop", &fx.targets);
    let body = run_full(&fx.state, None).await;
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("{body}"));
    validation_settled(&fx.state, &done).await;
    assert!(
        !fx.tools.path().join("undo").exists(),
        "the agent ran its edits"
    );

    let guarded = sse_events(&body, "td_decisions")
        .pop()
        .unwrap_or_else(|| panic!("{body}"));
    assert_eq!(guarded["reverted"], json!(["TD-20261001-rej"]), "{guarded}");
    assert_eq!(guarded["restored"], json!(["TD-20261001-adr"]), "{guarded}");
    assert_eq!(
        guarded["settled_duplicates"],
        json!([["TD-20261011-again", "TD-20261001-rej"]]),
        "{guarded}"
    );
    let decision = |id: &str| {
        crate::core::audit_validation::td_decision(
            &std::fs::read_to_string(td.join(format!("{id}.md"))).unwrap(),
        )
    };
    use crate::core::audit_validation::TdDecision;
    assert_eq!(decision("TD-20261001-rej"), Some(TdDecision::Rejected));
    assert_eq!(
        decision("TD-20261001-adr"),
        Some(TdDecision::AcceptedDecision)
    );
    let status = fx
        .state
        .db
        .with_conn(|conn| {
            let key = crate::core::td_tasks::td_task_key(PROJECT_ID, "TD-20261001-rej");
            let (id, _) = crate::db::planning::task_by_idempotency_key(conn, &key)?
                .ok_or_else(|| anyhow::anyhow!("expected TD task"))?;
            Ok(crate::db::planning::get_task(conn, &id)?
                .ok_or_else(|| anyhow::anyhow!("expected stored TD task"))?
                .summary
                .status)
        })
        .await
        .unwrap();
    assert_eq!(status, crate::models::PlanningTaskStatus::Archived);

    // A refresh of the security step under the cross-agent option.
    let notes = format!(
        "{BRIEFING_MARKER}\n{} ClaudeCode, Codex\n",
        crate::core::cross_audit::BRIEFING_OPTION_PREFIX
    );
    std::fs::write(fx.project.path().join("docs/briefing.md"), &notes).unwrap();
    fx.state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, PROJECT_ID, Some(&notes))
        })
        .await
        .unwrap();
    let step = fx.step_of("docs/inconsistencies-security.md") as usize;
    let partial = crate::api::audit::drift::partial_audit(
        axum::extract::State(fx.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(crate::models::PartialAuditRequest {
            agent: AgentType::ClaudeCode,
            connection_id: None,
            tier: None,
            steps: vec![step],
        }),
    )
    .await
    .into_response();
    let partial = sse_body(partial).await;
    let notice = sse_events(&partial, "single_agent")
        .pop()
        .unwrap_or_else(|| panic!("{partial}"));
    assert!(
        notice["message"].as_str().unwrap().contains("single-agent"),
        "{partial}"
    );
    let done = sse_events(&partial, "done")
        .pop()
        .unwrap_or_else(|| panic!("{partial}"));
    assert_eq!(done["cross_agent_skipped"], true, "{partial}");
}
