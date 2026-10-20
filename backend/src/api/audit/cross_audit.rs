//! Cross-agent audit of a finding step (KT-1183, experimental): the model
//! calls and the files. Every decision is taken by `core::cross_audit`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use chrono::Utc;
use futures::future::BoxFuture;
use uuid::Uuid;

use crate::core::cross_audit::{
    self as engine, ChallengeResult, ChecklistLine, CrossFindingRecord, DetectionOutput,
    DroppedFinding, FindingGroup, IndexInput, Outcome, RepliesOutput, ReplyResult, VerdictsOutput,
};
use crate::db::audit_runs::{StepTokens, TokenAccounting};
use crate::models::*;
use crate::AppState;

/// The chosen agents, read from `docs/briefing.md` first, then the DB notes.
pub(crate) fn briefing_cross_agents(
    file_notes: Option<&str>,
    db_notes: Option<&str>,
) -> Option<Vec<AgentType>> {
    file_notes
        .or(db_notes)
        .and_then(engine::briefing_cross_agents)
}

/// What one model call returned.
#[derive(Debug, Clone)]
pub(crate) struct AgentReply {
    pub text: String,
    pub usage: StepTokens,
    pub cost_usd_micros: Option<u64>,
}

/// Runs one prompt on one agent. The production runner launches the audit's
/// agents; tests script it.
pub(crate) trait CrossAgentRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        agent: &'a AgentType,
        prompt: String,
    ) -> BoxFuture<'a, Result<AgentReply, String>>;
}

/// A text-silent agent this long is stalled.
const CALL_STALL: std::time::Duration = std::time::Duration::from_secs(600);

/// Launches each agent the way an audit step does, collecting its answer.
pub(crate) struct LauncherRunner {
    pub state: AppState,
    pub tier: ModelTier,
    pub project_path: PathBuf,
    pub project_path_str: String,
    pub tokens: TokensConfig,
    pub step_target: String,
}

impl CrossAgentRunner for LauncherRunner {
    fn run<'a>(
        &'a self,
        agent: &'a AgentType,
        prompt: String,
    ) -> BoxFuture<'a, Result<AgentReply, String>> {
        Box::pin(async move {
            let launcher =
                super::agent_launch::AuditAgentLauncher::for_request(&self.state, agent, None)
                    .await?;
            let cancel = launcher
                .stops_with_token()
                .then(tokio_util::sync::CancellationToken::new);
            let process = launcher
                .start(
                    agent,
                    self.tier,
                    &self.project_path,
                    &self.project_path_str,
                    &prompt,
                    &self.tokens,
                    cancel,
                    &self.step_target,
                    None,
                )
                .await?;
            let output = crate::workflows::steps::drive_agent_to_output(
                process,
                None,
                None,
                CALL_STALL,
                agent,
                "cross-agent audit",
            )
            .await
            .map_err(|e| e.to_string())?;
            let usage = match output.reported_usage {
                Some(usage) => StepTokens::from_reported(Some(usage)).inclusive_for(agent),
                None => match output.tokens_used {
                    Some(total) => StepTokens {
                        input: Some(total),
                        output: Some(0),
                        ..StepTokens::UNKNOWN
                    },
                    None => StepTokens::UNKNOWN,
                },
            };
            Ok(AgentReply {
                text: output.text,
                usage,
                cost_usd_micros: output
                    .reported_cost_usd
                    .filter(|c| c.is_finite() && *c >= 0.0)
                    .map(|c| (c * 1_000_000.0).round() as u64),
            })
        })
    }
}

/// What the cross-checked step produced, before it is written.
#[derive(Debug, Default, Clone)]
pub(crate) struct CrossStepReport {
    pub records: Vec<CrossFindingRecord>,
    pub dropped: Vec<DroppedFinding>,
    pub checklists: Vec<(AgentType, Vec<ChecklistLine>)>,
    pub failed_agents: Vec<(AgentType, String)>,
    pub calls: u32,
    usage: Option<StepTokens>,
    cost: CallCost,
}

impl CrossStepReport {
    /// Every call's usage summed; unknown when one call reported none.
    pub fn usage(&self) -> StepTokens {
        self.usage.unwrap_or(StepTokens::UNKNOWN)
    }

    pub fn cost_usd_micros(&self) -> Option<u64> {
        match self.cost {
            CallCost::Known(c) => Some(c),
            _ => None,
        }
    }

    fn count(&mut self, reply: &Result<AgentReply, String>) {
        self.calls += 1;
        let (usage, cost) = match reply {
            Ok(r) => (r.usage, r.cost_usd_micros),
            Err(_) => (StepTokens::UNKNOWN, None),
        };
        let usage = StepTokens {
            accounting: TokenAccounting::AsReported,
            ..usage
        };
        self.usage = Some(match self.usage {
            None => usage,
            Some(sum) => sum.plus(usage),
        });
        self.cost = match (self.cost, cost) {
            (CallCost::None, Some(c)) => CallCost::Known(c),
            (CallCost::Known(s), Some(c)) => CallCost::Known(s.saturating_add(c)),
            _ => CallCost::Unknown,
        };
    }
}

#[derive(Debug, Default, Clone, Copy)]
enum CallCost {
    #[default]
    None,
    Known(u64),
    Unknown,
}

/// Steps 1 to 7: independent detection, citation filter, matching,
/// cross-check, right of reply, arbitration. Model calls only for detection,
/// refutation and reply.
pub(crate) async fn run_cross_step(
    runner: &dyn CrossAgentRunner,
    root: &Path,
    agents: &[AgentType],
    step_prompt: &str,
) -> Result<CrossStepReport, String> {
    let mut report = CrossStepReport::default();

    // 1. Independent detection, in parallel: each prompt is built before any
    // agent answers, so none can carry another's work.
    let prompts: Vec<String> = agents
        .iter()
        .map(|a| format!("{step_prompt}{}", engine::detection_appendix(a)))
        .collect();
    let replies = futures::future::join_all(
        agents
            .iter()
            .zip(prompts)
            .map(|(agent, prompt)| runner.run(agent, prompt)),
    )
    .await;
    let mut detections: Vec<(AgentType, DetectionOutput)> = Vec::new();
    for (agent, reply) in agents.iter().zip(replies) {
        report.count(&reply);
        match reply
            .and_then(|r| engine::parse_fenced::<DetectionOutput>(&r.text, engine::FINDINGS_FENCE))
        {
            Ok(output) => {
                report
                    .checklists
                    .push((agent.clone(), output.checklist.clone()));
                detections.push((agent.clone(), output));
            }
            Err(why) => report.failed_agents.push((agent.clone(), why)),
        }
    }
    if detections.is_empty() {
        return Err(format!(
            "no agent returned a usable detection: {}",
            report
                .failed_agents
                .iter()
                .map(|(a, why)| format!("{}: {why}", engine::agent_label(a)))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    // 2-3. Deterministic filter, then matching.
    let (kept, dropped) = engine::filter_findings(root, &detections);
    report.dropped = dropped;
    let groups = engine::match_findings(kept);
    let by_id: BTreeMap<&str, &FindingGroup> = groups.iter().map(|g| (g.id.as_str(), g)).collect();

    // 4. Cross-check: each challenger receives all its findings in one call.
    let (assigned, _unchecked) = engine::assign_challengers(&groups, agents);
    let mut per_challenger: BTreeMap<String, (AgentType, Vec<&str>)> = BTreeMap::new();
    for (group_id, challenger) in &assigned {
        per_challenger
            .entry(engine::agent_label(challenger))
            .or_insert_with(|| (challenger.clone(), Vec::new()))
            .1
            .push(group_id.as_str());
    }
    let challenge_calls: Vec<(AgentType, Vec<&str>, String)> = per_challenger
        .into_values()
        .map(|(agent, ids)| {
            let findings: Vec<(&str, &engine::RawFinding)> = ids
                .iter()
                .map(|id| (*id, &by_id[id].representative().finding))
                .collect();
            let prompt = engine::challenge_prompt(&agent, &findings);
            (agent, ids, prompt)
        })
        .collect();
    let challenge_replies = futures::future::join_all(
        challenge_calls
            .iter()
            .map(|(agent, _, prompt)| runner.run(agent, prompt.clone())),
    )
    .await;
    let mut challenges: BTreeMap<String, ChallengeResult> = BTreeMap::new();
    for ((_, ids, _), reply) in challenge_calls.iter().zip(challenge_replies) {
        report.count(&reply);
        let parsed = reply
            .and_then(|r| engine::parse_fenced::<VerdictsOutput>(&r.text, engine::VERDICTS_FENCE));
        for id in ids {
            let result = match &parsed {
                Ok(out) => {
                    engine::evaluate_verdict(root, out.verdicts.iter().find(|v| v.id == *id), None)
                }
                Err(why) => engine::evaluate_verdict(root, None, Some(why)),
            };
            challenges.insert((*id).to_string(), result);
        }
    }

    // 5-6. Right of reply, once, only against a proven refutation.
    let mut per_author: BTreeMap<String, (AgentType, Vec<&str>)> = BTreeMap::new();
    for group in &groups {
        if challenges.get(&group.id).is_some_and(engine::needs_reply) {
            let author = &group.representative().author;
            per_author
                .entry(engine::agent_label(author))
                .or_insert_with(|| (author.clone(), Vec::new()))
                .1
                .push(group.id.as_str());
        }
    }
    let reply_calls: Vec<(AgentType, Vec<&str>, String)> = per_author
        .into_values()
        .map(|(agent, ids)| {
            let items: Vec<(&str, &engine::RawFinding, &str, &[engine::Citation])> = ids
                .iter()
                .filter_map(|id| match challenges.get(*id) {
                    Some(ChallengeResult::RefutedProven { argument, proof }) => Some((
                        *id,
                        &by_id[id].representative().finding,
                        argument.as_str(),
                        proof.as_slice(),
                    )),
                    _ => None,
                })
                .collect();
            let prompt = engine::reply_prompt(&agent, &items);
            (agent, ids, prompt)
        })
        .collect();
    let reply_answers = futures::future::join_all(
        reply_calls
            .iter()
            .map(|(agent, _, prompt)| runner.run(agent, prompt.clone())),
    )
    .await;
    let mut replies: BTreeMap<String, ReplyResult> = BTreeMap::new();
    for ((_, ids, _), answer) in reply_calls.iter().zip(reply_answers) {
        report.count(&answer);
        let parsed = answer
            .and_then(|r| engine::parse_fenced::<RepliesOutput>(&r.text, engine::REPLIES_FENCE));
        for id in ids {
            let result = match &parsed {
                Ok(out) => {
                    engine::evaluate_reply(root, out.replies.iter().find(|r| r.id == *id), None)
                }
                Err(why) => engine::evaluate_reply(root, None, Some(why)),
            };
            replies.insert((*id).to_string(), result);
        }
    }

    // 7. Arbitration, per finding.
    report.records = groups
        .iter()
        .map(|group| {
            engine::build_record(
                group,
                agents.len(),
                assigned.get(&group.id),
                challenges.remove(&group.id),
                replies.remove(&group.id),
            )
        })
        .collect();
    Ok(report)
}

pub(crate) fn heading_for(
    target_file: &str,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> &'static str {
    match docs_root.strip(target_file) {
        Some("inconsistencies-security.md") => "Security findings (cross-agent audit)",
        _ => "Findings (cross-agent audit)",
    }
}

/// Writes `content` unless the file already holds it; true when it wrote.
fn write_if_changed(path: &Path, content: &str) -> Result<bool, String> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == content => Ok(false),
        Ok(_) => std::fs::write(path, content)
            .map(|()| true)
            .map_err(|e| format!("write {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::write(path, content)
            .map(|()| true)
            .map_err(|e| format!("write {}: {e}", path.display())),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Rows of the previous index's findings tables (`| ID | Problem | …`).
fn previous_finding_rows(previous: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in previous.lines() {
        let t = line.trim();
        if t.starts_with("| ID | Problem |") {
            in_table = true;
            continue;
        }
        if !t.starts_with('|') {
            in_table = false;
            continue;
        }
        let first = t[1..].split('|').next().unwrap_or("").trim();
        if in_table && crate::core::td_index::is_td_id(first) {
            rows.push((first.to_string(), t.to_string()));
        }
    }
    rows
}

/// What the previous index held that this run must not drop: rows it did
/// not re-detect (an active sheet, or a row without a sheet that a person
/// wrote), and settled sheets it named.
type Carried = (Vec<String>, Vec<(String, String, String)>);

fn carried_from_previous(previous: &str, td_dir: &Path, records: &[CrossFindingRecord]) -> Carried {
    let redetected: std::collections::BTreeSet<&str> =
        records.iter().filter_map(|r| r.td_id.as_deref()).collect();
    let sheet = |id: &str| {
        std::fs::read_to_string(td_dir.join(format!("{id}.md")))
            .ok()
            .map(|c| crate::core::td_index::parse_sheet(id, &c))
    };
    let mut rows: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (id, row) in previous_finding_rows(previous) {
        if redetected.contains(id.as_str()) || !seen.insert(id.clone()) {
            continue;
        }
        if sheet(&id).is_none_or(|s| s.is_active()) {
            rows.push(row);
        }
    }
    let mut settled = Vec::new();
    for id in crate::core::td_review::mentioned_td_ids(previous) {
        if redetected.contains(id.as_str()) {
            continue;
        }
        if let Some(s) = sheet(&id).filter(|s| !s.is_active()) {
            let label = match s.decision {
                Some(crate::core::audit_validation::TdDecision::Rejected) => "rejected",
                _ => "accepted as a decision",
            };
            settled.push((id, s.title, label.to_string()));
        }
    }
    (rows, settled)
}

/// Step 8, on disk: TD ids, human-card keys, the index and the sheets. An
/// existing sheet keeps its body; only its Kronn provenance block changes.
/// The previous index's undetected rows, settled sheets and human-owned
/// sections are kept. Returns whether any file changed.
pub(crate) fn write_outputs(
    root: &Path,
    target_file: &str,
    agents: &[AgentType],
    run_id: &str,
    report: &mut CrossStepReport,
) -> Result<bool, String> {
    let docs_root = crate::core::docs_location::DocsRoot::of(root);
    let td_dir = docs_root.file(root, "tech-debt");
    std::fs::create_dir_all(&td_dir).map_err(|e| format!("create {}: {e}", td_dir.display()))?;
    let yyyymmdd = Utc::now().format("%Y%m%d").to_string();
    engine::assign_td_ids(&mut report.records, &td_dir, &yyyymmdd);
    for record in report.records.iter_mut().filter(|r| r.awaits_human()) {
        record.question_key = Some(engine::question_key(run_id, record));
    }
    let target = root.join(target_file);
    crate::core::fs_guard::assert_contained_no_symlink(root, &target)?;
    let previous = match std::fs::read_to_string(&target) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", target.display())),
    };
    let (carried_rows, settled) = carried_from_previous(&previous, &td_dir, &report.records);
    let mut changed = false;
    for record in report.records.iter().filter(|r| r.has_sheet()) {
        let Some(id) = record.td_id.as_deref() else {
            continue;
        };
        let path = td_dir.join(format!("{id}.md"));
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(existing) => engine::merge_provenance(&existing, record),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => engine::render_td_sheet(record),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        changed |= write_if_changed(&path, &content)?;
    }
    let index = engine::render_index(&IndexInput {
        heading: heading_for(target_file, &docs_root),
        agents,
        records: &report.records,
        dropped: &report.dropped,
        checklists: &report.checklists,
        failed_agents: &report.failed_agents,
        carried_rows: &carried_rows,
        settled: &settled,
    });
    let index = super::anti_hallu_enforce::enforce_human_owned_sections(&previous, &index)
        .map_or(index, |(restored, _)| restored);
    changed |= write_if_changed(&target, &index)?;
    Ok(changed)
}

/// The human cards of the serious disagreements, in their own discussion:
/// one `kronn-question` per dispute, both arguments side by side.
pub(crate) fn dispute_discussion(
    project_id: &str,
    agent: &AgentType,
    language: &str,
    tier: ModelTier,
    report: &CrossStepReport,
) -> Option<(Discussion, DiscussionMessage)> {
    let cards: Vec<String> = report
        .records
        .iter()
        .filter(|r| r.awaits_human())
        .filter_map(engine::dispute_card)
        .collect();
    if cards.is_empty() {
        return None;
    }
    let now = Utc::now();
    let content = format!(
        "Kronn — cross-agent audit (experimental). {} finding(s) were refuted with verified proof and \
         defended with verified proof. Kronn does not decide by majority: please judge each one on the \
         evidence.\n\n{}",
        cards.len(),
        cards.join("\n\n")
    );
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::Agent,
        channel: MessageChannel::Main,
        content,
        agent_type: Some(agent.clone()),
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    let discussion = Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: Uuid::new_v4().to_string(),
        project_id: Some(project_id.to_string()),
        title: "Cross-agent audit — disputes".to_string(),
        agent: agent.clone(),
        language: language.to_string(),
        participants: vec![agent.clone()],
        messages: vec![message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier,
        model: None,
        pin_first_message: true,
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::OnDemand,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    Some((discussion, message))
}

/// Persists the dispute discussion (its cards are ingested with the message)
/// and the step's provenance, in one transaction.
pub(crate) fn persist(
    conn: &rusqlite::Connection,
    run_id: &str,
    step_index: u32,
    file_label: &str,
    agents: &[AgentType],
    report: &CrossStepReport,
    disputes: Option<&(Discussion, DiscussionMessage)>,
) -> anyhow::Result<()> {
    let tx = conn.unchecked_transaction()?;
    if let Some((disc, msg)) = disputes {
        crate::db::discussions::insert_discussion(&tx, disc)?;
        crate::db::discussions::insert_message(&tx, &disc.id, msg)?;
    }
    let agent_names: Vec<String> = agents.iter().map(engine::agent_label).collect();
    let failed: Vec<(String, String)> = report
        .failed_agents
        .iter()
        .map(|(a, why)| (engine::agent_label(a), why.clone()))
        .collect();
    crate::db::audit_cross::record_step(
        &tx,
        &crate::db::audit_cross::CrossStepRow {
            audit_run_id: run_id,
            step_index,
            file_label,
            agents: &agent_names,
            dropped: &report.dropped,
            failed_agents: &failed,
            calls: report.calls,
            step_tokens: report.usage().total(),
            cost_usd_micros: report.cost_usd_micros(),
            dispute_discussion_id: disputes.map(|(d, _)| d.id.as_str()),
        },
        &report.records,
    )?;
    tx.commit()?;
    Ok(())
}

/// One line per verdict for the SSE `cross_audit` event.
pub(crate) fn summary_json(
    report: &CrossStepReport,
    step: usize,
    dispute_discussion: Option<&str>,
) -> serde_json::Value {
    let count = |o: Outcome| report.records.iter().filter(|r| r.outcome == o).count();
    serde_json::json!({
        "step": step,
        "experimental": true,
        "calls": report.calls,
        "dropped": report.dropped.len(),
        "confirmed": count(Outcome::Confirmed),
        "not_refuted": count(Outcome::NotRefuted),
        "contested": count(Outcome::Contested),
        "dismissed": count(Outcome::Dismissed),
        "disputed": count(Outcome::Disputed),
        "not_cross_checked": count(Outcome::NotCrossChecked),
        "kept_settled": report.records.iter().filter(|r| r.settled_as.is_some()).count(),
        "failed_agents": report.failed_agents.iter().map(|(a, _)| engine::agent_label(a)).collect::<Vec<_>>(),
        "dispute_discussion_id": dispute_discussion,
    })
}

// ── Cost estimate before launch ─────────────────────────────────────────────

/// Detection runs the step once per agent; the cross-check and the reply add
/// at most one call per agent each, every one reading code again. Hence a
/// range from N to 2N times the step's single-agent cost.
pub(crate) fn estimate(
    agents: u32,
    base: Option<(String, Option<u64>, Option<u64>)>,
    measured: Option<crate::db::audit_cross::MeasuredStep>,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> CrossAuditEstimate {
    let low = agents.max(engine::MIN_AGENTS as u32);
    let high = low * 2;
    let (base_run_id, base_tokens, base_cost) = match base {
        Some((id, tokens, cost)) => (Some(id), tokens, cost),
        None => (None, None, None),
    };
    let measured_same = measured.as_ref().filter(|m| m.agents == Some(low as usize));
    CrossAuditEstimate {
        agents: low,
        cross_checked_steps: engine::cross_checked_targets(docs_root)
            .map(str::to_string)
            .collect(),
        low_factor: low,
        high_factor: high,
        max_cross_checked: engine::MAX_CROSS_CHECKED as u32,
        base_run_id,
        base_tokens,
        base_cost_usd_micros: base_cost,
        low_tokens: base_tokens.map(|t| t.saturating_mul(low as u64)),
        high_tokens: base_tokens.map(|t| t.saturating_mul(high as u64)),
        low_cost_usd_micros: base_cost.map(|c| c.saturating_mul(low as u64)),
        high_cost_usd_micros: base_cost.map(|c| c.saturating_mul(high as u64)),
        measured_run_id: measured_same.map(|m| m.audit_run_id.clone()),
        measured_tokens: measured_same.and_then(|m| m.step_tokens),
        measured_cost_usd_micros: measured_same.and_then(|m| m.cost_usd_micros),
        estimated: measured_same.is_none(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct CrossEstimateQuery {
    #[serde(default)]
    pub agents: Option<u32>,
}

/// GET /api/projects/{id}/audit/cross-estimate?agents=N
pub async fn cross_audit_estimate(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<CrossEstimateQuery>,
) -> Json<ApiResponse<CrossAuditEstimate>> {
    let agents = query
        .agents
        .unwrap_or(engine::MIN_AGENTS as u32)
        .clamp(engine::MIN_AGENTS as u32, engine::MAX_AGENTS as u32);
    let result = state
        .db
        .with_read_conn(move |conn| {
            // Step labels are the project's relocated targets (KT-1192).
            let docs_root = match crate::db::projects::get_project(conn, &id)? {
                Some(project) => crate::core::docs_location::DocsRoot::of(
                    &crate::core::scanner::resolve_host_path(&project.path),
                ),
                None => crate::core::docs_location::DocsRoot::default_root(),
            };
            let label = engine::cross_checked_targets(&docs_root)
                .next()
                .unwrap_or_default()
                .to_string();
            let base = match crate::db::audit_cross::latest_single_agent_step(conn, &id, &label)? {
                Some(step) => {
                    // The step's figures as KT-1084's run cost reads them:
                    // reported cost first, else Kronn's estimate.
                    let row = crate::db::audit_runs::list_audit_steps(conn, &step.audit_run_id)?
                        .into_iter()
                        .find(|s| s.file_label == label && s.carried_from_run_id.is_none());
                    let cost = row
                        .as_ref()
                        .and_then(|s| s.cost_usd_micros.or(s.estimated_cost_usd_micros));
                    Some((step.audit_run_id, step.step_tokens, cost))
                }
                None => None,
            };
            let measured = crate::db::audit_cross::latest_cross_step(conn, &id, &label)?;
            Ok(estimate(agents, base, measured, &docs_root))
        })
        .await;
    match result {
        Ok(estimate) => Json(ApiResponse::ok(estimate)),
        Err(e) => Json(ApiResponse::err(format!("db: {e}"))),
    }
}

// Its fixtures script a `claude` shell program (`api::other_projects_fixture`).
#[cfg(all(test, unix))]
#[path = "cross_audit_tests.rs"]
mod tests;
