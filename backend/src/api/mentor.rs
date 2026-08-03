//! Mode Mentor — API surface for guided learning parcours.
//!
//! Increment 2a: typed read/write of a parcours' `mentor_state` (the JSON blob
//! on the disc — see migration 074 + `db::discussions::{get,set}_mentor_state`).
//! Heavier flows (create-with-disc from a ticket/subject, the mentor→censeur
//! turn, hint ladder) land in later increments.
//!
//! See docs/design/mentor-mode.md.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// GET /api/mentor/parcours/{disc_id} — read the typed parcours state.
///
/// `Not a mentor parcours` when the disc has no `mentor_state` (NULL column) —
/// i.e. it's an ordinary discussion.
pub async fn get_parcours(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<MentorState>> {
    let raw = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_mentor_state(conn, &disc_id))
        .await
    {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    match raw {
        Some(json) => match serde_json::from_str::<MentorState>(&json) {
            Ok(state) => Json(ApiResponse::ok(state)),
            Err(e) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("Corrupt mentor_state: {}", e),
            )),
        },
        None => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Not a mentor parcours",
        )),
    }
}

/// DELETE /api/mentor/parcours/{disc_id} — delete a parcours (its discussion +
/// mentor_state). Clears a failed generation or an abandoned parcours. Guards
/// that the disc is actually a mentor parcours before deleting.
pub async fn delete_parcours(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<bool>> {
    let did_check = disc_id.clone();
    let is_parcours = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_mentor_state(conn, &did_check))
        .await
    {
        Ok(v) => v.is_some(),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if !is_parcours {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Not a mentor parcours",
        ));
    }
    let did = disc_id.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::discussions::delete_discussion(conn, &did))
        .await
    {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Discussion not found",
        )),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Load a parcours' state, apply a synchronous mutation, persist, and return the
/// updated state. Shared by the deterministic state-machine endpoints
/// (submit / advance / hint / chapter). The live mentor→censeur turn is
/// front-orchestrated and writes back through these same endpoints.
async fn apply(
    state: &AppState,
    disc_id: &str,
    mutate: impl FnOnce(&mut MentorState) -> Result<(), String> + Send + 'static,
) -> Json<ApiResponse<MentorState>> {
    let did = disc_id.to_string();
    // The whole read→mutate→write runs inside ONE `with_conn` closure, so it holds
    // the single connection mutex for the entire cycle: no other request can slip a
    // write in between the load and the save. Fixes the lost-update race where a
    // background run (hint/turn/bilan) and a concurrent learner action each loaded
    // the same JSON, mutated their copy, and the last writer clobbered the other.
    enum Outcome {
        Ok(Box<MentorState>),
        NotFound,
        Corrupt(String),
        Invalid(String),
        Serialize(String),
    }
    let res = state
        .db
        .with_conn(move |conn| {
            let Some(json) = crate::db::discussions::get_mentor_state(conn, &did)? else {
                return Ok(Outcome::NotFound);
            };
            let mut parcours: MentorState = match serde_json::from_str(&json) {
                Ok(s) => s,
                Err(e) => return Ok(Outcome::Corrupt(e.to_string())),
            };
            if let Err(e) = mutate(&mut parcours) {
                return Ok(Outcome::Invalid(e));
            }
            let out = match serde_json::to_string(&parcours) {
                Ok(j) => j,
                Err(e) => return Ok(Outcome::Serialize(e.to_string())),
            };
            if !crate::db::discussions::set_mentor_state(conn, &did, Some(&out))? {
                return Ok(Outcome::NotFound);
            }
            Ok(Outcome::Ok(Box::new(parcours)))
        })
        .await;
    match res {
        Ok(Outcome::Ok(p)) => Json(ApiResponse::ok(*p)),
        Ok(Outcome::NotFound) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Not a mentor parcours",
        )),
        Ok(Outcome::Corrupt(e)) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Corrupt mentor_state: {}", e),
        )),
        Ok(Outcome::Invalid(e)) => Json(ApiResponse::err_coded(ApiErrorCode::Validation, e)),
        Ok(Outcome::Serialize(e)) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Serialize error: {}", e),
        )),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/mentor/parcours/{disc_id}/submit — store the learner's submission.
pub async fn submit_block(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<SubmitBlockRequest>,
) -> Json<ApiResponse<MentorState>> {
    apply(&state, &disc_id, move |s| s.submit(&req.block, req.content)).await
}

/// POST /api/mentor/parcours/{disc_id}/turn — run a live mentor→censeur→
/// evaluateur turn SERVER-SIDE and fold the censeur-vetted result into the block.
///
/// This is the anti-solution guarantee: unlike the old front-orchestrated turn
/// (which streamed the mentor's RAW answer to the browser and trusted a
/// client-supplied verdict), the workflow runs entirely on the server. Only the
/// vetted reply — kept when the censeur cleared it (`leak == false`), dropped
/// otherwise — is ever persisted or returned. The learner never receives the raw
/// answer nor supplies the verdict, so the guard can't be bypassed from devtools.
///
/// Mirrors `request_hint`: validate-before-mutate, mark a `Pending` turn, return
/// at once, and run the generation in a background task (the front polls until it
/// settles — the turn also survives navigating away).
pub async fn run_turn(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<RunTurnRequest>,
) -> Json<ApiResponse<MentorState>> {
    // The live turn REQUIRES the mentor-turn workflow — no counter-only fallback.
    let wf_id = {
        state
            .config
            .read()
            .await
            .server
            .mentor_turn_workflow_id
            .clone()
    };
    let Some(wf_id) = wf_id else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Tour mentor non configuré",
        ));
    };

    // Load the turn workflow (must exist + be enabled).
    let wf_id_load = wf_id.clone();
    let mut wf = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id_load))
        .await
    {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Turn workflow not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if !wf.enabled {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Turn workflow is disabled",
        ));
    }

    // Anchor the turn on the disc's project (so the mentor reasons over real code).
    let did_disc = disc_id.clone();
    let disc = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did_disc))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    wf.project_id = disc.project_id.clone();

    // Validate-before-mutate: the block must be an unlocked learner block, and we
    // grab the subject BEFORE any write so a validation/insert failure below can't
    // strand a turn in `Pending` (mirrors the hint "validate-before-mutate" rule).
    let did_read = disc_id.clone();
    let raw = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_mentor_state(conn, &did_read))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    let Some(json) = raw else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Not a mentor parcours",
        ));
    };
    // Grab the subject + the prior dialogue on this block BEFORE the current
    // submission is recorded (so `history` = the exchanges that preceded it),
    // and feed both into the turn so the mentor builds on the conversation
    // instead of re-asking. `preview_turn` re-validates the block is open.
    let (subject, history) = match serde_json::from_str::<MentorState>(&json) {
        Ok(p) => match p.preview_turn(&req.block) {
            Ok(()) => (p.objective.clone(), p.recent_dialogue(&req.block, 3)),
            Err(msg) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg)),
        },
        Err(e) => return Json(ApiResponse::err(format!("Corrupt mentor_state: {}", e))),
    };

    // Build + insert the run row (validates launch variables). Done BEFORE the
    // Pending commit so a failure here leaves no dangling `Pending` turn.
    let run =
        match build_turn_run(&state, &wf, &subject, &req.block, &req.submission, &history).await {
            Ok(r) => r,
            Err(msg) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg)),
        };

    // Everything that can fail is done — NOW commit the Pending turn (stores the
    // submission + marks last_turn Pending). begin_turn re-validates the block.
    let apply_block = req.block.clone();
    let apply_sub = req.submission.clone();
    let apply_res = apply(&state, &disc_id, move |s| {
        s.begin_turn(apply_block, apply_sub)
    })
    .await;
    if !apply_res.0.success {
        return apply_res;
    }

    // Fire the generation in the background; the client polls while `Pending`.
    let inputs = TurnInputs {
        subject,
        block: req.block.clone(),
        submission: req.submission.clone(),
        history,
    };
    let bg_state = state.clone();
    let bg_disc = disc_id.clone();
    tokio::spawn(async move {
        run_turn_bg(bg_state, bg_disc, inputs, wf, run).await;
    });

    // Return the Pending state at once (submission stored, last_turn = Pending).
    apply_res
}

/// Build + insert a `Pending` run row for a turn pass (validates launch vars).
/// Shared by the first pass and the leak reformulation.
async fn build_turn_run(
    state: &AppState,
    wf: &Workflow,
    subject: &str,
    block: &MentorPhase,
    submission: &str,
    history: &str,
) -> Result<WorkflowRun, String> {
    let mut vars = std::collections::HashMap::new();
    vars.insert("subject".to_string(), subject.to_string());
    vars.insert("block".to_string(), phase_str(block));
    vars.insert("submission".to_string(), submission.to_string());
    // Prior dialogue on this block so the mentor doesn't repeat itself. Empty on
    // the first turn; when set, the seed prompt renders it under "ÉCHANGES DÉJÀ EUS".
    vars.insert(
        "history".to_string(),
        if history.trim().is_empty() {
            "(aucun — premier échange sur ce bloc)".to_string()
        } else {
            history.to_string()
        },
    );
    crate::api::workflows::validate_launch_variables(&wf.variables, &vars)?;
    let trigger_obj = crate::api::workflows::build_manual_trigger_obj(&vars, Utc::now());
    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };
    let run_row = run.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &run_row))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    Ok(run)
}

/// Inputs threaded through a background turn run (kept as one struct so the
/// helpers stay under clippy's argument-count limit).
struct TurnInputs {
    subject: String,
    block: MentorPhase,
    submission: String,
    history: String,
}

/// Background task: run the turn to completion, resolve it to a censeur-vetted
/// outcome, and fold it into the parcours (dialogue + approval). Superseded
/// completions are dropped by `finish_turn`.
async fn run_turn_bg(
    state: AppState,
    disc_id: String,
    inputs: TurnInputs,
    wf: Workflow,
    mut run: WorkflowRun,
) {
    let outcome = generate_turn(&state, &wf, &mut run, &inputs)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("Mentor turn failed for {}: {}", disc_id, e);
            TurnOutcome::Failed(e)
        });
    let fin_block = inputs.block.clone();
    let fin_sub = inputs.submission.clone();
    let Json(r) = apply(&state, &disc_id, move |s| {
        s.finish_turn(&fin_block, fin_sub, outcome);
        Ok(())
    })
    .await;
    if !r.success {
        // Don't silently strand the learner behind a Pending turn on a write failure.
        tracing::error!(
            "Mentor turn write-back failed for {}: {:?}",
            disc_id,
            r.error
        );
    }
}

/// Run the turn workflow to completion and resolve it to a fail-closed outcome.
/// The mentor answer is kept only when the censeur clears it (`leak == false`);
/// on a leak, one automatic reformulation (questions-only) is attempted. The
/// evaluateur's approval verdict is always the FIRST pass's (it judged the
/// learner's real submission, not the reformulation directive).
async fn generate_turn(
    state: &AppState,
    wf: &Workflow,
    run: &mut WorkflowRun,
    inputs: &TurnInputs,
) -> Result<TurnOutcome, String> {
    let (mentor_text, leak, ready) = run_turn_pass(state, wf, run).await?;
    // Fail-closed: keep the mentor answer only when the censeur cleared it.
    let mut reply = if leak == Some(false) {
        Some(mentor_text)
    } else {
        None
    };

    // One automatic reformulation when the censeur flagged a leak: re-run the
    // mentor, questions-only. Best-effort — a failure just leaves reply = None
    // (filtered notice). `ready` stays the first pass's verdict.
    if leak == Some(true) {
        let directive = format!(
            "Ta réponse précédente à cette soumission a été bloquée par le garde-fou car elle \
             dévoilait tout ou partie de la solution. Reformule ta réaction UNIQUEMENT sous forme \
             de questions ouvertes, sans jamais révéler la solution ni de code.\n\n\
             Soumission de l'apprenti :\n{}",
            inputs.submission
        );
        if let Ok(mut run2) = build_turn_run(
            state,
            wf,
            &inputs.subject,
            &inputs.block,
            &directive,
            &inputs.history,
        )
        .await
        {
            if let Ok((mentor2, leak2, _)) = run_turn_pass(state, wf, &mut run2).await {
                if leak2 == Some(false) {
                    reply = Some(mentor2);
                }
            }
        }
    }

    Ok(TurnOutcome::Done { reply, ready })
}

/// One pass of the turn workflow: execute it and extract the mentor's answer plus
/// the censeur `leak` and evaluateur `ready` verdicts. Both verdicts default to
/// `None` (undeterminable) when their step is missing/unparseable.
async fn run_turn_pass(
    state: &AppState,
    wf: &Workflow,
    run: &mut WorkflowRun,
) -> Result<(String, Option<bool>, Option<bool>), String> {
    let (tokens, agents) = {
        let cfg = state.config.read().await;
        (cfg.tokens.clone(), cfg.agents.clone())
    };
    crate::workflows::runner::execute_run(
        state.clone(),
        wf,
        run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("execution error: {}", e))?;
    if run.status != RunStatus::Success {
        return Err(format!(
            "turn generation did not succeed ({:?})",
            run.status
        ));
    }
    let mentor_text = run
        .step_results
        .iter()
        .find(|r| r.step_name == "mentor")
        .map(|s| s.output.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no mentor reply in the run".to_string())?;
    let leak = run
        .step_results
        .iter()
        .find(|r| r.step_name == "censeur")
        .and_then(|s| parse_leak(&s.output));
    let ready = run
        .step_results
        .iter()
        .find(|r| r.step_name == "evaluateur")
        .and_then(|s| parse_ready(&s.output));
    Ok((mentor_text, leak, ready))
}

/// POST /api/mentor/parcours/{disc_id}/advance — validate a block, unlock the next.
/// When the last block is validated (parcours → `done`), kicks off the mentor's
/// closure synthesis in the background (best-effort; the advance still succeeds).
pub async fn advance_block(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<AdvanceBlockRequest>,
) -> Json<ApiResponse<MentorState>> {
    let res = apply(&state, &disc_id, move |s| s.advance(&req.block, req.force)).await;
    maybe_kick_off_bilan(&state, &disc_id, res).await
}

/// POST /api/mentor/parcours/{disc_id}/chapter — mark an onboarding chapter done
/// (unlocks the next). Out-of-range index → validation error. Onboarding only.
pub async fn complete_chapter(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<CompleteChapterRequest>,
) -> Json<ApiResponse<MentorState>> {
    let res = apply(&state, &disc_id, move |s| {
        s.complete_chapter(req.index as usize, req.answer, req.needs_review)
    })
    .await;
    // Finishing the last chapter completes an onboarding course → recap it too.
    maybe_kick_off_bilan(&state, &disc_id, res).await
}

/// POST /api/mentor/parcours/{disc_id}/resource-read — mark a curated resource
/// read/unread (block ② Resources). Persists the flag so the resources read-gate
/// and progress reflect the learner's real reading state.
pub async fn set_resource_read(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<SetResourceReadRequest>,
) -> Json<ApiResponse<MentorState>> {
    apply(&state, &disc_id, move |s| {
        s.set_resource_read(req.index as usize, req.read)
    })
    .await
}

/// POST /api/mentor/parcours/{disc_id}/hint — request a graded "Coup de pouce".
///
/// Level 2 of the design: the generation runs server-side. This endpoint bumps
/// the hint ladder, marks a `Pending` hint on the parcours, returns at once, and
/// spawns a background task that runs the mentor-hint workflow (mentor nudge +
/// censeur), fail-closed, folding the vetted result back into `last_hint`. The
/// learner can navigate away — the run finishes regardless and the nudge is
/// waiting on return (the front polls while `Pending`).
///
/// Degrades to counter-only (old behaviour) when no hint workflow is configured.
pub async fn request_hint(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(req): Json<RequestHintRequest>,
) -> Json<ApiResponse<MentorState>> {
    // No hint workflow configured → just bump the ladder (counter-only fallback).
    let wf_id = {
        state
            .config
            .read()
            .await
            .server
            .mentor_hint_workflow_id
            .clone()
    };
    let Some(wf_id) = wf_id else {
        return apply(&state, &disc_id, |s| {
            s.hint();
            s.last_hint = None;
            Ok(())
        })
        .await;
    };

    // Load the hint workflow (must exist + be enabled).
    let wf_id_load = wf_id.clone();
    let mut wf = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id_load))
        .await
    {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Hint workflow not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if !wf.enabled {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Hint workflow is disabled",
        ));
    }

    // Anchor the hint on the disc's project (so the mentor reasons over real code).
    let did_disc = disc_id.clone();
    let disc = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did_disc))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    wf.project_id = disc.project_id.clone();

    // Read-only precondition + prospective rung + subject, computed BEFORE any
    // mutation so a validation/insert failure below can't strand a hint in
    // `Pending` forever (B1 — mirrors the bilan "validate-before-mutate" rule).
    let did_read = disc_id.clone();
    let raw = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_mentor_state(conn, &did_read))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    let Some(json) = raw else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Not a mentor parcours",
        ));
    };
    let (level, subject) = match serde_json::from_str::<MentorState>(&json) {
        Ok(p) => match p.preview_hint(&req.block) {
            Ok(l) => (l, p.objective.clone()),
            Err(msg) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg)),
        },
        Err(e) => return Json(ApiResponse::err(format!("Corrupt mentor_state: {}", e))),
    };

    // Launch variables (validated against the workflow's declared vars).
    let mut vars = std::collections::HashMap::new();
    vars.insert("subject".to_string(), subject);
    vars.insert("block".to_string(), phase_str(&req.block));
    vars.insert("hint_level".to_string(), level.to_string());
    vars.insert("submission".to_string(), req.submission.clone());
    if let Err(msg) = crate::api::workflows::validate_launch_variables(&wf.variables, &vars) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg));
    }
    let trigger_obj = crate::api::workflows::build_manual_trigger_obj(&vars, Utc::now());

    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };

    // The run row must exist before `execute_run` records step results against it.
    let run_row = run.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_run(conn, &run_row))
        .await
    {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Everything that can fail is done — NOW commit the Pending state (bump the
    // ladder + mark last_hint Pending). begin_hint re-validates the block.
    let apply_block = req.block.clone();
    let did_apply = disc_id.clone();
    let apply_res = apply(&state, &did_apply, move |s| {
        s.begin_hint(apply_block.clone()).map(|_| ())
    })
    .await;
    if !apply_res.0.success {
        return apply_res;
    }

    // Fire the generation in the background; the client can leave the page.
    let bg_state = state.clone();
    let bg_disc = disc_id.clone();
    let bg_block = req.block.clone();
    tokio::spawn(async move {
        run_hint(bg_state, bg_disc, bg_block, level, wf, run).await;
    });

    // Return the Pending state at once (ladder bumped, last_hint = Pending).
    apply_res
}

/// Serialize a `MentorPhase` to its snake_case workflow-variable string
/// ("comprehension", "plan", …) — the shape the mentor-hint workflow expects.
fn phase_str(p: &MentorPhase) -> String {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Background task: run the mentor-hint workflow to completion, extract the
/// mentor's nudge + the censeur's leak verdict (fail-closed), and fold the
/// outcome into the parcours' `last_hint`. Superseded completions are dropped
/// by `finish_hint`.
async fn run_hint(
    state: AppState,
    disc_id: String,
    block: MentorPhase,
    level: u32,
    wf: Workflow,
    mut run: WorkflowRun,
) {
    let outcome = generate_hint(&state, &wf, &mut run).await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("Mentor hint failed for {}: {}", disc_id, e);
            HintOutcome::Failed(e)
        }
    };
    let _ = apply(&state, &disc_id, move |s| {
        s.finish_hint(&block, level, outcome);
        Ok(())
    })
    .await;
}

/// Run the hint workflow and resolve it to a fail-closed outcome. The nudge is
/// revealed only when the censeur explicitly clears it (`leak == false`); any
/// other verdict (leak, unparseable, missing step) filters it.
async fn generate_hint(
    state: &AppState,
    wf: &Workflow,
    run: &mut WorkflowRun,
) -> Result<HintOutcome, String> {
    let (tokens, agents) = {
        let cfg = state.config.read().await;
        (cfg.tokens.clone(), cfg.agents.clone())
    };
    crate::workflows::runner::execute_run(
        state.clone(),
        wf,
        run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("execution error: {}", e))?;
    if run.status != RunStatus::Success {
        return Err(format!(
            "hint generation did not succeed ({:?})",
            run.status
        ));
    }

    let mentor_text = run
        .step_results
        .iter()
        .find(|r| r.step_name == "mentor")
        .map(|s| s.output.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "no mentor nudge in the run".to_string())?;

    // Fail-closed: reveal only when the censeur cleared it (leak == false).
    let censeur = run.step_results.iter().find(|r| r.step_name == "censeur");
    let leak = censeur.and_then(|s| parse_leak(&s.output));
    Ok(if leak == Some(false) {
        HintOutcome::Ready(mentor_text)
    } else {
        HintOutcome::Filtered
    })
}

/// Parse the censeur step's typed output for its `leak` verdict. Mirrors the
/// front's `parseLeak`: unwrap the `---STEP_OUTPUT---` envelope, read `leak`
/// (or nested `data.leak`). `None` when undeterminable → treated as a leak.
fn parse_leak(output: &str) -> Option<bool> {
    let raw = crate::workflows::template::extract_step_envelope(output)
        .map(|e| e.data_json)
        .unwrap_or_else(|| output.to_string());
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("leak")
        .or_else(|| v.get("data").and_then(|d| d.get("leak")))
        .and_then(|l| l.as_bool())
}

/// Parse the evaluateur step's typed output for its `ready` verdict (the block-
/// approval signal). Same envelope handling as [`parse_leak`]. `None` when
/// undeterminable → the block's approval is left untouched (no false sign-off).
fn parse_ready(output: &str) -> Option<bool> {
    let raw = crate::workflows::template::extract_step_envelope(output)
        .map(|e| e.data_json)
        .unwrap_or_else(|| output.to_string());
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("ready")
        .or_else(|| v.get("data").and_then(|d| d.get("ready")))
        .and_then(|r| r.as_bool())
}

// ── Closure synthesis (Bilan) — mentor recaps what was learned once done ──────

/// POST /api/mentor/parcours/{disc_id}/bilan — (re)generate the closure synthesis
/// on demand (retry after a failure, or force a refresh). Only meaningful once
/// the parcours is `done`; otherwise returns the current state unchanged.
pub async fn regenerate_bilan(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<MentorState>> {
    if state
        .config
        .read()
        .await
        .server
        .mentor_bilan_workflow_id
        .is_none()
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Synthèse de bilan non configurée",
        ));
    }
    match spawn_bilan_synthesis(&state, &disc_id).await {
        Ok(Some(updated)) => Json(ApiResponse::ok(updated)),
        // Nothing to do (not done yet, or a synthesis is already ready/pending) —
        // return the current state so the front stays in sync.
        Ok(None) => get_parcours(State(state), Path(disc_id)).await,
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// If a state-changing call left the parcours `done`, kick off the closure
/// synthesis and return the updated (Pending) state; otherwise pass `res`
/// through untouched. Best-effort: a kickoff error is logged, never surfaced —
/// the completion itself already succeeded.
async fn maybe_kick_off_bilan(
    state: &AppState,
    disc_id: &str,
    res: Json<ApiResponse<MentorState>>,
) -> Json<ApiResponse<MentorState>> {
    let done = res.0.success
        && res
            .0
            .data
            .as_ref()
            .is_some_and(|s| s.status == MentorStatus::Done);
    if !done {
        return res;
    }
    match spawn_bilan_synthesis(state, disc_id).await {
        Ok(Some(updated)) => Json(ApiResponse::ok(updated)),
        Ok(None) => res,
        Err(e) => {
            tracing::warn!("Mentor bilan kickoff failed for {}: {}", disc_id, e);
            res
        }
    }
}

/// Kick off the closure-synthesis workflow for a completed parcours. Guards via
/// `begin_bilan` (Done + not already generating). On success, persists the
/// Pending synthesis, spawns the background run, and returns the updated state.
/// `Ok(None)` = nothing to do (not configured / workflow missing / guard failed).
async fn spawn_bilan_synthesis(
    state: &AppState,
    disc_id: &str,
) -> Result<Option<MentorState>, String> {
    let wf_id = {
        state
            .config
            .read()
            .await
            .server
            .mentor_bilan_workflow_id
            .clone()
    };
    let Some(wf_id) = wf_id else { return Ok(None) };

    let wf_id_load = wf_id.clone();
    let mut wf = match state
        .db
        .with_conn(move |c| crate::db::workflows::get_workflow(c, &wf_id_load))
        .await
    {
        Ok(Some(wf)) if wf.enabled => wf,
        Ok(_) => return Ok(None), // missing or disabled → skip synthesis
        Err(e) => return Err(format!("DB error: {}", e)),
    };

    // Anchor the recap on the disc's project (so the mentor can cite real code).
    let did_disc = disc_id.to_string();
    let disc = match state
        .db
        .with_conn(move |c| crate::db::discussions::get_discussion(c, &did_disc))
        .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Err("Discussion not found".into()),
        Err(e) => return Err(format!("DB error: {}", e)),
    };
    wf.project_id = disc.project_id.clone();

    // Load state; build the launch variables BEFORE mutating so a validation
    // failure can't leave a synthesis stuck in Pending.
    let did = disc_id.to_string();
    let raw = state
        .db
        .with_conn(move |c| crate::db::discussions::get_mentor_state(c, &did))
        .await
        .map_err(|e| format!("DB error: {}", e))?;
    let Some(json) = raw else {
        return Err("Not a mentor parcours".into());
    };
    let mut parcours: MentorState =
        serde_json::from_str(&json).map_err(|e| format!("Corrupt mentor_state: {}", e))?;

    let mut vars = std::collections::HashMap::new();
    vars.insert("subject".to_string(), parcours.objective.clone());
    vars.insert("mode".to_string(), mode_str(&parcours.mode));
    vars.insert("context".to_string(), build_bilan_context(&parcours));
    crate::api::workflows::validate_launch_variables(&wf.variables, &vars)?;

    // Guard + mark Pending. `begin_bilan` returns false (no-op) unless the
    // parcours is Done and not already generating/ready.
    if !parcours.begin_bilan() {
        return Ok(None);
    }
    let out = serde_json::to_string(&parcours).map_err(|e| format!("Serialize error: {}", e))?;
    let did = disc_id.to_string();
    state
        .db
        .with_conn(move |c| crate::db::discussions::set_mentor_state(c, &did, Some(&out)))
        .await
        .map_err(|e| format!("DB error: {}", e))?;

    let trigger_obj = crate::api::workflows::build_manual_trigger_obj(&vars, Utc::now());
    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };
    let run_row = run.clone();
    if let Err(e) = state
        .db
        .with_conn(move |c| crate::db::workflows::insert_run(c, &run_row))
        .await
    {
        return Err(format!("DB error: {}", e));
    }

    let bg_state = state.clone();
    let bg_disc = disc_id.to_string();
    tokio::spawn(async move {
        run_bilan(bg_state, bg_disc, wf, run).await;
    });

    Ok(Some(parcours))
}

/// `MentorMode` as the `mode` workflow variable ("mentor" | "onboarding").
fn mode_str(m: &MentorMode) -> String {
    match m {
        MentorMode::Mentor => "mentor".to_string(),
        MentorMode::Onboarding => "onboarding".to_string(),
    }
}

/// Assemble the parcours recap fed to the synthesis prompt: criteria + resources,
/// then (mentor) the learner's own bilan + plan + code, or (onboarding) the
/// course chapters. Kept plain-text; the prompt turns it into Markdown.
fn build_bilan_context(p: &MentorState) -> String {
    let mut s = String::new();
    if !p.criteria.is_empty() {
        s.push_str("Critères de réussite :\n");
        for c in &p.criteria {
            s.push_str(&format!("- {}\n", c));
        }
        s.push('\n');
    }
    if !p.resources.is_empty() {
        s.push_str("Ressources du parcours :\n");
        for r in &p.resources {
            s.push_str(&format!("- {} — {}\n", r.title, r.url));
        }
        s.push('\n');
    }
    match p.mode {
        MentorMode::Onboarding => {
            s.push_str("Chapitres du cours :\n");
            for (i, ch) in p.chapters.iter().enumerate() {
                s.push_str(&format!("{}. {}\n", i + 1, ch.title));
                if let Some(a) = ch.learner_answer.as_ref().filter(|a| !a.trim().is_empty()) {
                    s.push_str(&format!("   réponse de l'apprenti : {}\n", a));
                }
            }
        }
        MentorMode::Mentor => {
            if let Some(b) = p.bilan.learner.as_ref().filter(|b| !b.trim().is_empty()) {
                s.push_str(&format!("Bilan écrit par l'apprenti :\n{}\n\n", b));
            }
            if let Some(pl) = p.plan.learner.as_ref().filter(|b| !b.trim().is_empty()) {
                s.push_str(&format!("Son plan :\n{}\n\n", pl));
            }
            if let Some(co) = p.code.learner.as_ref().filter(|b| !b.trim().is_empty()) {
                s.push_str(&format!("Son code / sa démarche :\n{}\n\n", co));
            }
        }
    }
    s
}

/// Background task: run the synthesis workflow and fold the recap into the
/// parcours' `bilan_synthesis` (Ready on success, Failed otherwise). No censeur —
/// the work is done, there is no solution left to protect.
async fn run_bilan(state: AppState, disc_id: String, wf: Workflow, mut run: WorkflowRun) {
    let outcome = match generate_bilan_text(&state, &wf, &mut run).await {
        Ok(text) => HintOutcome::Ready(text),
        Err(e) => {
            tracing::error!("Mentor bilan failed for {}: {}", disc_id, e);
            HintOutcome::Failed(e)
        }
    };
    let _ = apply(&state, &disc_id, move |s| {
        s.finish_bilan(outcome);
        Ok(())
    })
    .await;
}

/// Run the synthesis workflow and return its `synthese` step text.
async fn generate_bilan_text(
    state: &AppState,
    wf: &Workflow,
    run: &mut WorkflowRun,
) -> Result<String, String> {
    let (tokens, agents) = {
        let cfg = state.config.read().await;
        (cfg.tokens.clone(), cfg.agents.clone())
    };
    crate::workflows::runner::execute_run(
        state.clone(),
        wf,
        run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("execution error: {}", e))?;
    if run.status != RunStatus::Success {
        return Err(format!("synthèse non aboutie ({:?})", run.status));
    }
    run.step_results
        .iter()
        .find(|r| r.step_name == "synthese")
        .map(|s| s.output.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "aucune synthèse produite".to_string())
}

/// GET /api/mentor/parcours — list every parcours (mentor + onboarding),
/// newest-first, for the Mentor landing page. A corrupt `mentor_state` row is
/// skipped rather than failing the whole list.
pub async fn list_parcours(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<ParcoursSummary>>> {
    let rows = match state
        .db
        .with_conn(crate::db::discussions::list_mentor_parcours)
        .await
    {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let summaries = rows
        .into_iter()
        .filter_map(
            |(disc_id, title, json, updated_at, project_id, project_name)| {
                let s: MentorState = serde_json::from_str(&json).ok()?;
                let (done, total) = s.progress();
                Some(ParcoursSummary {
                    disc_id,
                    title,
                    mode: s.mode,
                    status: s.status,
                    objective: s.objective,
                    source: s.source,
                    progress_done: done,
                    progress_total: total,
                    updated_at,
                    generation_error: s.generation_error,
                    project_id,
                    project_name,
                    topic_id: s.topic_id,
                    level: s.level,
                    kind: s.kind,
                })
            },
        )
        .collect();

    Json(ApiResponse::ok(summaries))
}

/// GET /api/mentor/onboarding-catalog/{project_id} — the onboarding catalogue
/// parsed from the project's `docs/onboarding.md` registry. Returns an empty
/// list (not an error) when the project has no registry yet, so the UI shows
/// an empty catalogue rather than a failure. See `core::onboarding_registry`.
pub async fn onboarding_catalog(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<Vec<OnboardingTopic>>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Project not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let path_str = project.path.clone();
    let topics = tokio::task::spawn_blocking(move || {
        let root = crate::core::scanner::resolve_host_path(&path_str);
        let file = crate::core::scanner::detect_docs_dir(&root).join("onboarding.md");
        match std::fs::read_to_string(&file) {
            Ok(md) => crate::core::onboarding_registry::parse_registry(&md),
            Err(_) => Vec::new(), // no registry yet → empty catalogue
        }
    })
    .await
    .unwrap_or_default();

    Json(ApiResponse::ok(topics))
}

/// Build the discussion + pinned protocol message for a new parcours. Shared by
/// `generate_parcours` (background generation). Binds
/// the disc to the right persona: onboarding = the expository "Prof" (no censor);
/// mentor = the socratic persona + the strict no-solution directive.
async fn build_parcours_disc(
    state: &AppState,
    title: &str,
    project_id: Option<String>,
    objective: &str,
    is_onboarding: bool,
) -> (Discussion, DiscussionMessage) {
    let (language, author_pseudo, author_avatar_email, summary_strategy) = {
        let config = state.config.read().await;
        (
            config.language.clone(),
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
            config.server.default_summary_strategy,
        )
    };

    let (profile_ids, directive_ids) = if is_onboarding {
        (vec!["mentor-prof".to_string()], vec![])
    } else {
        (
            vec!["mentor-socratique".to_string()],
            vec!["mentor-no-solution".to_string()],
        )
    };

    let now = Utc::now();
    let protocol = if is_onboarding {
        format!(
            "Parcours Mode Mentor — posture ONBOARDING (cours explicatif).\n\nSujet : {}\n\nLe formateur explique le sujet pas à pas, avec le vrai code du projet, et ponctue de checkpoints.",
            objective
        )
    } else {
        format!(
            "Parcours Mode Mentor — posture socratique stricte.\n\nSujet : {}\n\nL'apprenti travaille par étapes ; le mentor guide sans jamais donner la solution (voir la directive).",
            objective
        )
    };
    let initial_message = DiscussionMessage {
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::System,
        content: protocol,
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

    let agent = AgentType::ClaudeCode;
    let disc = Discussion {
        id: Uuid::new_v4().to_string(),
        project_id,
        title: title.to_string(),
        agent: agent.clone(),
        language,
        participants: vec![agent.clone()],
        messages: vec![initial_message.clone()],
        message_count: 1,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids,
        directive_ids,
        tier: ModelTier::Default,
        model: None,
        pin_first_message: true,
        archived: false,
        pinned: false,
        awaiting_agent: false,
        workspace_mode: "Direct".to_string(),
        workspace_path: None,
        worktree_branch: None,
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
    (disc, initial_message)
}

/// Typed shape of the parcours generator's `generate` step output.
#[derive(serde::Deserialize)]
struct MentorDraft {
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    criteria: Vec<String>,
    #[serde(default)]
    resources: Vec<MentorResource>,
    #[serde(default)]
    target_archi: Option<String>,
    #[serde(default)]
    target_tests: Option<String>,
}

/// Typed shape of the onboarding course generator's `generate_course` step output.
#[derive(serde::Deserialize)]
struct CourseDraft {
    #[serde(default)]
    objective: Option<String>,
    /// Audience level, prerequisites and reference paths — rendered into the
    /// persisted course's self-sufficient meta header (see onboarding_registry).
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    prerequisites: Option<String>,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default)]
    chapters: Vec<Chapter>,
}

/// What a successful generation run produces, ready to fold into the parcours.
enum GenFill {
    Mentor {
        objective: String,
        criteria: Vec<String>,
        resources: Vec<MentorResource>,
        target_archi: Option<String>,
        target_tests: Option<String>,
    },
    Onboarding {
        objective: String,
        level: Option<String>,
        prerequisites: Option<String>,
        references: Vec<String>,
        chapters: Vec<Chapter>,
    },
}

/// Run the generator workflow to completion and parse its typed output. Returns
/// the content to fold in, or a human-readable error (surfaced as `generation_error`).
async fn generate_and_fill(
    state: &AppState,
    mode: MentorMode,
    wf: &Workflow,
    run: &mut WorkflowRun,
) -> Result<GenFill, String> {
    let (tokens, agents) = {
        let cfg = state.config.read().await;
        (cfg.tokens.clone(), cfg.agents.clone())
    };
    crate::workflows::runner::execute_run(
        state.clone(),
        wf,
        run,
        &tokens,
        &agents,
        None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("execution error: {}", e))?;
    if run.status != RunStatus::Success {
        return Err(format!("generation did not succeed ({:?})", run.status));
    }
    let is_onboarding = matches!(mode, MentorMode::Onboarding);
    let step_name = if is_onboarding {
        "generate_course"
    } else {
        "generate"
    };
    let step = run
        .step_results
        .iter()
        .find(|r| r.step_name == step_name)
        .ok_or_else(|| format!("step '{}' not found in run", step_name))?;
    let env = crate::workflows::template::extract_step_envelope(&step.output)
        .ok_or_else(|| "no typed output in the generator step".to_string())?;
    if is_onboarding {
        let d: CourseDraft = serde_json::from_str(&env.data_json)
            .map_err(|e| format!("could not parse course: {}", e))?;
        if d.chapters.is_empty() {
            return Err("the generator produced no chapters".to_string());
        }
        let mut chapters = d.chapters;
        // #1 — spread the correct-answer position deterministically (kills the
        // LLM's position bias) before this course is stored + persisted.
        crate::core::onboarding_registry::normalize_checkpoint_positions(&mut chapters);
        Ok(GenFill::Onboarding {
            objective: d.objective.unwrap_or_default(),
            level: d.level,
            prerequisites: d.prerequisites,
            references: d.references,
            chapters,
        })
    } else {
        let d: MentorDraft = serde_json::from_str(&env.data_json)
            .map_err(|e| format!("could not parse draft: {}", e))?;
        Ok(GenFill::Mentor {
            objective: d.objective.unwrap_or_default(),
            criteria: d.criteria,
            resources: d.resources,
            target_archi: d.target_archi,
            target_tests: d.target_tests,
        })
    }
}

/// Background task: generate the parcours content, then fold it into the (already
/// persisted) placeholder — flipping `generating → draft` on success, or leaving
/// it `generating` with a `generation_error` on failure.
async fn run_generation(
    state: AppState,
    disc_id: String,
    mode: MentorMode,
    wf: Workflow,
    mut run: WorkflowRun,
) {
    let outcome = generate_and_fill(&state, mode, &wf, &mut run).await;

    // Persist an onboarding course to docs/onboarding/NN-slug.md (best-effort)
    // so the generated chapters become a durable, versioned doc-IA artifact —
    // the same index+folder shape as the tech-debt docs. Borrow before the
    // `outcome` value is moved into the apply closure below.
    if let Ok(GenFill::Onboarding {
        objective,
        level,
        prerequisites,
        references,
        chapters,
    }) = &outcome
    {
        persist_onboarding_course(
            &state,
            &disc_id,
            objective,
            level.as_deref(),
            prerequisites.as_deref(),
            references,
            chapters,
        )
        .await;
    }

    let did_log = disc_id.clone();
    let _ = apply(&state, &disc_id, move |s| {
        match outcome {
            Ok(GenFill::Mentor {
                objective,
                criteria,
                resources,
                target_archi,
                target_tests,
            }) => {
                if !objective.trim().is_empty() {
                    s.objective = objective;
                }
                s.criteria = criteria;
                s.resources = resources;
                s.target_archi = target_archi;
                s.target_tests = target_tests;
                // A freshly generated parcours opens straight to the learner
                // (no draft gate): open + unlock the first block.
                s.open_to_learner();
                s.generation_error = None;
            }
            Ok(GenFill::Onboarding {
                objective,
                chapters,
                ..
            }) => {
                if !objective.trim().is_empty() {
                    s.objective = objective;
                }
                s.chapters = chapters;
                // Onboarding opens straight away.
                s.status = MentorStatus::Open;
                s.generation_error = None;
            }
            Err(e) => {
                tracing::error!("Mentor generation failed for {}: {}", did_log, e);
                s.generation_error = Some(e);
            }
        }
        Ok(())
    })
    .await;
}

/// Persist a freshly generated onboarding course to the project's
/// `docs/onboarding/NN-slug.md` and link it from the registry. Best-effort: a
/// project with no resolvable path, or a write error, is logged and skipped —
/// the parcours still works from `mentor_state`. The course title is the
/// discussion title (falls back to a generic label).
async fn persist_onboarding_course(
    state: &AppState,
    disc_id: &str,
    objective: &str,
    level: Option<&str>,
    prerequisites: Option<&str>,
    references: &[String],
    chapters: &[Chapter],
) {
    let did = disc_id.to_string();
    let disc = match state
        .db
        .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &did))
        .await
    {
        Ok(Some(d)) => d,
        _ => return,
    };
    let Some(project_id) = disc.project_id.clone() else {
        return;
    };
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
        .await
    {
        Ok(Some(p)) => p,
        _ => return,
    };
    let resolved = crate::core::scanner::resolve_host_path(&project.path);
    let docs_dir = crate::core::scanner::detect_docs_dir(&resolved);
    let title = if disc.title.trim().is_empty() {
        "Onboarding"
    } else {
        disc.title.trim()
    };
    match crate::core::onboarding_registry::persist_course(
        &docs_dir,
        title,
        objective,
        level,
        prerequisites,
        references,
        chapters,
    ) {
        Ok(rel) => tracing::info!("Persisted onboarding course → {}", rel),
        Err(e) => tracing::warn!("Failed to persist onboarding course for {}: {}", disc_id, e),
    }
}

/// POST /api/mentor/parcours/generate — kick off background AI generation. Creates
/// a placeholder parcours (status `generating`), returns its disc id at once, and
/// runs the generator workflow in a spawned task that fills the parcours in. The
/// UI can navigate away; the landing list shows it generating → ready.
pub async fn generate_parcours(
    State(state): State<AppState>,
    Json(req): Json<GenerateParcoursRequest>,
) -> Json<ApiResponse<CreateParcoursResponse>> {
    if req.title.trim().is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Title is required",
        ));
    }
    if req.objective.trim().is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Objective is required",
        ));
    }

    let is_onboarding = matches!(req.mode, MentorMode::Onboarding);
    // Pick the right generator workflow (parcours vs onboarding course).
    let wf_id = {
        let config = state.config.read().await;
        if is_onboarding {
            config.server.mentor_course_workflow_id.clone()
        } else {
            config.server.mentor_generator_workflow_id.clone()
        }
    };
    let Some(wf_id) = wf_id else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Generator workflow not configured",
        ));
    };

    let wf_id_load = wf_id.clone();
    let mut wf = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id_load))
        .await
    {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Generator workflow not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };
    if !wf.enabled {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Generator workflow is disabled",
        ));
    }

    // Normalize a blank/whitespace project_id to None up front, so the disc FK,
    // the run anchor and the validation all agree — a " " must never be stored as
    // a bogus project link (it was previously written to the disc unvalidated).
    let project_id = req.project_id.clone().filter(|p| !p.trim().is_empty());

    // Per-run project anchor (onboarding generates from the picked project's code).
    if let Some(pid) = project_id.clone() {
        let pid_check = pid.clone();
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &pid_check))
            .await
        {
            Ok(Some(_)) => wf.project_id = Some(pid),
            Ok(None) => {
                return Json(ApiResponse::err_coded(
                    ApiErrorCode::Validation,
                    "Project not found",
                ))
            }
            Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        }
    }

    // Generator launch variables (validated against the workflow's declared vars).
    let mut vars = std::collections::HashMap::new();
    vars.insert("subject".to_string(), req.subject.clone());
    vars.insert("ticket_key".to_string(), req.ticket_key.clone());
    if let Err(msg) = crate::api::workflows::validate_launch_variables(&wf.variables, &vars) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg));
    }
    let trigger_obj = crate::api::workflows::build_manual_trigger_obj(&vars, Utc::now());

    // Placeholder parcours (empty, status `generating`).
    let (mut disc, initial_message) = build_parcours_disc(
        &state,
        &req.title,
        project_id.clone(),
        &req.objective,
        is_onboarding,
    )
    .await;
    let mut placeholder = if is_onboarding {
        MentorState::new_onboarding(req.source.clone(), req.objective.clone(), vec![])
    } else {
        MentorState::new_draft(req.source.clone(), req.objective.clone(), vec![])
    };
    placeholder.status = MentorStatus::Generating;
    // Carry the source registry topic id (if any) so the catalogue can later
    // match this parcours to its topic (resume instead of a duplicate).
    placeholder.topic_id = req.topic_id.clone().filter(|s| !s.trim().is_empty());
    // Carry the source topic's registry level + curriculum kind so the landing
    // list can badge this parcours without re-reading the registry.
    placeholder.level = req.level.clone().filter(|s| !s.trim().is_empty());
    placeholder.kind = req.kind.clone().filter(|s| !s.trim().is_empty());

    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };
    disc.workflow_run_id = Some(run.id.clone());

    let state_json = match serde_json::to_string(&placeholder) {
        Ok(j) => j,
        Err(e) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("Serialize error: {}", e),
            ))
        }
    };

    // Atomic: persist the disc + message + placeholder state + the pending run.
    let disc_id = disc.id.clone();
    let insert = {
        let disc = disc.clone();
        let msg = initial_message.clone();
        let sj = state_json.clone();
        let did = disc_id.clone();
        let run_row = run.clone();
        state
            .db
            .with_conn(move |conn| {
                // The run must exist first — `discussions.workflow_run_id` has a
                // FK to `workflow_runs(id)` (migration 028).
                crate::db::workflows::insert_run(conn, &run_row)?;
                crate::db::discussions::insert_discussion(conn, &disc)?;
                crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
                crate::db::discussions::set_mentor_state(conn, &did, Some(&sj))?;
                Ok(())
            })
            .await
    };
    if let Err(e) = insert {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Fire the generation in the background; the client can leave the page.
    let bg_state = state.clone();
    let bg_disc = disc_id.clone();
    let mode = req.mode;
    tokio::spawn(async move {
        run_generation(bg_state, bg_disc, mode, wf, run).await;
    });

    Json(ApiResponse::ok(CreateParcoursResponse {
        disc_id,
        state: placeholder,
    }))
}
