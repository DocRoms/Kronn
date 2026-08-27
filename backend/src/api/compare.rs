use anyhow::Context;
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::compare::{CompareAiVerdictInput, CompareJudgeLabel, NewCompareJudgeRun};
use crate::models::*;
use crate::AppState;

const COMPARE_RUBRIC_VERSION: &str = "compare-quality-v2";
const MAX_SOURCE_CHARS: usize = 32_000;
const MAX_ANSWER_CHARS: usize = 18_000;

fn last_complete_agent_answer(discussion: &Discussion) -> Option<&DiscussionMessage> {
    discussion.messages.iter().rev().find(|message| {
        matches!(message.role, MessageRole::Agent)
            && matches!(message.channel, MessageChannel::Main)
            && !message.recovered_partial
            && !message.content.trim().is_empty()
    })
}

fn first_user_prompt(discussion: &Discussion) -> Option<&str> {
    discussion
        .messages
        .iter()
        .find(|message| {
            matches!(message.role, MessageRole::User)
                && matches!(message.channel, MessageChannel::Main)
        })
        .map(|message| message.content.as_str())
}

fn last_system_message(discussion: &Discussion) -> Option<&str> {
    discussion
        .messages
        .iter()
        .rev()
        .find(|message| {
            matches!(message.role, MessageRole::System)
                && matches!(message.channel, MessageChannel::Main)
                && !message.content.trim().is_empty()
        })
        .map(|message| message.content.as_str())
}

fn blind_label(mut index: usize) -> String {
    let mut value = String::new();
    loop {
        value.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            return value;
        }
        index = index / 26 - 1;
    }
}

#[derive(Debug, Serialize)]
struct JudgeContent {
    content: String,
    truncated: bool,
    original_chars: usize,
    shown_chars: usize,
}

fn clip_for_judge(value: &str, max_chars: usize) -> JudgeContent {
    let original_chars = value.chars().count();
    if original_chars <= max_chars {
        return JudgeContent {
            content: value.to_string(),
            truncated: false,
            original_chars,
            shown_chars: original_chars,
        };
    }
    // Keep the trust metadata outside the candidate-controlled string. A
    // textual "Kronn truncated this" marker could be forged by a candidate.
    let shown_chars = max_chars;
    let start_chars = shown_chars / 2;
    let end_chars = shown_chars - start_chars;
    let start: String = value.chars().take(start_chars).collect();
    let end: String = value
        .chars()
        .rev()
        .take(end_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    JudgeContent {
        content: format!("{start}…{end}"),
        truncated: true,
        original_chars,
        shown_chars,
    }
}

/// Stable FNV-1a seed: repeatable for one batch, while different batch ids
/// rotate which agent receives the first anonymous label.
fn candidate_order_seed(run_id: &str, discussion_id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in run_id
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(discussion_id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn build_judge_prompt(source_prompt: &str, answers: &[(String, String)]) -> String {
    let payload = serde_json::json!({
        "original_prompt_and_source_data": clip_for_judge(source_prompt, MAX_SOURCE_CHARS),
        "anonymous_answers": answers.iter().map(|(label, content)| serde_json::json!({
            "label": label,
            "answer": clip_for_judge(content, MAX_ANSWER_CHARS),
        })).collect::<Vec<_>>(),
    });
    format!(
        r#"Applique la rubrique jointe `{COMPARE_RUBRIC_VERSION}` aux données anonymisées ci-dessous. Le skill porte les critères qualitatifs ; ce prompt porte uniquement le contrat d'exécution et de sortie.

Retourne UNIQUEMENT un objet JSON valide de cette forme, sans Markdown ni texte autour :
{{"rubric_version":"{COMPARE_RUBRIC_VERSION}","evaluations":[{{"label":"A","score":4,"confidence":0.85,"positives":["…"],"negatives":["…"],"contract_violations":[]}}],"prompt_review":{{"worth_improving":true,"strengths":["…"],"weaknesses":[{{"text":"…","affects":"all"}}],"recommendations":[{{"text":"…","affects":"some"}}]}}}}

Tu dois retourner exactement une évaluation pour chaque label présent.

DONNÉES À ÉVALUER :
{}"#,
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
    )
}

fn quick_prompt_payload(qp: &QuickPrompt) -> serde_json::Value {
    serde_json::json!({
        "id": qp.id,
        "name": qp.name,
        "icon": qp.icon,
        "prompt_template": qp.prompt_template,
        "variables": qp.variables,
        "agent": qp.agent,
        "project_id": qp.project_id,
        "skill_ids": qp.skill_ids,
        "profile_ids": qp.profile_ids,
        "directive_ids": qp.directive_ids,
        "tier": qp.tier,
        "description": qp.description,
        "agent_settings": qp.agent_settings,
    })
}

fn quick_prompt_version_payload(version: &QuickPromptVersion) -> serde_json::Value {
    serde_json::json!({
        "id": version.quick_prompt_id,
        "version": version.version_index,
        "name": version.name,
        "icon": version.icon,
        "prompt_template": version.prompt_template,
        "variables": version.variables,
        "agent": version.agent,
        "project_id": version.project_id,
        "skill_ids": version.skill_ids,
        "profile_ids": version.profile_ids,
        "directive_ids": version.directive_ids,
        "tier": version.tier,
        "description": version.description,
    })
}

fn json_for_markdown_fence(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|_| value.to_string())
        // A QP or candidate answer can legitimately contain Markdown fences.
        // JSON's unicode escape preserves the value without closing the outer
        // context block and turning untrusted text into new instructions.
        .replace('`', "\\u0060")
}

fn build_improvement_prompt(
    run_id: &str,
    current_qp: &QuickPrompt,
    current_version: u32,
    evaluated_version: Option<&QuickPromptVersion>,
    discussions: &[Discussion],
    details: &BatchCompareDetails,
) -> String {
    let evaluations = details
        .evaluations
        .iter()
        .map(|evaluation| (evaluation.discussion_id.as_str(), evaluation))
        .collect::<std::collections::HashMap<_, _>>();
    let excerpt_chars = (60_000 / discussions.len().max(1)).clamp(800, 6_000);
    let candidates = discussions
        .iter()
        .map(|discussion| {
            let answer = last_complete_agent_answer(discussion);
            serde_json::json!({
                "discussion_id": discussion.id,
                "agent": discussion.agent,
                "tier": discussion.tier,
                "model": answer.and_then(|message| message.model.as_deref()),
                "tokens_used": answer.and_then(|message| (message.tokens_used > 0).then_some(message.tokens_used)),
                "duration_ms": answer.and_then(|message| message.duration_ms),
                "answer": answer.map(|message| clip_for_judge(&message.content, excerpt_chars)),
                "failure_or_system_context": answer.is_none().then(|| last_system_message(discussion)).flatten(),
                "evaluation": evaluations.get(discussion.id.as_str()),
            })
        })
        .collect::<Vec<_>>();
    let evidence = serde_json::json!({
        "compare_run_id": run_id,
        "evaluated_quick_prompt": evaluated_version.map(quick_prompt_version_payload),
        "current_quick_prompt_version": current_version,
        "judge_prompt_review": details.latest_judge_run.as_ref().and_then(|run| run.prompt_review.as_ref()),
        "candidates": candidates,
        "instructions": {
            "full_discussions": "Use the discussion_id values only if a precise point requires reading the durable full discussion.",
            "causality": "Do not blame the prompt for authentication, rate-limit, provider, API, or tooling failures.",
        },
    });
    format!(
        r#"Améliore le Quick Prompt courant ci-dessous à partir des preuves du run Compare. Le skill `qp-improver` est normatif : audite le QP, propose une version complète et termine avec son protocole de déploiement. Ne modifie aucun fichier.

Le premier bloc JSON est le QP **courant** qui sera mis à jour. Le run a pu évaluer une version plus ancienne : réconcilie les recommandations avec le QP courant et ne réintroduis pas un défaut déjà corrigé.

```json
{}
```

## Preuves Compare versionnées

```json
{}
```"#,
        json_for_markdown_fence(&quick_prompt_payload(current_qp)),
        json_for_markdown_fence(&evidence),
    )
}

#[derive(Debug, Deserialize)]
struct RawJudgeEnvelope {
    rubric_version: String,
    evaluations: Vec<RawJudgeEvaluation>,
    prompt_review: BatchComparePromptReview,
}

#[derive(Debug, Deserialize)]
struct RawJudgeEvaluation {
    label: String,
    score: u8,
    confidence: f64,
    #[serde(default)]
    positives: Vec<String>,
    #[serde(default)]
    negatives: Vec<String>,
    #[serde(default)]
    contract_violations: Vec<String>,
}

struct ParsedJudgeAnswer {
    verdicts: Vec<CompareAiVerdictInput>,
    prompt_review: BatchComparePromptReview,
}

fn json_object_slice(content: &str) -> Option<&str> {
    let trimmed = content.trim();
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return Some(trimmed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    (end >= start).then_some(&trimmed[start..=end])
}

fn parse_judge_answer(
    content: &str,
    labels: &[CompareJudgeLabel],
) -> anyhow::Result<ParsedJudgeAnswer> {
    let json = json_object_slice(content).context("Le juge n'a pas retourné d'objet JSON")?;
    let envelope: RawJudgeEnvelope =
        serde_json::from_str(json).context("Le JSON du juge est invalide")?;
    if envelope.rubric_version != COMPARE_RUBRIC_VERSION {
        anyhow::bail!(
            "Version de rubrique inattendue: {}",
            envelope.rubric_version
        );
    }
    let by_label = labels
        .iter()
        .map(|label| (label.label.as_str(), label.discussion_id.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    if envelope.evaluations.len() != labels.len() {
        anyhow::bail!(
            "Le juge a retourné {} verdicts pour {} réponses",
            envelope.evaluations.len(),
            labels.len()
        );
    }
    let mut seen = std::collections::HashSet::new();
    let verdicts = envelope
        .evaluations
        .into_iter()
        .map(|evaluation| {
            let discussion_id = by_label
                .get(evaluation.label.as_str())
                .context("Le juge a retourné un label inconnu")?;
            if !seen.insert(evaluation.label.clone()) {
                anyhow::bail!("Le juge a retourné deux fois le même label");
            }
            if !(1..=5).contains(&evaluation.score) {
                anyhow::bail!("Une note IA est hors de la plage 1..=5");
            }
            if !evaluation.confidence.is_finite() || !(0.0..=1.0).contains(&evaluation.confidence) {
                anyhow::bail!("Une confiance IA est hors de la plage 0..=1");
            }
            Ok(CompareAiVerdictInput {
                discussion_id: (*discussion_id).to_string(),
                score: evaluation.score,
                confidence: evaluation.confidence,
                positives: evaluation.positives,
                negatives: evaluation.negatives,
                contract_violations: evaluation.contract_violations,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(ParsedJudgeAnswer {
        verdicts,
        prompt_review: envelope.prompt_review,
    })
}

async fn refresh_running_judge(state: &AppState, run_id: &str) -> anyhow::Result<()> {
    let lookup = run_id.to_string();
    let Some(stored) = state
        .db
        .with_conn(move |conn| crate::db::compare::running_judge_run(conn, &lookup))
        .await?
    else {
        return Ok(());
    };
    let Some(discussion_id) = stored.public.judge_discussion_id.clone() else {
        let judge_id = stored.public.id.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::compare::fail_judge_run(
                    conn,
                    &judge_id,
                    "La discussion du juge a disparu",
                )
            })
            .await?;
        return Ok(());
    };
    let discussion_lookup = discussion_id.clone();
    let discussion = state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &discussion_lookup))
        .await?;
    if let Some(answer) = discussion.as_ref().and_then(last_complete_agent_answer) {
        let parsed = match parse_judge_answer(&answer.content, &stored.labels) {
            Ok(parsed) => parsed,
            Err(error) => {
                let judge_id = stored.public.id.clone();
                let message = error.to_string();
                state
                    .db
                    .with_conn(move |conn| {
                        crate::db::compare::fail_judge_run(conn, &judge_id, &message)
                    })
                    .await?;
                return Ok(());
            }
        };
        let judge_id = stored.public.id.clone();
        let tokens = answer.tokens_used;
        let duration = answer.duration_ms;
        let model = answer.model.clone();
        state
            .db
            .with_conn(move |conn| {
                crate::db::compare::finalize_judge_run(
                    conn,
                    &judge_id,
                    &parsed.verdicts,
                    &parsed.prompt_review,
                    tokens,
                    duration,
                    model.as_deref(),
                )
            })
            .await?;
        return Ok(());
    }
    let failure_lookup = discussion_id;
    if let Some(error) = state
        .db
        .with_conn(move |conn| crate::db::compare::judge_dispatch_failure(conn, &failure_lookup))
        .await?
    {
        let judge_id = stored.public.id;
        state
            .db
            .with_conn(move |conn| crate::db::compare::fail_judge_run(conn, &judge_id, &error))
            .await?;
    }
    Ok(())
}

/// POST /api/comparisons — persist a free cross-run discussion selection.
pub async fn create_ad_hoc(
    State(state): State<AppState>,
    Json(request): Json<CreateAdHocCompareRequest>,
) -> Json<ApiResponse<CreateAdHocCompareResponse>> {
    match state
        .db
        .with_conn(move |conn| {
            crate::db::compare::create_ad_hoc_compare_run(conn, &request.discussion_ids)
        })
        .await
    {
        Ok(run_id) => Json(ApiResponse::ok(CreateAdHocCompareResponse { run_id })),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// GET /api/workflow-runs/:run_id/compare-details
pub async fn details(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<BatchCompareDetails>> {
    if let Err(error) = refresh_running_judge(&state, &run_id).await {
        tracing::warn!(run_id = %run_id, "Failed to refresh compare judge: {error}");
    }
    let lookup = run_id.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::compare::compare_details(conn, &lookup))
        .await
    {
        Ok(details) => Json(ApiResponse::ok(details)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// PUT /api/workflow-runs/:run_id/compare-details/:discussion_id/manual
pub async fn update_manual_score(
    State(state): State<AppState>,
    Path((run_id, discussion_id)): Path<(String, String)>,
    Json(request): Json<UpdateBatchCompareManualScoreRequest>,
) -> Json<ApiResponse<BatchCompareDetails>> {
    let write_run = run_id.clone();
    let write_discussion = discussion_id;
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::compare::set_manual_score(conn, &write_run, &write_discussion, request.score)
        })
        .await
    {
        return Json(ApiResponse::err(error.to_string()));
    }
    let lookup = run_id.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::compare::compare_details(conn, &lookup))
        .await
    {
        Ok(details) => Json(ApiResponse::ok(details)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// POST /api/workflow-runs/:run_id/compare-judge
pub async fn start_judge(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<StartBatchCompareJudgeRequest>,
) -> Json<ApiResponse<StartBatchCompareJudgeResponse>> {
    if let Err(error) = refresh_running_judge(&state, &run_id).await {
        return Json(ApiResponse::err(error.to_string()));
    }
    let lookup = run_id.clone();
    let discussions = match state
        .db
        .with_conn(move |conn| crate::db::compare::list_batch_discussions(conn, &lookup))
        .await
    {
        Ok(discussions) => discussions,
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    match crate::db::compare::prompt_compatibility(&discussions) {
        ComparePromptCompatibility::Identical => {}
        ComparePromptCompatibility::Different => {
            return Json(ApiResponse::err(
                "Les prompts sélectionnés sont différents ; le juge IA est désactivé pour éviter une évaluation trompeuse",
            ));
        }
        ComparePromptCompatibility::Missing => {
            return Json(ApiResponse::err(
                "Au moins une discussion n'a pas de prompt utilisateur exploitable ; le juge IA est désactivé",
            ));
        }
    }
    let source_prompt = discussions
        .first()
        .and_then(first_user_prompt)
        .unwrap_or_default();
    let mut candidates = discussions
        .iter()
        .filter_map(|discussion| {
            last_complete_agent_answer(discussion).map(|answer| (discussion, answer))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left, _), (right, _)| {
        candidate_order_seed(&run_id, &left.id)
            .cmp(&candidate_order_seed(&run_id, &right.id))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut labels = Vec::new();
    let mut answers = Vec::new();
    for (discussion, answer) in candidates {
        let label = blind_label(labels.len());
        labels.push(CompareJudgeLabel {
            label: label.clone(),
            discussion_id: discussion.id.clone(),
        });
        answers.push((label, answer.content.clone()));
    }
    if answers.is_empty() {
        return Json(ApiResponse::err("Aucune réponse exploitable à évaluer"));
    }

    let now = Utc::now();
    let judge_run_id = Uuid::new_v4().to_string();
    let judge_discussion_id = Uuid::new_v4().to_string();
    let prompt = build_judge_prompt(source_prompt, &answers);
    let first = discussions.first();
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: Some("Kronn Compare".into()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    let discussion = Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: judge_discussion_id.clone(),
        project_id: first.and_then(|discussion| discussion.project_id.clone()),
        title: "Juge IA — comparaison anonyme".into(),
        agent: request.agent.clone(),
        language: first
            .map(|discussion| discussion.language.clone())
            .unwrap_or_else(|| "fr".into()),
        participants: vec![request.agent],
        messages: vec![message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec!["compare-quality".into()],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: true,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: request.tier,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::Off,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let insert_id = judge_run_id.clone();
    let insert_run = run_id;
    let insert_result = state
        .db
        .with_conn(move |conn| {
            crate::db::compare::insert_judge_run(
                conn,
                NewCompareJudgeRun {
                    id: &insert_id,
                    run_id: &insert_run,
                    discussion: &discussion,
                    message: &message,
                    labels: &labels,
                    rubric_version: COMPARE_RUBRIC_VERSION,
                },
            )
        })
        .await;
    if let Err(error) = insert_result {
        return Json(ApiResponse::err(error.to_string()));
    }
    state.agent_dispatch_notify.notify_one();
    Json(ApiResponse::ok(StartBatchCompareJudgeResponse {
        judge_run_id,
        judge_discussion_id,
        status: "Running".into(),
    }))
}

/// POST /api/workflow-runs/:run_id/compare-improve
pub async fn start_improvement(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<StartBatchCompareImprovementRequest>,
) -> Json<ApiResponse<StartBatchCompareImprovementResponse>> {
    let lookup = run_id.clone();
    let loaded = state
        .db
        .with_conn(move |conn| {
            let discussions = crate::db::compare::list_batch_discussions(conn, &lookup)?;
            match crate::db::compare::prompt_compatibility(&discussions) {
                ComparePromptCompatibility::Identical => {}
                ComparePromptCompatibility::Different => {
                    anyhow::bail!(
                        "Les prompts sélectionnés sont différents ; aucun prompt commun ne peut être amélioré"
                    );
                }
                ComparePromptCompatibility::Missing => {
                    anyhow::bail!(
                        "Au moins une discussion n'a pas de prompt utilisateur exploitable"
                    );
                }
            }
            let (qp_id, evaluated_version_index) =
                crate::db::compare::comparison_quick_prompt_origin(
                    conn,
                    &lookup,
                    &discussions,
                )?
                    .context("Les discussions ne proviennent pas toutes du même Quick Prompt")?;
            let current_qp = crate::db::quick_prompts::get_quick_prompt(conn, &qp_id)?
                .context("Quick Prompt not found")?;
            let current_version = crate::db::quick_prompts::current_version_index(conn, &qp_id)?
                .context("Quick Prompt has no version snapshot")?;
            let evaluated_version = evaluated_version_index.and_then(|wanted| {
                crate::db::quick_prompts::list_quick_prompt_versions(conn, &qp_id)
                    .ok()
                    .and_then(|versions| {
                        versions
                            .into_iter()
                            .find(|version| version.version_index == wanted)
                    })
            });
            let details = crate::db::compare::compare_details(conn, &lookup)?;
            Ok((
                qp_id,
                current_qp,
                current_version,
                evaluated_version,
                discussions,
                details,
            ))
        })
        .await;
    let (qp_id, current_qp, current_version, evaluated_version, discussions, details) = match loaded
    {
        Ok(loaded) => loaded,
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };

    let prompt = build_improvement_prompt(
        &run_id,
        &current_qp,
        current_version,
        evaluated_version.as_ref(),
        &discussions,
        &details,
    );
    let (author_pseudo, author_avatar_email, summary_strategy) = {
        let config = state.config.read().await;
        (
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
            config.server.default_summary_strategy,
        )
    };
    let now = Utc::now();
    let discussion_id = Uuid::new_v4().to_string();
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo,
        author_avatar_email,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    let project_id = current_qp.project_id.clone().or_else(|| {
        discussions
            .first()
            .and_then(|discussion| discussion.project_id.clone())
    });
    let language = discussions
        .first()
        .map(|discussion| discussion.language.clone())
        .unwrap_or_else(|| "fr".into());
    let discussion = Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: discussion_id.clone(),
        project_id,
        title: format!("Améliorer le QP — {}", current_qp.name),
        agent: request.agent.clone(),
        language,
        participants: vec![request.agent],
        messages: vec![message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec!["qp-improver".into()],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: request.tier,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let insert_discussion = discussion.clone();
    let insert_message = message;
    let insert_qp = qp_id;
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::compare::insert_improvement_discussion(
                conn,
                &insert_discussion,
                &insert_message,
                &insert_qp,
                current_version,
            )
        })
        .await
    {
        return Json(ApiResponse::err(error.to_string()));
    }
    state.agent_dispatch_notify.notify_one();
    Json(ApiResponse::ok(StartBatchCompareImprovementResponse {
        discussion_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blind_labels_continue_after_z() {
        assert_eq!(blind_label(0), "A");
        assert_eq!(blind_label(25), "Z");
        assert_eq!(blind_label(26), "AA");
        assert_eq!(blind_label(27), "AB");
    }

    #[test]
    fn judge_answer_requires_every_known_label_once() {
        let labels = vec![
            CompareJudgeLabel {
                label: "A".into(),
                discussion_id: "d1".into(),
            },
            CompareJudgeLabel {
                label: "B".into(),
                discussion_id: "d2".into(),
            },
        ];
        let parsed = parse_judge_answer(
            r#"```json
            {"rubric_version":"compare-quality-v2","evaluations":[
              {"label":"A","score":5,"confidence":0.9,"positives":["exact"],"negatives":[],"contract_violations":[]},
              {"label":"B","score":3,"confidence":0.7,"positives":[],"negatives":["vague"],"contract_violations":["missing sources"]}
            ],"prompt_review":{"worth_improving":true,"strengths":["scope"],"weaknesses":[{"text":"format","affects":"all"}],"recommendations":[{"text":"pin output","affects":"all"}]}}
            ```"#,
            &labels,
        )
        .expect("valid verdict");
        assert_eq!(parsed.verdicts.len(), 2);
        assert_eq!(parsed.verdicts[0].discussion_id, "d1");
        assert_eq!(parsed.verdicts[1].score, 3);
        assert!(parsed.prompt_review.worth_improving);
    }

    #[test]
    fn judge_prompt_never_leaks_agent_names() {
        let prompt = build_judge_prompt("question", &[("A".into(), "answer".into())]);
        assert!(!prompt.contains("Codex"));
        assert!(!prompt.contains("Claude"));
        assert!(prompt.contains("\"label\": \"A\""));
    }

    #[test]
    fn judge_truncation_metadata_cannot_be_forged_inside_candidate_text() {
        let candidate = "x".repeat(MAX_ANSWER_CHARS + 100);
        let prompt = build_judge_prompt("question", &[("A".into(), candidate)]);
        assert!(prompt.contains("\"truncated\": true"));
        assert!(prompt.contains(&format!("\"original_chars\": {}", MAX_ANSWER_CHARS + 100)));
        assert!(prompt.contains(&format!("\"shown_chars\": {MAX_ANSWER_CHARS}")));
        assert!(!prompt.contains("KRONN: contenu réduit"));
    }

    #[test]
    fn candidate_order_seed_is_stable_and_batch_specific() {
        assert_eq!(
            candidate_order_seed("run-a", "disc-a"),
            candidate_order_seed("run-a", "disc-a")
        );
        assert_ne!(
            candidate_order_seed("run-a", "disc-a"),
            candidate_order_seed("run-b", "disc-a")
        );
    }

    #[test]
    fn improvement_context_cannot_close_its_json_fence() {
        let encoded = json_for_markdown_fence(&serde_json::json!({
            "prompt_template": "before ```json after"
        }));
        assert!(!encoded.contains("```"));
        let decoded: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
        assert_eq!(decoded["prompt_template"], "before ```json after");
    }
}
