use super::*;
use crate::core::cross_audit::{Citation, RawFinding};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const STEP_PROMPT: &str = "SECURITY STEP PROMPT";
const CLAUDE: AgentType = AgentType::ClaudeCode;
const CODEX: AgentType = AgentType::Codex;
const OPENCODE: AgentType = AgentType::OpenCode;

/// A scripted agent pool: one answer per (role, agent), every call recorded.
#[derive(Default)]
struct Fake {
    answers: HashMap<(&'static str, String), String>,
    calls: Mutex<Vec<(String, &'static str, String)>>,
    /// Detection waits here: sequential calls would never get through.
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

fn role(prompt: &str) -> &'static str {
    if prompt.contains("DETECTION MODE") {
        "detect"
    } else if prompt.contains("CROSS-AGENT AUDIT — CROSS-CHECK") {
        "check"
    } else if prompt.contains("CROSS-AGENT AUDIT — RIGHT OF REPLY") {
        "reply"
    } else {
        "other"
    }
}

impl Fake {
    fn answer(mut self, role: &'static str, agent: &AgentType, text: String) -> Self {
        self.answers
            .insert((role, engine::agent_label(agent)), text);
        self
    }

    fn calls(&self, role: &str) -> Vec<(String, String)> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r, _)| *r == role)
            .map(|(a, _, p)| (a.clone(), p.clone()))
            .collect()
    }
}

impl CrossAgentRunner for Fake {
    fn run<'a>(
        &'a self,
        agent: &'a AgentType,
        prompt: String,
    ) -> BoxFuture<'a, Result<AgentReply, String>> {
        Box::pin(async move {
            let role = role(&prompt);
            let label = engine::agent_label(agent);
            self.calls
                .lock()
                .unwrap()
                .push((label.clone(), role, prompt));
            if role == "detect" {
                if let Some(barrier) = &self.barrier {
                    tokio::time::timeout(std::time::Duration::from_secs(5), barrier.wait())
                        .await
                        .map_err(|_| "detections ran one after the other".to_string())?;
                }
            }
            let text = self
                .answers
                .get(&(role, label))
                .cloned()
                .ok_or_else(|| "agent failed".to_string())?;
            Ok(AgentReply {
                text,
                usage: StepTokens {
                    input: Some(100),
                    output: Some(10),
                    ..StepTokens::UNKNOWN
                },
                cost_usd_micros: Some(1_000),
            })
        })
    }
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/db.rs"),
        "fn a() {}\nfn b() {}\nlet q = format!(\"SELECT * FROM t WHERE id = {}\", id);\nconn.execute(&q);\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/guard.rs"),
        "pub fn check(id: &str) -> bool {\n    id.chars().all(|c| c.is_ascii_digit())\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/web.rs"),
        "let cors = \"*\";\nallow_origin(cors);\n",
    )
    .unwrap();
    dir
}

fn cite(file: &str, line: u32, quote: &str) -> Citation {
    Citation {
        file: file.into(),
        line,
        line_end: None,
        quote: quote.into(),
    }
}

fn sql() -> RawFinding {
    RawFinding {
        title: "SQL query built with format!".into(),
        category: "security".into(),
        severity: "High".into(),
        citation: cite("src/db.rs", 3, "format!(\"SELECT * FROM t"),
        impact: "an attacker reads any row".into(),
        remediation: "bind the parameter".into(),
        td_id: None,
    }
}

fn cors() -> RawFinding {
    RawFinding {
        title: "CORS allows every origin".into(),
        category: "security".into(),
        severity: "Medium".into(),
        citation: cite("src/web.rs", 1, "let cors = \"*\""),
        impact: "any site calls the API".into(),
        remediation: "list the origins".into(),
        td_id: None,
    }
}

fn invented() -> RawFinding {
    RawFinding {
        title: "Hardcoded password in config".into(),
        citation: cite("src/config.rs", 7, "password = hunter2"),
        ..sql()
    }
}

fn findings(list: &[RawFinding]) -> String {
    format!(
        "Checked.\n```kronn-findings\n{}\n```\n",
        serde_json::json!({ "findings": list, "checklist": [{"item": "HSTS", "status": "verified absent"}] })
    )
}

fn verdicts(list: serde_json::Value) -> String {
    format!(
        "```kronn-verdicts\n{}\n```",
        serde_json::json!({ "verdicts": list })
    )
}

fn replies(list: serde_json::Value) -> String {
    format!(
        "```kronn-replies\n{}\n```",
        serde_json::json!({ "replies": list })
    )
}

fn guard_proof() -> serde_json::Value {
    serde_json::json!([{"file": "src/guard.rs", "line": 2, "quote": "is_ascii_digit"}])
}

#[tokio::test]
async fn detection_runs_in_parallel_and_no_agent_sees_another_agents_work() {
    let dir = repo();
    let fake = Fake {
        barrier: Some(Arc::new(tokio::sync::Barrier::new(3))),
        ..Default::default()
    }
    .answer("detect", &CLAUDE, findings(&[sql()]))
    .answer("detect", &CODEX, findings(&[cors()]))
    .answer("detect", &OPENCODE, findings(&[]));
    let agents = [CLAUDE, CODEX, OPENCODE];
    let report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    assert!(
        report.failed_agents.is_empty(),
        "{:?}",
        report.failed_agents
    );
    let detections = fake.calls("detect");
    assert_eq!(detections.len(), 3);
    for (agent, prompt) in &detections {
        assert!(prompt.starts_with(STEP_PROMPT));
        assert!(prompt.contains(&format!("You are agent `{agent}`")));
        assert!(prompt.contains("Do NOT create, edit or delete ANY file"));
        assert!(
            !prompt.contains(&sql().title) && !prompt.contains(&cors().title),
            "{agent} saw another agent's finding"
        );
    }
}

#[tokio::test]
async fn a_false_citation_is_dropped_without_a_model_call() {
    let dir = repo();
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[invented()]))
        .answer("detect", &CODEX, findings(&[]));
    let report = run_cross_step(&fake, dir.path(), &[CLAUDE, CODEX], STEP_PROMPT)
        .await
        .unwrap();
    assert_eq!(report.dropped.len(), 1);
    assert!(report.records.is_empty());
    assert_eq!(report.calls, 2, "only the two detections");
    assert!(fake.calls("check").is_empty());

    // Next to a true finding, the false one never reaches the challenger.
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[invented(), sql()]))
        .answer("detect", &CODEX, findings(&[]))
        .answer(
            "check",
            &CODEX,
            verdicts(serde_json::json!([{"id": "F1", "verdict": "confirm", "argument": "real"}])),
        );
    let report = run_cross_step(&fake, dir.path(), &[CLAUDE, CODEX], STEP_PROMPT)
        .await
        .unwrap();
    assert_eq!(report.records.len(), 1);
    let checks = fake.calls("check");
    assert_eq!(checks.len(), 1);
    assert!(!checks[0].1.contains(&invented().title));
}

#[tokio::test]
async fn a_finding_only_one_agent_found_is_kept_unless_refuted_with_proof() {
    let dir = repo();
    let agents = [CLAUDE, CODEX, OPENCODE];
    for (answer, expected) in [
        (
            Some(verdicts(
                serde_json::json!([{"id": "F1", "verdict": "confirm", "argument": "real"}]),
            )),
            Outcome::Confirmed,
        ),
        (None, Outcome::NotRefuted),
    ] {
        let mut fake = Fake::default()
            .answer("detect", &CLAUDE, findings(&[sql()]))
            .answer("detect", &CODEX, findings(&[]))
            .answer("detect", &OPENCODE, findings(&[]));
        if let Some(answer) = answer {
            fake = fake.answer("check", &CODEX, answer);
        }
        let report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
            .await
            .unwrap();
        let record = &report.records[0];
        assert_eq!(record.detection_count, 1);
        assert_eq!(record.outcome, expected);
        assert!(record.outcome.in_audit());
    }
}

#[tokio::test]
async fn a_refutation_without_proof_contests_and_a_proven_one_dismisses() {
    let dir = repo();
    let run = |verdict: serde_json::Value, reply: Option<String>| {
        let mut fake = Fake::default()
            .answer("detect", &CLAUDE, findings(&[sql()]))
            .answer("detect", &CODEX, findings(&[sql()]))
            .answer("check", &OPENCODE, verdicts(verdict));
        if let Some(reply) = reply {
            fake = fake.answer("reply", &CLAUDE, reply);
        }
        fake
    };
    let agents = [CLAUDE, CODEX, OPENCODE];

    let fake = run(
        serde_json::json!([{"id": "F1", "verdict": "refute", "argument": "trust me", "proof": []}]),
        None,
    );
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    assert_eq!(report.records[0].outcome, Outcome::Contested);
    assert_eq!(report.records[0].detection_count, 2);
    assert!(fake.calls("reply").is_empty(), "nothing to answer");
    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-a",
        &mut report,
    )
    .unwrap();
    let index =
        std::fs::read_to_string(dir.path().join("docs/inconsistencies-security.md")).unwrap();
    assert!(index
        .split("## Cross-check verdicts")
        .next()
        .unwrap()
        .contains("(contested)"));

    let fake = run(
        serde_json::json!([{"id": "F1", "verdict": "refute", "argument": "digits only", "proof": guard_proof()}]),
        Some(replies(
            serde_json::json!([{"id": "F1", "stance": "withdraw", "argument": "right"}]),
        )),
    );
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    assert_eq!(report.records[0].outcome, Outcome::Dismissed);
    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-b",
        &mut report,
    )
    .unwrap();
    let index =
        std::fs::read_to_string(dir.path().join("docs/inconsistencies-security.md")).unwrap();
    let table = index.split("## Cross-check verdicts").next().unwrap();
    assert!(
        !table.contains(&sql().title),
        "dismissed findings leave the audit"
    );
    let log = index
        .split("## Dismissed with verified proof (logged)")
        .nth(1)
        .unwrap();
    assert!(log.contains(&sql().title) && log.contains("[src: file: src/guard.rs:2]"));
    assert_eq!(report.records[0].td_id, None);
}

#[tokio::test]
async fn the_author_replies_once_and_a_serious_disagreement_becomes_a_human_card() {
    let dir = repo();
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[sql()]))
        .answer("detect", &CODEX, findings(&[]))
        .answer(
            "check",
            &CODEX,
            verdicts(serde_json::json!([{"id": "F1", "verdict": "refute", "argument": "AGAINST: digits only", "proof": guard_proof()}])),
        )
        .answer(
            "reply",
            &CLAUDE,
            replies(serde_json::json!([{"id": "F1", "stance": "maintain", "argument": "FOR: the guard is never called",
                "proof": [{"file": "src/db.rs", "line": 4, "quote": "conn.execute(&q)"}]}])),
        );
    let agents = [CLAUDE, CODEX];
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    let replies_sent = fake.calls("reply");
    assert_eq!(replies_sent.len(), 1, "one right of reply, no debate");
    assert_eq!(replies_sent[0].0, "ClaudeCode");
    assert!(replies_sent[0].1.contains("AGAINST: digits only"));
    assert_eq!(fake.calls("check").len(), 1);
    assert_eq!(report.records[0].outcome, Outcome::Disputed);
    assert_eq!(report.calls, 4);

    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-1234abcd",
        &mut report,
    )
    .unwrap();
    let td_id = report.records[0].td_id.clone().unwrap();
    let key = report.records[0].question_key.clone().unwrap();
    assert_eq!(key, format!("cross-audit.run1234a.{td_id}"));

    let (state, run_id) = state_with_run().await;
    let disputes = dispute_discussion(
        crate::api::other_projects_fixture::PROJECT_ID,
        &CLAUDE,
        "en",
        ModelTier::Reasoning,
        &report,
    )
    .unwrap();
    let disc_id = disputes.0.id.clone();
    let snapshot = report.clone();
    let rid = run_id.clone();
    let (questions, rows) = state
        .db
        .with_conn(move |conn| {
            persist(
                conn,
                &rid,
                1,
                "docs/inconsistencies-security.md",
                &agents,
                &snapshot,
                Some(&disputes),
            )?;
            let questions = crate::db::discussion_questions::list(conn, &disc_id)?;
            let rows = crate::db::audit_cross::list_findings(conn, &rid)?;
            Ok((questions, rows))
        })
        .await
        .unwrap();
    let pending = serde_json::to_value(&questions).unwrap();
    let text = pending.to_string();
    assert!(text.contains(&key), "{text}");
    assert!(
        text.contains("FOR: the guard is never called") && text.contains("AGAINST: digits only"),
        "{text}"
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.question_key.as_deref(), Some(key.as_str()));
}

#[tokio::test]
async fn every_finding_keeps_its_provenance() {
    let dir = repo();
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[sql(), cors()]))
        .answer("detect", &CODEX, findings(&[sql()]))
        .answer("detect", &OPENCODE, findings(&[]))
        .answer(
            "check",
            &OPENCODE,
            verdicts(serde_json::json!([
                {"id": "F1", "verdict": "confirm", "argument": "format! with input"},
            ])),
        )
        .answer(
            "check",
            &CODEX,
            verdicts(
                serde_json::json!([{"id": "F2", "verdict": "confirm", "argument": "wildcard"}]),
            ),
        );
    let agents = [CLAUDE, CODEX, OPENCODE];
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-p",
        &mut report,
    )
    .unwrap();
    let (state, run_id) = state_with_run().await;
    let snapshot = report.clone();
    let rid = run_id.clone();
    let rows = state
        .db
        .with_conn(move |conn| {
            persist(
                conn,
                &rid,
                1,
                "docs/inconsistencies-security.md",
                &agents,
                &snapshot,
                None,
            )?;
            crate::db::audit_cross::list_findings(conn, &rid)
        })
        .await
        .unwrap();
    let sql_row = &rows.iter().find(|(_, r)| r.title == sql().title).unwrap().1;
    assert_eq!(sql_row.author, "ClaudeCode");
    assert_eq!(sql_row.detected_by, vec!["ClaudeCode", "Codex"]);
    assert_eq!(sql_row.detection_count, 2);
    assert_eq!(sql_row.challenger.as_deref(), Some("OpenCode"));
    assert!(matches!(
        sql_row.challenge,
        Some(ChallengeResult::Confirmed { .. })
    ));
    let cors_row = &rows
        .iter()
        .find(|(_, r)| r.title == cors().title)
        .unwrap()
        .1;
    assert_eq!(cors_row.challenger.as_deref(), Some("Codex"));
    let sheet = std::fs::read_to_string(dir.path().join(format!(
        "docs/tech-debt/{}.md",
        sql_row.td_id.clone().unwrap()
    )))
    .unwrap();
    assert!(sheet.contains("- **Found by**: ClaudeCode (author), Codex"));
    assert!(sheet.contains("- **Checked by**: OpenCode — confirmed: format! with input"));
}

#[tokio::test]
async fn a_step_where_no_agent_answered_fails() {
    let dir = repo();
    let fake = Fake::default().answer("detect", &CLAUDE, "no block".into());
    let error = run_cross_step(&fake, dir.path(), &[CLAUDE, CODEX], STEP_PROMPT)
        .await
        .unwrap_err();
    assert!(
        error.contains("no agent returned a usable detection"),
        "{error}"
    );
}

#[tokio::test]
async fn usage_and_cost_add_up_over_every_call() {
    let dir = repo();
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[sql()]))
        .answer("detect", &CODEX, findings(&[]))
        .answer(
            "check",
            &CODEX,
            verdicts(serde_json::json!([{"id": "F1", "verdict": "confirm", "argument": "ok"}])),
        );
    let report = run_cross_step(&fake, dir.path(), &[CLAUDE, CODEX], STEP_PROMPT)
        .await
        .unwrap();
    assert_eq!(report.calls, 3);
    assert_eq!(report.usage().total(), Some(330));
    assert_eq!(report.cost_usd_micros(), Some(3_000));
}

async fn state_with_run() -> (AppState, String) {
    let state = crate::api::other_projects_fixture::fresh_state();
    crate::api::other_projects_fixture::register_project(
        &state,
        crate::api::other_projects_fixture::PROJECT_ID,
        "audited",
        "/tmp/kronn-cross-audit-unused",
        serde_json::json!({}),
    )
    .await;
    let run_id = Uuid::new_v4().to_string();
    let rid = run_id.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::audit_runs::insert_running(
                conn,
                &rid,
                crate::api::other_projects_fixture::PROJECT_ID,
                "Security",
                "ClaudeCode",
                Utc::now(),
            )
        })
        .await
        .unwrap();
    (state, run_id)
}

#[tokio::test]
async fn the_estimate_scales_the_last_measured_single_agent_step() {
    let (state, run_id) = state_with_run().await;
    let label = "docs/inconsistencies-security.md";
    let get = |state: AppState, agents: u32| async move {
        cross_audit_estimate(
            State(state),
            AxumPath(crate::api::other_projects_fixture::PROJECT_ID.to_string()),
            Query(CrossEstimateQuery {
                agents: Some(agents),
            }),
        )
        .await
        .0
        .data
        .unwrap()
    };
    let none = get(state.clone(), 2).await;
    assert!(none.estimated);
    assert_eq!((none.low_factor, none.high_factor), (2, 4));
    assert_eq!(none.base_tokens, None);

    let rid = run_id.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::audit_runs::insert_audit_step_start(conn, &rid, 1, label, Utc::now())?;
            let tokens = StepTokens {
                input: Some(9_000),
                output: Some(1_000),
                ..StepTokens::UNKNOWN
            };
            crate::db::audit_runs::finalize_audit_step(
                conn,
                &rid,
                1,
                Utc::now(),
                10,
                &tokens,
                Some(10_000),
                true,
                None,
                false,
            )?;
            crate::db::audit_runs::set_step_cost(conn, &rid, 1, 50_000)
        })
        .await
        .unwrap();
    let base = get(state.clone(), 3).await;
    assert!(base.estimated);
    assert_eq!(base.base_run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        (base.low_tokens, base.high_tokens),
        (Some(30_000), Some(60_000))
    );
    assert_eq!(
        (base.low_cost_usd_micros, base.high_cost_usd_micros),
        (Some(150_000), Some(300_000))
    );

    // A measured cross-agent run with as many agents replaces the guess.
    let (_, cross_run) = {
        let rid = Uuid::new_v4().to_string();
        let r2 = rid.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::audit_runs::insert_running(
                    conn,
                    &r2,
                    crate::api::other_projects_fixture::PROJECT_ID,
                    "Security",
                    "ClaudeCode",
                    Utc::now(),
                )?;
                let report = CrossStepReport {
                    usage: Some(StepTokens {
                        input: Some(40_000),
                        output: Some(2_000),
                        ..StepTokens::UNKNOWN
                    }),
                    cost: CallCost::Known(210_000),
                    calls: 6,
                    ..Default::default()
                };
                persist(
                    conn,
                    &r2,
                    1,
                    label,
                    &[CLAUDE, CODEX, OPENCODE],
                    &report,
                    None,
                )
            })
            .await
            .unwrap();
        ((), rid)
    };
    let measured = get(state.clone(), 3).await;
    assert!(!measured.estimated);
    assert_eq!(
        measured.measured_run_id.as_deref(),
        Some(cross_run.as_str())
    );
    assert_eq!(measured.measured_cost_usd_micros, Some(210_000));
    let other_n = get(state, 2).await;
    assert!(
        other_n.estimated,
        "a 3-agent measure says nothing about 2 agents"
    );
}

#[test]
fn the_briefing_writes_the_option_under_the_kronn_options_heading() {
    use crate::api::audit::briefing::{with_all_kronn_options, with_kronn_options};
    let body = "# Project Briefing\n".to_string();
    assert_eq!(with_all_kronn_options(body.clone(), false, &[]), body);
    assert_eq!(
        with_all_kronn_options(body.clone(), true, &[]),
        with_kronn_options(body.clone(), true)
    );
    let both = with_all_kronn_options(body.clone(), true, &[CLAUDE, CODEX]);
    assert_eq!(both.matches("## Kronn options").count(), 1);
    assert!(crate::core::td_tasks::briefing_wants_td_tasks(&both));
    assert_eq!(
        engine::briefing_cross_agents(&both),
        Some(vec![CLAUDE, CODEX])
    );
}

/// Clears a project's docs-root override when the test ends.
struct Relocated(String);
impl Drop for Relocated {
    fn drop(&mut self) {
        crate::core::docs_location::set_project_override(&self.0, None);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_relocated_project_gets_the_cross_step_documents_under_its_root() {
    use crate::core::docs_location::DocsRoot;
    let dir = repo();
    let path = dir.path().to_string_lossy().into_owned();
    crate::core::docs_location::set_project_override(&path, Some(".ai"));
    let _relocated = Relocated(path);
    let ai = DocsRoot::from_rel(".ai").unwrap();
    let target = ".ai/inconsistencies-security.md";
    assert!(engine::is_cross_checked_target(target, &ai));

    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[sql()]))
        .answer("detect", &CODEX, findings(&[sql()]))
        .answer(
            "check",
            &OPENCODE,
            verdicts(serde_json::json!([{"id": "F1", "verdict": "refute", "argument": "trust me", "proof": []}])),
        );
    let agents = [CLAUDE, CODEX, OPENCODE];
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    write_outputs(dir.path(), target, &agents, "run-ai", &mut report).unwrap();

    let index = std::fs::read_to_string(dir.path().join(target)).unwrap();
    assert!(
        index.contains("Security findings (cross-agent audit)"),
        "{index}"
    );
    let td_id = report.records[0].td_id.clone().unwrap();
    assert!(dir
        .path()
        .join(format!(".ai/tech-debt/{td_id}.md"))
        .is_file());
    assert!(!dir.path().join("docs").exists());

    let estimate = estimate(2, None, None, &ai);
    assert_eq!(estimate.cross_checked_steps, vec![target.to_string()]);
}

// ── KT-1208: the human answer to a dispute card is applied ──

struct Disputed {
    dir: tempfile::TempDir,
    state: AppState,
    disc_id: String,
    question_id: String,
    td_id: String,
    run_id: String,
}

async fn disputed_project() -> Disputed {
    let dir = repo();
    let fake = Fake::default()
        .answer("detect", &CLAUDE, findings(&[sql()]))
        .answer("detect", &CODEX, findings(&[]))
        .answer(
            "check",
            &CODEX,
            verdicts(serde_json::json!([{"id": "F1", "verdict": "refute", "argument": "AGAINST", "proof": guard_proof()}])),
        )
        .answer(
            "reply",
            &CLAUDE,
            replies(serde_json::json!([{"id": "F1", "stance": "maintain", "argument": "FOR",
                "proof": [{"file": "src/db.rs", "line": 4, "quote": "conn.execute(&q)"}]}])),
        );
    let agents = [CLAUDE, CODEX];
    let mut report = run_cross_step(&fake, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-1",
        &mut report,
    )
    .unwrap();
    let td_id = report.records[0].td_id.clone().unwrap();
    let state = crate::api::other_projects_fixture::fresh_state();
    crate::api::other_projects_fixture::register_project(
        &state,
        crate::api::other_projects_fixture::PROJECT_ID,
        "audited",
        &dir.path().to_string_lossy(),
        serde_json::json!({}),
    )
    .await;
    let run_id = Uuid::new_v4().to_string();
    let disputes = dispute_discussion(
        crate::api::other_projects_fixture::PROJECT_ID,
        &CLAUDE,
        "en",
        ModelTier::Reasoning,
        &report,
    )
    .unwrap();
    let disc_id = disputes.0.id.clone();
    let (rid, did) = (run_id.clone(), disc_id.clone());
    let question_id = state
        .db
        .with_conn(move |conn| {
            crate::db::audit_runs::insert_running(
                conn,
                &rid,
                crate::api::other_projects_fixture::PROJECT_ID,
                "Security",
                "ClaudeCode",
                Utc::now(),
            )?;
            persist(
                conn,
                &rid,
                1,
                "docs/inconsistencies-security.md",
                &agents,
                &report,
                Some(&disputes),
            )?;
            Ok(
                crate::db::discussion_questions::list(conn, &did)?.questions[0]
                    .id
                    .clone(),
            )
        })
        .await
        .unwrap();
    Disputed {
        dir,
        state,
        disc_id,
        question_id,
        td_id,
        run_id,
    }
}

async fn answer_card(d: &Disputed, option: &str) -> axum::http::StatusCode {
    crate::api::discussion_questions::answer(
        axum::extract::State(d.state.clone()),
        axum::extract::Path((d.disc_id.clone(), d.question_id.clone())),
        axum::Json(
            crate::db::discussion_questions::AnswerDiscussionQuestionRequest {
                selected_option_ids: vec![option.to_string()],
                item_answers: vec![],
                text: None,
                idempotency_key: format!("answer-{option}"),
            },
        ),
    )
    .await
    .0
}

async fn pending(d: &Disputed) -> Vec<crate::db::audit_cross::PendingDispute> {
    d.state
        .db
        .with_read_conn(|conn| {
            crate::db::audit_cross::pending_disputes(
                conn,
                crate::api::other_projects_fixture::PROJECT_ID,
            )
        })
        .await
        .unwrap()
}

fn sheet_of(d: &Disputed) -> String {
    std::fs::read_to_string(d.dir.path().join(format!("docs/tech-debt/{}.md", d.td_id))).unwrap()
}

fn index_of(d: &Disputed) -> String {
    std::fs::read_to_string(d.dir.path().join("docs/inconsistencies-security.md")).unwrap()
}

#[tokio::test]
async fn dismissing_a_dispute_rejects_the_sheet_and_clears_the_pending_label() {
    let d = disputed_project().await;
    assert!(index_of(&d).contains("(disputed — human decision pending)"));
    let items = crate::core::td_review::read_items(&d.dir.path().join("docs"));
    let open = pending(&d).await;
    assert_eq!(open.len(), 1);
    let refusal = crate::api::audit::validate::open_disputes_refusal(&items, &open).unwrap();
    assert!(
        refusal.contains(&d.td_id) && refusal.contains("1 cross-agent dispute"),
        "{refusal}"
    );

    assert_eq!(answer_card(&d, "dismiss").await, axum::http::StatusCode::OK);
    let sheet = sheet_of(&d);
    assert_eq!(
        crate::core::audit_validation::td_decision(&sheet),
        Some(crate::core::audit_validation::TdDecision::Rejected)
    );
    assert!(
        sheet.contains("- **Verdict**: disputed — dismissed by a human"),
        "{sheet}"
    );
    let index = index_of(&d);
    assert!(!index.contains("human decision pending"), "{index}");
    assert!(index.contains("disputed — dismissed by a human"));
    assert!(pending(&d).await.is_empty());
    let rid = d.run_id.clone();
    let rows = d
        .state
        .db
        .with_read_conn(move |conn| crate::db::audit_cross::list_findings(conn, &rid))
        .await
        .unwrap();
    assert_eq!(
        rows[0].1.human_decision,
        Some(crate::core::cross_audit::HumanDecision::Dismiss)
    );
    let items = crate::core::td_review::read_items(&d.dir.path().join("docs"));
    assert!(crate::api::audit::validate::open_disputes_refusal(&items, &[]).is_none());
}

#[tokio::test]
async fn keeping_a_dispute_leaves_the_sheet_active_and_says_so() {
    let d = disputed_project().await;
    let before = crate::core::td_index::parse_sheet(&d.td_id, &sheet_of(&d));
    assert_eq!(answer_card(&d, "keep").await, axum::http::StatusCode::OK);
    let after = crate::core::td_index::parse_sheet(&d.td_id, &sheet_of(&d));
    assert!(after.is_active());
    assert_eq!(after.status, before.status);
    assert!(sheet_of(&d).contains("- **Verdict**: disputed — kept by a human"));
    let index = index_of(&d);
    assert!(index.contains("(disputed — kept by a human)") && !index.contains("decision pending"));
    assert!(pending(&d).await.is_empty());
}

#[tokio::test]
async fn a_dispute_waits_for_a_running_audit_and_a_wrong_option_changes_nothing() {
    let d = disputed_project().await;
    let pid = crate::api::other_projects_fixture::PROJECT_ID;
    assert!(d.state.audit_tracker.lock().unwrap().try_acquire_lease(pid));
    let before = (sheet_of(&d), index_of(&d));
    assert_eq!(
        answer_card(&d, "dismiss").await,
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!((sheet_of(&d), index_of(&d)), before);
    assert_eq!(pending(&d).await.len(), 1);
    d.state.audit_tracker.lock().unwrap().release_lease(pid);
    // An answer without keep or dismiss is refused, the card stays pending.
    let status = crate::api::discussion_questions::answer(
        axum::extract::State(d.state.clone()),
        axum::extract::Path((d.disc_id.clone(), d.question_id.clone())),
        axum::Json(
            crate::db::discussion_questions::AnswerDiscussionQuestionRequest {
                selected_option_ids: vec![],
                item_answers: vec![],
                text: Some("not sure".into()),
                idempotency_key: "text".into(),
            },
        ),
    )
    .await
    .0;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!((sheet_of(&d), index_of(&d)), before);
    assert_eq!(pending(&d).await.len(), 1);
}

#[tokio::test]
async fn a_dispute_card_refuses_with_docs_busy_and_writes_nothing_while_the_lease_is_held() {
    let d = disputed_project().await;
    let pid = crate::api::other_projects_fixture::PROJECT_ID;
    let before = (sheet_of(&d), index_of(&d));
    let decisions_before = d
        .state
        .db
        .with_read_conn({
            let (disc, question) = (d.disc_id.clone(), d.question_id.clone());
            move |conn| {
                Ok(crate::db::discussion_questions::list(conn, &disc)?
                    .questions
                    .into_iter()
                    .find(|card| card.id == question)
                    .map(|card| card.state))
            }
        })
        .await
        .unwrap();
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&d.state, pid).unwrap();

    let answered = crate::api::discussion_questions::answer(
        axum::extract::State(d.state.clone()),
        axum::extract::Path((d.disc_id.clone(), d.question_id.clone())),
        axum::Json(
            crate::db::discussion_questions::AnswerDiscussionQuestionRequest {
                selected_option_ids: vec!["dismiss".to_string()],
                item_answers: vec![],
                text: None,
                idempotency_key: "answer-busy".into(),
            },
        ),
    )
    .await;
    let declined = crate::api::discussion_questions::decline(
        axum::extract::State(d.state.clone()),
        axum::extract::Path((d.disc_id.clone(), d.question_id.clone())),
        axum::Json(
            crate::db::discussion_questions::DeclineDiscussionQuestionRequest {
                idempotency_key: "decline-busy".into(),
                reason: None,
            },
        ),
    )
    .await;
    assert_eq!(answered.0, axum::http::StatusCode::CONFLICT);
    assert_eq!(declined.0, axum::http::StatusCode::CONFLICT);
    for refusal in [answered.1 .0.error, declined.1 .0.error] {
        assert_eq!(
            refusal.as_deref(),
            Some(crate::api::audit::docs_write::DOCS_BUSY)
        );
    }
    assert_eq!((sheet_of(&d), index_of(&d)), before, "a refused card wrote docs");
    assert_eq!(pending(&d).await.len(), 1);
    let decisions_after = d
        .state
        .db
        .with_read_conn({
            let (disc, question) = (d.disc_id.clone(), d.question_id.clone());
            move |conn| {
                Ok(crate::db::discussion_questions::list(conn, &disc)?
                    .questions
                    .into_iter()
                    .find(|card| card.id == question)
                    .map(|card| card.state))
            }
        })
        .await
        .unwrap();
    assert_eq!(decisions_after, decisions_before, "the card was resolved");

    // Released, the same card is decided: the lease alone refused it.
    drop(lease);
    assert_eq!(answer_card(&d, "dismiss").await, axum::http::StatusCode::OK);
    assert!(pending(&d).await.is_empty());
}

#[tokio::test]
async fn declining_a_dispute_keeps_the_finding_and_ends_the_pending_label() {
    let d = disputed_project().await;
    let status = crate::api::discussion_questions::decline(
        axum::extract::State(d.state.clone()),
        axum::extract::Path((d.disc_id.clone(), d.question_id.clone())),
        axum::Json(
            crate::db::discussion_questions::DeclineDiscussionQuestionRequest {
                idempotency_key: "decline".into(),
                reason: None,
            },
        ),
    )
    .await
    .0;
    assert_eq!(status, axum::http::StatusCode::OK);
    let index = index_of(&d);
    assert!(
        index.contains("(disputed — left undecided by a human)"),
        "{index}"
    );
    assert!(crate::core::td_index::parse_sheet(&d.td_id, &sheet_of(&d)).is_active());
    assert!(pending(&d).await.is_empty());
}

#[tokio::test]
async fn a_card_outside_the_recorded_dispute_discussion_moves_nothing() {
    let d = disputed_project().await;
    let did = d.disc_id.clone();
    let key = format!("cross-audit.run1.{}", d.td_id);
    let found = d
        .state
        .db
        .with_read_conn(move |conn| {
            Ok((
                crate::db::audit_cross::dispute_target(conn, &did, &key)?.is_some(),
                crate::db::audit_cross::dispute_target(conn, "another-room", &key)?.is_some(),
            ))
        })
        .await
        .unwrap();
    assert_eq!(found, (true, false));
}

#[tokio::test]
async fn a_rerun_keeps_undetected_and_settled_rows_and_reports_unchanged() {
    let dir = repo();
    let agents = [CLAUDE, CODEX];
    let confirmed = || {
        Fake::default()
            .answer("detect", &CLAUDE, findings(&[sql()]))
            .answer("detect", &CODEX, findings(&[]))
            .answer(
                "check",
                &CODEX,
                verdicts(serde_json::json!([{"id": "F1", "verdict": "confirm", "argument": "real", "proof": []}])),
            )
    };
    let mut first = run_cross_step(&confirmed(), dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    assert!(write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-1",
        &mut first
    )
    .unwrap());
    let mut again = run_cross_step(&confirmed(), dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    assert!(
        !write_outputs(
            dir.path(),
            "docs/inconsistencies-security.md",
            &agents,
            "run-2",
            &mut again
        )
        .unwrap(),
        "the same findings rewrite nothing: the step is unchanged"
    );

    // A person adds a row and a human-owned note; another sheet gets rejected.
    let index_path = dir.path().join("docs/inconsistencies-security.md");
    let td = dir.path().join("docs/tech-debt");
    std::fs::write(
        td.join("TD-20250101-rej.md"),
        "# r\n- **Area**: `src/web.rs`\n- **Status**: Rejected\n- **Problem**: Old CORS\n",
    )
    .unwrap();
    let human_row = "| TD-20250101-human | Written by a person | `src/web.rs` | Low |";
    let edited = index_of_text(&index_path).replace(
        "|----|---------|------|----------|\n",
        &format!("|----|---------|------|----------|\n{human_row}\n"),
    ) + "\nSee TD-20250101-rej.\n\n<!-- kronn:section name=\"notes\" owner=\"human\" -->\nKeep me.\n<!-- kronn:section:end -->\n";
    std::fs::write(&index_path, &edited).unwrap();

    // The next run finds nothing at all.
    let empty = Fake::default()
        .answer("detect", &CLAUDE, findings(&[]))
        .answer("detect", &CODEX, findings(&[]));
    let mut third = run_cross_step(&empty, dir.path(), &agents, STEP_PROMPT)
        .await
        .unwrap();
    write_outputs(
        dir.path(),
        "docs/inconsistencies-security.md",
        &agents,
        "run-3",
        &mut third,
    )
    .unwrap();
    let index = index_of_text(&index_path);
    let carried = index
        .split(crate::core::cross_audit::CARRIED_HEADING)
        .nth(1)
        .unwrap();
    assert!(carried.contains(human_row), "{index}");
    assert!(
        carried.contains("SQL query built with format!"),
        "an active sheet is never dropped: {index}"
    );
    let settled = index
        .split(crate::core::cross_audit::SETTLED_HEADING)
        .nth(1)
        .unwrap();
    assert!(
        settled.contains("`TD-20250101-rej` — Old CORS — rejected"),
        "{index}"
    );
    assert!(
        index.contains("Keep me."),
        "a human-owned section survives: {index}"
    );
}

fn index_of_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test]
async fn the_card_flags_an_open_dispute_and_validation_waits_for_it() {
    let d = disputed_project().await;
    let pid = crate::api::other_projects_fixture::PROJECT_ID.to_string();
    let list = |state: AppState, pid: String| async move {
        crate::api::audit::td_review::list_tds(
            axum::extract::State(state),
            axum::extract::Path(pid),
        )
        .await
        .0
        .data
        .unwrap()
    };
    let before = list(d.state.clone(), pid.clone()).await;
    assert_eq!(before.gate.open_disputes, 1);
    assert!(!before.gate.met);
    assert_eq!(
        before.dispute_discussion_id.as_deref(),
        Some(d.disc_id.as_str())
    );
    let refused = crate::api::audit::validate::validate_audit(
        axum::extract::State(d.state.clone()),
        axum::extract::Path(pid.clone()),
    )
    .await
    .0;
    assert!(refused
        .error
        .unwrap_or_default()
        .contains("cross-agent dispute"));

    assert_eq!(answer_card(&d, "keep").await, axum::http::StatusCode::OK);
    let after = list(d.state.clone(), pid.clone()).await;
    assert_eq!(after.gate.open_disputes, 0);
    assert_eq!(after.dispute_discussion_id, None);
    let next = crate::api::audit::validate::validate_audit(
        axum::extract::State(d.state.clone()),
        axum::extract::Path(pid),
    )
    .await
    .0;
    assert!(!next
        .error
        .unwrap_or_default()
        .contains("cross-agent dispute"));
}
