use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    AgentType, BatchCompareAiEvaluation, BatchCompareDetails, BatchCompareEvaluation,
    BatchCompareJudgeRun, CompareImprovementAvailability, ComparePromptCompatibility, Discussion,
    DiscussionMessage, MessageRole, ModelTier,
};

use super::parse_dt;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompareJudgeLabel {
    pub label: String,
    pub discussion_id: String,
}

#[derive(Debug, Clone)]
pub struct StoredCompareJudgeRun {
    pub public: BatchCompareJudgeRun,
    pub labels: Vec<CompareJudgeLabel>,
}

#[derive(Debug, Clone)]
pub struct CompareAiVerdictInput {
    pub discussion_id: String,
    pub score: u8,
    pub confidence: f64,
    pub positives: Vec<String>,
    pub negatives: Vec<String>,
    pub contract_violations: Vec<String>,
}

fn decode_agent(value: &str) -> Result<AgentType> {
    serde_json::from_str(value).context("invalid compare judge agent")
}

fn decode_tier(value: &str) -> Result<ModelTier> {
    serde_json::from_str(value).context("invalid compare judge tier")
}

fn decode_strings(value: String) -> Vec<String> {
    serde_json::from_str(&value).unwrap_or_default()
}

fn load_judge_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCompareJudgeRun> {
    let agent_json: String = row.get(2)?;
    let tier_json: String = row.get(3)?;
    let labels_json: String = row.get(6)?;
    let parse_error = |index: usize, error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error.into())
    };
    Ok(StoredCompareJudgeRun {
        public: BatchCompareJudgeRun {
            id: row.get(0)?,
            status: row.get(1)?,
            judge_agent: decode_agent(&agent_json).map_err(|error| parse_error(2, error))?,
            judge_tier: decode_tier(&tier_json).map_err(|error| parse_error(3, error))?,
            self_evaluation: false,
            judge_model: row.get(4)?,
            judge_discussion_id: row.get(5)?,
            rubric_version: row.get(7)?,
            prompt_review: row
                .get::<_, Option<String>>(13)?
                .and_then(|value| serde_json::from_str(&value).ok()),
            error: row.get(8)?,
            tokens_used: row
                .get::<_, Option<i64>>(9)?
                .map(|value| value.max(0) as u64),
            duration_ms: row
                .get::<_, Option<i64>>(10)?
                .map(|value| value.max(0) as u64),
            started_at: parse_dt(row.get(11)?),
            finished_at: row.get::<_, Option<String>>(12)?.map(parse_dt),
        },
        labels: serde_json::from_str(&labels_json).map_err(|error| parse_error(6, error.into()))?,
    })
}

const JUDGE_RUN_COLUMNS: &str = "id, status, judge_agent_json, judge_tier_json, model,
    judge_discussion_id, labels_json, rubric_version, error, tokens_used,
    duration_ms, started_at, finished_at, prompt_review_json";

const MAX_AD_HOC_COMPARE_DISCUSSIONS: usize = 50;

pub fn require_compare_run(conn: &Connection, run_id: &str) -> Result<String> {
    let run_type = conn
        .query_row(
            "SELECT run_type FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match run_type.as_deref() {
        Some("batch" | "compare") => Ok(run_type.unwrap()),
        Some(_) => anyhow::bail!("Run is not a comparison"),
        None => anyhow::bail!("Comparison run not found"),
    }
}

/// Create (or reopen) one durable ad-hoc comparison for an unordered set of
/// discussions. Reusing a scope keeps its human ratings and judge history when
/// the same selection is opened again; visual column order remains client-side.
pub fn create_ad_hoc_compare_run(conn: &Connection, discussion_ids: &[String]) -> Result<String> {
    if !(2..=MAX_AD_HOC_COMPARE_DISCUSSIONS).contains(&discussion_ids.len()) {
        anyhow::bail!(
            "A comparison needs between 2 and {MAX_AD_HOC_COMPARE_DISCUSSIONS} discussions"
        );
    }
    let mut canonical_ids = discussion_ids.to_vec();
    canonical_ids.sort();
    canonical_ids.dedup();
    if canonical_ids.len() != discussion_ids.len() {
        anyhow::bail!("A discussion cannot appear twice in one comparison");
    }
    for discussion_id in &canonical_ids {
        if super::discussions::get_discussion(conn, discussion_id)?.is_none() {
            anyhow::bail!("Discussion not found: {discussion_id}");
        }
    }
    let selection_key = serde_json::to_string(&canonical_ids)?;
    if let Some(existing) = conn
        .query_row(
            "SELECT run_id FROM compare_run_scopes WHERE selection_key = ?1",
            [&selection_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(existing);
    }

    let now = Utc::now().to_rfc3339();
    let run_id = uuid::Uuid::new_v4().to_string();
    let placeholder = super::workflows::ensure_batch_placeholder_workflow(
        conn,
        "__ad_hoc_compare__",
        "Ad-hoc comparisons",
        None,
    )?;
    let transaction = conn.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO workflow_runs
            (id, workflow_id, status, trigger_context, step_results_json, tokens_used,
             started_at, finished_at, run_type, batch_total, batch_completed,
             batch_failed, batch_name)
         VALUES (?1, ?2, 'Success', ?3, '[]', 0, ?4, ?4, 'compare', ?5, ?5, 0, ?6)",
        params![
            run_id,
            placeholder,
            serde_json::to_string(&serde_json::json!({
                "type": "ad_hoc_compare",
                "discussion_ids": discussion_ids,
            }))?,
            now,
            discussion_ids.len() as i64,
            "Comparaison libre",
        ],
    )?;
    transaction.execute(
        "INSERT INTO compare_run_scopes (run_id, selection_key, created_at)
         VALUES (?1, ?2, ?3)",
        params![run_id, selection_key, now],
    )?;
    for (position, discussion_id) in discussion_ids.iter().enumerate() {
        transaction.execute(
            "INSERT INTO compare_run_discussions (run_id, discussion_id, position)
             VALUES (?1, ?2, ?3)",
            params![run_id, discussion_id, position as i64],
        )?;
    }
    transaction.commit()?;
    Ok(run_id)
}

/// Judge discussions are the only discussion runs whose answer must be based
/// exclusively on the captured comparison payload. This durable lookup lets
/// the dispatcher remove project/API tool access without trusting a title or
/// a user-attachable skill id as a security marker.
pub fn is_judge_discussion(conn: &Connection, discussion_id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM batch_compare_judge_runs WHERE judge_discussion_id = ?1
        )",
        [discussion_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn list_batch_discussions(conn: &Connection, run_id: &str) -> Result<Vec<Discussion>> {
    let run_type = require_compare_run(conn, run_id)?;
    let sql = if run_type == "compare" {
        "SELECT discussion_id FROM compare_run_discussions
         WHERE run_id = ?1 ORDER BY position"
    } else {
        "SELECT id FROM discussions WHERE workflow_run_id = ?1 ORDER BY created_at, id"
    };
    let mut stmt = conn.prepare(sql)?;
    let ids = stmt
        .query_map([run_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| {
            super::discussions::get_discussion(conn, &id)?
                .with_context(|| format!("compare child discussion disappeared: {id}"))
        })
        .collect()
}

fn normalized_first_user_prompt(discussion: &Discussion) -> Option<String> {
    discussion
        .messages
        .iter()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.content.replace("\r\n", "\n").trim().to_string())
        .filter(|prompt| !prompt.is_empty())
}

pub fn prompt_compatibility(discussions: &[Discussion]) -> ComparePromptCompatibility {
    let mut prompts = discussions.iter().map(normalized_first_user_prompt);
    let Some(Some(first)) = prompts.next() else {
        return ComparePromptCompatibility::Missing;
    };
    for prompt in prompts {
        match prompt {
            None => return ComparePromptCompatibility::Missing,
            Some(prompt) if prompt != first => return ComparePromptCompatibility::Different,
            Some(_) => {}
        }
    }
    ComparePromptCompatibility::Identical
}

/// A prompt can only be improved when every candidate came from the same QP.
/// The version may differ across runs; in that case the caller still gets the
/// common QP id but no single evaluated snapshot index.
pub fn common_quick_prompt_origin(
    conn: &Connection,
    discussions: &[Discussion],
) -> Result<Option<(String, Option<u32>)>> {
    let mut common_id: Option<String> = None;
    let mut common_version: Option<Option<u32>> = None;
    for discussion in discussions {
        let origin = conn.query_row(
            "SELECT originating_qp_id, originating_qp_version FROM discussions WHERE id = ?1",
            [&discussion.id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        )?;
        let Some(qp_id) = origin.0 else {
            return Ok(None);
        };
        if common_id.as_ref().is_some_and(|current| current != &qp_id) {
            return Ok(None);
        }
        common_id.get_or_insert(qp_id);
        let version = origin.1.map(|value| value.max(0) as u32);
        common_version = Some(match common_version {
            None => version,
            Some(current) if current == version => current,
            Some(_) => None,
        });
    }
    Ok(common_id.map(|id| (id, common_version.flatten())))
}

/// Prefer per-discussion lineage (required for cross-run scopes), then fall
/// back to the immutable batch trigger for legacy batches created before
/// discussion lineage was stamped. This keeps the existing Improve CTA usable
/// across upgrades without letting an ad-hoc mixed-QP selection borrow the
/// first run's provenance.
pub fn comparison_quick_prompt_origin(
    conn: &Connection,
    run_id: &str,
    discussions: &[Discussion],
) -> Result<Option<(String, Option<u32>)>> {
    if let Some(origin) = common_quick_prompt_origin(conn, discussions)? {
        return Ok(Some(origin));
    }
    let run = conn
        .query_row(
            "SELECT run_type, trigger_context FROM workflow_runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((run_type, Some(trigger))) = run else {
        return Ok(None);
    };
    if run_type != "batch" {
        return Ok(None);
    }
    let trigger: serde_json::Value = serde_json::from_str(&trigger)?;
    let Some(qp_id) = trigger
        .get("quick_prompt_id")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let version = trigger
        .get("quick_prompt_version")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    Ok(Some((qp_id.to_string(), version)))
}

pub fn latest_judge_run(conn: &Connection, run_id: &str) -> Result<Option<StoredCompareJudgeRun>> {
    conn.query_row(
        &format!(
            "SELECT {JUDGE_RUN_COLUMNS} FROM batch_compare_judge_runs
             WHERE run_id = ?1 ORDER BY started_at DESC, id DESC LIMIT 1"
        ),
        [run_id],
        load_judge_run_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn running_judge_run(conn: &Connection, run_id: &str) -> Result<Option<StoredCompareJudgeRun>> {
    conn.query_row(
        &format!(
            "SELECT {JUDGE_RUN_COLUMNS} FROM batch_compare_judge_runs
             WHERE run_id = ?1 AND status = 'Running' LIMIT 1"
        ),
        [run_id],
        load_judge_run_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn compare_details(conn: &Connection, run_id: &str) -> Result<BatchCompareDetails> {
    let discussions = list_batch_discussions(conn, run_id)?;
    let prompt_compatibility = prompt_compatibility(&discussions);
    let improvement_availability = match prompt_compatibility {
        ComparePromptCompatibility::Different => CompareImprovementAvailability::DifferentPrompts,
        ComparePromptCompatibility::Missing => CompareImprovementAvailability::MissingPrompt,
        ComparePromptCompatibility::Identical => {
            if comparison_quick_prompt_origin(conn, run_id, &discussions)?.is_some() {
                CompareImprovementAvailability::Available
            } else {
                CompareImprovementAvailability::NoSharedQuickPrompt
            }
        }
    };
    let mut latest = latest_judge_run(conn, run_id)?;
    if let Some(judge) = latest.as_mut() {
        judge.public.self_evaluation = discussions
            .iter()
            .any(|discussion| discussion.agent == judge.public.judge_agent);
    }
    let mut evaluations = Vec::with_capacity(discussions.len());
    for discussion in discussions {
        let row = conn
            .query_row(
                "SELECT manual_score, manual_updated_at, ai_score, ai_confidence,
                        ai_positives_json, ai_negatives_json, ai_violations_json,
                        ai_judge_run_id, ai_updated_at
                 FROM batch_compare_evaluations
                 WHERE run_id = ?1 AND discussion_id = ?2",
                params![run_id, discussion.id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .optional()?;
        let evaluation = if let Some((
            manual_score,
            manual_updated_at,
            ai_score,
            ai_confidence,
            positives,
            negatives,
            violations,
            ai_judge_run_id,
            ai_updated_at,
        )) = row
        {
            let ai = match (ai_score, ai_confidence, ai_judge_run_id, ai_updated_at) {
                (Some(score), Some(confidence), Some(judge_run_id), Some(judged_at)) => {
                    let judge = conn
                        .query_row(
                            &format!(
                                "SELECT {JUDGE_RUN_COLUMNS} FROM batch_compare_judge_runs WHERE id = ?1"
                            ),
                            [&judge_run_id],
                            load_judge_run_row,
                        )
                        .optional()?;
                    judge.map(|judge| BatchCompareAiEvaluation {
                        score: score.clamp(1, 5) as u8,
                        confidence: confidence.clamp(0.0, 1.0),
                        positives: decode_strings(positives),
                        negatives: decode_strings(negatives),
                        contract_violations: decode_strings(violations),
                        judge_run_id,
                        judge_agent: judge.public.judge_agent,
                        judge_tier: judge.public.judge_tier,
                        judge_model: judge.public.judge_model,
                        judge_duration_ms: judge.public.duration_ms,
                        judge_tokens_used: judge.public.tokens_used,
                        rubric_version: judge.public.rubric_version,
                        judged_at: parse_dt(judged_at),
                    })
                }
                _ => None,
            };
            BatchCompareEvaluation {
                discussion_id: discussion.id,
                manual_score: manual_score.map(|score| score.clamp(1, 5) as u8),
                manual_updated_at: manual_updated_at.map(parse_dt),
                ai,
            }
        } else {
            BatchCompareEvaluation {
                discussion_id: discussion.id,
                manual_score: None,
                manual_updated_at: None,
                ai: None,
            }
        };
        evaluations.push(evaluation);
    }
    Ok(BatchCompareDetails {
        run_id: run_id.to_string(),
        prompt_compatibility,
        improvement_availability,
        evaluations,
        latest_judge_run: latest.map(|run| run.public),
    })
}

pub fn set_manual_score(
    conn: &Connection,
    run_id: &str,
    discussion_id: &str,
    score: Option<u8>,
) -> Result<()> {
    let run_type = require_compare_run(conn, run_id)?;
    if score.is_some_and(|value| !(1..=5).contains(&value)) {
        anyhow::bail!("Manual score must be between 1 and 5");
    }
    let belongs = if run_type == "compare" {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM compare_run_discussions
             WHERE discussion_id = ?1 AND run_id = ?2)",
            params![discussion_id, run_id],
            |row| row.get::<_, bool>(0),
        )?
    } else {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1 AND workflow_run_id = ?2)",
            params![discussion_id, run_id],
            |row| row.get::<_, bool>(0),
        )?
    };
    if !belongs {
        anyhow::bail!("Discussion is not part of this batch");
    }
    let updated_at = score.map(|_| Utc::now().to_rfc3339());
    conn.execute(
        "INSERT INTO batch_compare_evaluations
            (run_id, discussion_id, manual_score, manual_updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(run_id, discussion_id) DO UPDATE SET
            manual_score = excluded.manual_score,
            manual_updated_at = excluded.manual_updated_at",
        params![run_id, discussion_id, score.map(i64::from), updated_at],
    )?;
    Ok(())
}

pub struct NewCompareJudgeRun<'a> {
    pub id: &'a str,
    pub run_id: &'a str,
    pub discussion: &'a Discussion,
    pub message: &'a DiscussionMessage,
    pub labels: &'a [CompareJudgeLabel],
    pub rubric_version: &'a str,
}

pub fn insert_improvement_discussion(
    conn: &Connection,
    discussion: &Discussion,
    message: &DiscussionMessage,
    quick_prompt_id: &str,
    quick_prompt_version: u32,
) -> Result<()> {
    let transaction = conn.unchecked_transaction()?;
    super::discussions::insert_discussion(&transaction, discussion)?;
    let trigger_sort_order =
        super::discussions::insert_message(&transaction, &discussion.id, message)?;
    super::discussions::set_originating_qp(
        &transaction,
        &discussion.id,
        quick_prompt_id,
        quick_prompt_version,
    )?;
    let dispatch_id = uuid::Uuid::new_v4().to_string();
    let dedupe_key = format!("compare-improve:{}:{}", quick_prompt_id, discussion.id);
    super::agent_dispatch::enqueue(
        &transaction,
        super::agent_dispatch::NewAgentDispatchJob {
            id: &dispatch_id,
            discussion_id: &discussion.id,
            trigger_message_id: &message.id,
            trigger_sort_order,
            dedupe_key: &dedupe_key,
            agent_override: None,
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )?;
    super::discussions::set_awaiting_agent(&transaction, &discussion.id, true)?;
    transaction.commit()?;
    Ok(())
}

pub fn insert_judge_run(conn: &Connection, input: NewCompareJudgeRun<'_>) -> Result<()> {
    require_compare_run(conn, input.run_id)?;
    let discussions = list_batch_discussions(conn, input.run_id)?;
    if prompt_compatibility(&discussions) != ComparePromptCompatibility::Identical {
        anyhow::bail!(
            "AI judging requires every compared discussion to share the same user prompt"
        );
    }
    if running_judge_run(conn, input.run_id)?.is_some() {
        anyhow::bail!("A judge is already running for this comparison");
    }
    let transaction = conn.unchecked_transaction()?;
    super::discussions::insert_discussion(&transaction, input.discussion)?;
    let trigger_sort_order =
        super::discussions::insert_message(&transaction, &input.discussion.id, input.message)?;
    let dispatch_id = uuid::Uuid::new_v4().to_string();
    let dedupe_key = format!("compare-judge:{}:{}", input.run_id, input.id);
    super::agent_dispatch::enqueue(
        &transaction,
        super::agent_dispatch::NewAgentDispatchJob {
            id: &dispatch_id,
            discussion_id: &input.discussion.id,
            trigger_message_id: &input.message.id,
            trigger_sort_order,
            dedupe_key: &dedupe_key,
            agent_override: None,
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )?;
    super::discussions::set_awaiting_agent(&transaction, &input.discussion.id, true)?;
    transaction.execute(
        "INSERT INTO batch_compare_judge_runs
            (id, run_id, judge_discussion_id, judge_agent_json, judge_tier_json,
             rubric_version, labels_json, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Running', ?8)",
        params![
            input.id,
            input.run_id,
            input.discussion.id,
            serde_json::to_string(&input.discussion.agent)?,
            serde_json::to_string(&input.discussion.tier)?,
            input.rubric_version,
            serde_json::to_string(input.labels)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn finalize_judge_run(
    conn: &Connection,
    judge_run_id: &str,
    verdicts: &[CompareAiVerdictInput],
    prompt_review: &crate::models::BatchComparePromptReview,
    tokens_used: u64,
    duration_ms: Option<u64>,
    model: Option<&str>,
) -> Result<()> {
    let stored = conn
        .query_row(
            &format!("SELECT {JUDGE_RUN_COLUMNS} FROM batch_compare_judge_runs WHERE id = ?1"),
            [judge_run_id],
            load_judge_run_row,
        )
        .optional()?
        .context("Compare judge run not found")?;
    let run_id = conn.query_row(
        "SELECT run_id FROM batch_compare_judge_runs WHERE id = ?1",
        [judge_run_id],
        |row| row.get::<_, String>(0),
    )?;
    let now = Utc::now().to_rfc3339();
    let transaction = conn.unchecked_transaction()?;
    for verdict in verdicts {
        transaction.execute(
            "INSERT INTO batch_compare_evaluations
                (run_id, discussion_id, ai_score, ai_confidence,
                 ai_positives_json, ai_negatives_json, ai_violations_json,
                 ai_judge_run_id, ai_updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(run_id, discussion_id) DO UPDATE SET
                ai_score = excluded.ai_score,
                ai_confidence = excluded.ai_confidence,
                ai_positives_json = excluded.ai_positives_json,
                ai_negatives_json = excluded.ai_negatives_json,
                ai_violations_json = excluded.ai_violations_json,
                ai_judge_run_id = excluded.ai_judge_run_id,
                ai_updated_at = excluded.ai_updated_at",
            params![
                run_id,
                verdict.discussion_id,
                i64::from(verdict.score),
                verdict.confidence,
                serde_json::to_string(&verdict.positives)?,
                serde_json::to_string(&verdict.negatives)?,
                serde_json::to_string(&verdict.contract_violations)?,
                judge_run_id,
                now,
            ],
        )?;
    }
    transaction.execute(
        "UPDATE batch_compare_judge_runs SET
            status = 'Completed', error = NULL, tokens_used = ?2,
            duration_ms = ?3, model = ?4, finished_at = ?5,
            prompt_review_json = ?6
         WHERE id = ?1",
        params![
            judge_run_id,
            tokens_used as i64,
            duration_ms.map(|value| value as i64),
            model,
            now,
            serde_json::to_string(prompt_review)?,
        ],
    )?;
    transaction.commit()?;
    let _ = stored;
    Ok(())
}

pub fn fail_judge_run(conn: &Connection, judge_run_id: &str, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE batch_compare_judge_runs SET status = 'Failed', error = ?2,
                finished_at = ?3 WHERE id = ?1 AND status = 'Running'",
        params![judge_run_id, error, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn judge_dispatch_failure(conn: &Connection, discussion_id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT COALESCE(last_error, status) FROM agent_dispatch_jobs
         WHERE discussion_id = ?1 AND status IN ('Failed', 'Cancelled')
         ORDER BY updated_at DESC LIMIT 1",
        [discussion_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(Into::into)
}
