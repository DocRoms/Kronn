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
/// count: the n-th one works on line n of `targets`. In `fill` mode it fills
/// that target's placeholders, or appends a line when there is nothing to fill;
/// in `noop` mode it changes nothing.
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
    echo "$n $target" >> '{state}/steps.log'
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

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_second_full_audit_of_an_up_to_date_repo_completes_with_unchanged_steps() {
    let _restore = TemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
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
    let _route = route_claude(project.path(), &fixture);

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
    let first = run_full(&state, None).await;
    let done = sse_events(&first, "done").pop().expect("terminal done");
    validation_settled(&state, &done).await;
    assert_eq!(done["status"], "complete", "{first}");
    assert!(sse_events(&first, "step_unchanged").is_empty(), "{first}");
    assert!(sse_events(&first, "step_warning").is_empty(), "{first}");

    // Second audit with an agent that changes nothing: Completed, every
    // documentary step `unchanged`, none failed.
    arm(tools.path(), "noop", &targets);
    let second = run_full(&state, None).await;
    let done = sse_events(&second, "done").pop().expect("terminal done");
    validation_settled(&state, &done).await;
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
    let (run, steps, prompt) = persisted(&state, &run_id).await;
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
    let third = run_full(&state, None).await;
    let done = sse_events(&third, "done").pop().expect("terminal done");
    validation_settled(&state, &done).await;
    assert_eq!(done["status"], "interrupted", "{third}");
    assert_eq!(done["steps_to_redo"], json!([broken]), "{third}");
    assert!(
        !sse_events(&third, "step_unchanged")
            .iter()
            .any(|e| e["step"] == broken),
        "{third}"
    );
    let interrupted = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, _, _) = persisted(&state, &interrupted).await;
    assert_eq!(run.status, "Interrupted");
    assert_eq!(outcomes(&run)["failed"], json!([broken]));

    // The resume replays only that step: the unchanged ones are skipped, and
    // the run they are carried into still knows them as unchanged.
    arm(tools.path(), "fill", &["docs/coding-rules.md"]);
    let fourth = run_full(&state, Some(interrupted)).await;
    let done = sse_events(&fourth, "done").pop().expect("terminal done");
    assert_eq!(done["status"], "complete", "{fourth}");
    let started: Vec<Value> = sse_events(&fourth, "step_start");
    assert_eq!(started.len(), 1, "{fourth}");
    assert_eq!(started[0]["step"], broken);
    assert_eq!(sse_events(&fourth, "step_skipped").len() as u32, total - 1);
    let run_id = done["audit_run_id"].as_str().unwrap().to_string();
    let (run, steps, prompt) = persisted(&state, &run_id).await;
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
