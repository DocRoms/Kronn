//! 0.9.0 — Continual Learning HTTP API. The `propose` handler is the validation
//! pipeline (spec §6): secret-guard → Gate-1 existence → kind binding → Gate-2
//! faithfulness (off by default) → anti-generalization → confidence haircut →
//! negative-learning → INSERT pending. `validate` routes the scope + promotes to
//! the dedicated learnings file. Nothing is written to a truth file without a
//! human hitting `validate` (posture B + désagentification).

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::core::faithfulness::{self, FaithfulnessBackend};
use crate::core::learning_gate::{self, EvidenceCheck};
use crate::core::learning_promote::promote_to_file;
use crate::core::learning_scope::{promotion_target, route_scope};
use crate::core::{redact, user_context};
use crate::db::{learnings as db_learnings, projects as db_projects};
use crate::models::learnings::*;
use crate::models::ApiResponse;
use crate::AppState;

const NEGATIVE_LEARNING_THRESHOLD: i64 = 3;

#[derive(Debug, Deserialize, ts_rs::TS)]
#[ts(export, rename = "LearningProposeRequest")]
pub struct ProposeRequest {
    pub claim: String,
    pub evidence: Vec<Evidence>,
    pub kind: LearningKind,
    #[serde(default)]
    pub discussion_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ProposeResult {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub warnings: Vec<String>,
    pub evidence_checks: Vec<EvidenceCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learning: Option<Learning>,
}

fn reject(reason: &str, checks: Vec<EvidenceCheck>) -> Json<ApiResponse<ProposeResult>> {
    Json(ApiResponse::ok(ProposeResult {
        accepted: false,
        reason: Some(reason.to_string()),
        warnings: vec![],
        evidence_checks: checks,
        learning: None,
    }))
}

pub async fn propose_learning(
    State(state): State<AppState>,
    Json(req): Json<ProposeRequest>,
) -> Json<ApiResponse<ProposeResult>> {
    // 0. master toggle — capture is opt-in (beta, default OFF). Validating /
    // rejecting EXISTING pending candidates stays allowed when off (drain, not
    // capture), so only `propose` is gated here.
    if !state.config.read().await.server.continual_learning_enabled {
        return reject("apprentissage continu désactivé (feature beta, OFF par défaut)", vec![]);
    }

    // 1. shape
    if req.claim.trim().is_empty() {
        return reject("claim vide", vec![]);
    }
    if req.evidence.is_empty() {
        return reject("evidence[] obligatoire (au moins 1)", vec![]);
    }
    if req.evidence.iter().any(|e| e.reference.trim().is_empty()) {
        return reject("chaque evidence doit avoir une référence non vide", vec![]);
    }

    // 2. secret guard
    let secret_hit = redact::looks_like_secret(&req.claim)
        || req.evidence.iter().any(|e| {
            redact::looks_like_secret(&e.reference)
                || e.quote.as_deref().is_some_and(redact::looks_like_secret)
        });
    if secret_hit {
        return reject("contenu ressemblant à un secret — refusé", vec![]);
    }

    // scope (for negative-learning hash + later routing)
    let scope = route_scope(req.kind, req.project_id.as_deref());
    let hash = learning_gate::claim_hash(req.kind.as_str(), Some(scope.as_str()), &req.claim);

    // 3. negative-learning + project root resolution (one DB hop)
    let project_id = req.project_id.clone();
    let pre = state
        .db
        .with_conn(move |conn| {
            let rej = db_learnings::rejection_count(conn, &hash).unwrap_or(0);
            let root = project_id
                .as_deref()
                .and_then(|pid| db_projects::get_project(conn, pid).ok().flatten())
                .map(|p| p.path);
            Ok::<_, anyhow::Error>((rej, root))
        })
        .await;
    let (rejections, project_root) = match pre {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    // A `project_id` that doesn't resolve is refused HERE (not silently kept to
    // fail later at validate with "scope=project sans chemin").
    if req.project_id.is_some() && project_root.is_none() {
        return reject("project_id inconnu (projet introuvable)", vec![]);
    }
    if rejections >= NEGATIVE_LEARNING_THRESHOLD {
        return reject(
            &format!("claim déjà rejeté {rejections}× — auto-refus (safeguard #6a)"),
            vec![],
        );
    }

    // 4. Gate-1 — evidence existence
    let roots: Vec<&std::path::Path> =
        project_root.as_deref().map(std::path::Path::new).into_iter().collect();
    let report = learning_gate::verify_evidence(&req.evidence, &roots);
    if report.any_fabricated {
        return reject(
            "Gate-1 : au moins une evidence ne résout pas (fichier/ligne inexistant)",
            report.checks,
        );
    }

    // 5. kind binding — a `fact` should carry at least one Verified evidence
    let mut warnings = Vec::new();
    if req.kind == LearningKind::Fact && report.verified_count == 0 {
        warnings.push(
            "kind=fact sans evidence mécaniquement vérifiée (url/user only) — à confirmer".into(),
        );
    }
    if req.kind == LearningKind::Inference {
        warnings.push("kind=inference — double validation recommandée avant promotion".into());
    }

    // 6. Gate-2 faithfulness — OFF by default (posture B, informative). Reading
    // the backend from config lands in PR4a-bis with the LLM-judge impl; until
    // there's a non-Off backend to select, the default IS off.
    let backend = FaithfulnessBackend::Off;
    let quote = req.evidence.iter().find_map(|e| e.quote.clone()).unwrap_or_default();
    let faithfulness = faithfulness::check(backend, &req.claim, &quote).map(|v| v.verdict);
    if matches!(faithfulness, Some(Faithfulness::Contradiction)) {
        warnings.push("Gate-2 : la source semble CONTREDIRE le claim — à vérifier".into());
    }

    // 7. anti-generalization
    if learning_gate::is_overgeneralized(&req.claim) {
        warnings.push(
            "sur-généralisation (always/never/toujours sans scope) — reformuler avec un périmètre"
                .into(),
        );
    }

    // 8. confidence haircut
    let confidence = learning_gate::haircut(req.confidence);

    // 9. INSERT pending
    let learning = Learning {
        id: uuid::Uuid::new_v4().to_string(),
        claim: req.claim.trim().to_string(),
        evidence: req.evidence.clone(),
        kind: req.kind,
        status: LearningStatus::Pending,
        scope: None, // routed at validation time
        confidence,
        faithfulness,
        discussion_id: req.discussion_id.clone(),
        project_id: req.project_id.clone(),
        source_agent: req.source_agent.clone(),
        promoted_target: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_validated_at: None,
        validated_by: None,
    };
    let to_insert = learning.clone();
    let inserted = state.db.with_conn(move |conn| db_learnings::insert(conn, &to_insert)).await;
    match inserted {
        Ok(()) => Json(ApiResponse::ok(ProposeResult {
            accepted: true,
            reason: None,
            warnings,
            evidence_checks: report.checks,
            learning: Some(learning),
        })),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE") {
                reject("learning déjà proposé (dédup kind+scope+claim)", report.checks)
            } else {
                Json(ApiResponse::err(format!("insert: {msg}")))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub project_id: Option<String>,
}

pub async fn list_learnings(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Json<ApiResponse<Vec<Learning>>> {
    let status = q.status.as_deref().and_then(LearningStatus::from_db);
    let project_id = q.project_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            db_learnings::list(conn, status, project_id.as_deref())
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await
    {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

#[derive(Debug, Serialize)]
pub struct PendingCount {
    pub count: i64,
}

pub async fn pending_count(State(state): State<AppState>) -> Json<ApiResponse<PendingCount>> {
    match state.db.with_conn(|conn| db_learnings::count_pending(conn).map_err(|e| anyhow::anyhow!("{e}"))).await {
        Ok(count) => Json(ApiResponse::ok(PendingCount { count })),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

pub async fn disc_learnings(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<Vec<Learning>>> {
    match state
        .db
        .with_conn(move |conn| db_learnings::disc_pending(conn, &disc_id).map_err(|e| anyhow::anyhow!("{e}")))
        .await
    {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

/// Human validation → route scope → promote to the dedicated learnings file →
/// mark promoted. The ONLY path that writes a truth file.
pub async fn validate_learning(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Learning>> {
    // load + resolve project path in one hop
    let id2 = id.clone();
    let loaded = state
        .db
        .with_conn(move |conn| {
            let l = db_learnings::get(conn, &id2)?;
            let root = l
                .as_ref()
                .and_then(|l| l.project_id.clone())
                .and_then(|pid| db_projects::get_project(conn, &pid).ok().flatten())
                .map(|p| p.path);
            Ok::<_, anyhow::Error>((l, root))
        })
        .await;
    let (learning, project_root) = match loaded {
        Ok((Some(l), root)) => (l, root),
        Ok((None, _)) => return Json(ApiResponse::err("learning introuvable")),
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };

    // P2 — only a PENDING candidate can be promoted (a rejected/stale/already-
    // promoted row must not be writable via a direct API call).
    if learning.status != LearningStatus::Pending {
        return Json(ApiResponse::err(format!(
            "learning non-validable (statut {:?}, attendu pending)",
            learning.status
        )));
    }

    // P0 — re-run Gate-1 against the CURRENT code at validation time (the code
    // may have moved since the proposal): no truth-file write on evidence that
    // doesn't resolve NOW. For a `fact`, require ≥1 mechanically-Verified
    // evidence (the human must fix the evidence or reclassify otherwise).
    let roots: Vec<&std::path::Path> =
        project_root.as_deref().map(std::path::Path::new).into_iter().collect();
    let report = learning_gate::verify_evidence(&learning.evidence, &roots);
    if report.any_fabricated {
        return Json(ApiResponse::err(
            "Gate-1 (revérif) : une evidence ne résout plus (le code a bougé) — promotion refusée",
        ));
    }
    if learning.kind == LearningKind::Fact && report.verified_count == 0 {
        return Json(ApiResponse::err(
            "kind=fact sans evidence mécaniquement vérifiée — corrige l'evidence ou reclasse en preference/inference",
        ));
    }
    // §5 binding — a `preference` (User scope, high blast-radius) must carry a
    // dated user evidence (kind=user + a YYYY-MM-DD date in the ref). Spec
    // invariant #5: user/global scope needs a dated user confirmation.
    if learning.kind == LearningKind::Preference
        && !learning_gate::preference_has_dated_user_evidence(&learning.evidence)
    {
        return Json(ApiResponse::err(
            "kind=preference exige une evidence user contenant une date YYYY-MM-DD",
        ));
    }
    // kind=inference: promoted on a single human validation in 0.9.0 (the
    // human gate IS the validation, posture B). Double-validation across 2
    // sessions is a future safeguard (0.9.x) — NOT enforced here, and the spec
    // no longer promises it for 0.9.0. The modal carries the inference warning.

    let scope = route_scope(learning.kind, learning.project_id.as_deref());
    let target = match scope {
        LearningScope::Project => match project_root.as_deref() {
            Some(root) => std::path::PathBuf::from(root).join(promotion_target(scope)),
            None => return Json(ApiResponse::err("scope=project mais projet sans chemin")),
        },
        LearningScope::User => user_context::user_context_dir().join(promotion_target(scope)),
    };

    // Atomic 2-phase promotion (race-safe vs concurrent validate/reject):
    //   1. CAS-claim the row `pending → promoting` — if lost, abort BEFORE any
    //      write (someone rejected/validated it in between).
    //   2. write the file (serialized + idempotent).
    //   3. finalize `promoting → promoted` on success, else revert to `pending`.
    let id_claim = id.clone();
    let claimed = state
        .db
        .with_conn(move |conn| db_learnings::claim_for_promotion(conn, &id_claim))
        .await;
    match claimed {
        Ok(true) => {}
        Ok(false) => {
            return Json(ApiResponse::err(
                "learning non-validable (course validate/reject, ou plus pending)",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    }

    if let Err(e) = promote_to_file(&target, &learning, &roots) {
        // write failed → revert the claim so a retry can re-claim.
        let id_rev = id.clone();
        let _ = state.db.with_conn(move |conn| db_learnings::revert_promotion(conn, &id_rev)).await;
        return Json(ApiResponse::err(format!("promotion: {e}")));
    }
    let target_str = target.to_string_lossy().to_string();

    let id3 = id.clone();
    let res = state
        .db
        .with_conn(move |conn| {
            db_learnings::finalize_promotion(conn, &id3, scope, Some(&target_str), "human")?;
            db_learnings::get(conn, &id3)
        })
        .await;
    match res {
        Ok(Some(l)) => Json(ApiResponse::ok(l)),
        Ok(None) => Json(ApiResponse::err("learning disparu après validation")),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}

#[derive(Debug, Serialize)]
pub struct SyncDocResult {
    pub outcome: String,
    pub enabled: bool,
}

/// PR4c — sync the `kronn:section name="learnings"` doc pointer for one project
/// against the master toggle: inject (+ seed `docs/learnings.md`) when ON,
/// remove when OFF. Idempotent. Called on toggle change + after an audit.
pub async fn sync_learnings_doc(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<SyncDocResult>> {
    let enabled = state.config.read().await.server.continual_learning_enabled;
    let pid = project_id.clone();
    let path = state
        .db
        .with_conn(move |conn| {
            Ok::<_, anyhow::Error>(db_projects::get_project(conn, &pid)?.map(|p| p.path))
        })
        .await;
    let project_path = match path {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("projet introuvable")),
        Err(e) => return Json(ApiResponse::err(format!("db: {e}"))),
    };
    match crate::core::learning_doc::sync(std::path::Path::new(&project_path), enabled) {
        Ok(outcome) => Json(ApiResponse::ok(SyncDocResult {
            outcome: format!("{outcome:?}"),
            enabled,
        })),
        Err(e) => Json(ApiResponse::err(format!("sync doc: {e}"))),
    }
}

/// Human rejection → mark rejected + bump the negative-learning counter so a
/// repeatedly-rejected claim auto-refuses on the 4th proposal.
pub async fn reject_learning(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    let res = state
        .db
        .with_conn(move |conn| {
            let Some(l) = db_learnings::get(conn, &id)? else {
                return Ok::<Option<bool>, anyhow::Error>(None);
            };
            // P1-2 — recompute the routed scope from (kind, project_id) so the
            // rejection hash matches the basis `propose` checks against (stored
            // `scope` is None until validation).
            let scope = route_scope(l.kind, l.project_id.as_deref());
            let hash = learning_gate::claim_hash(l.kind.as_str(), Some(scope.as_str()), &l.claim);
            // CAS reject (pending only) — can't reject an already-promoted row.
            let rejected = db_learnings::reject(conn, &id)?;
            if rejected {
                db_learnings::record_rejection(conn, &hash, "human reject")?;
            }
            Ok(Some(rejected))
        })
        .await;
    match res {
        Ok(Some(true)) => Json(ApiResponse::ok(())),
        Ok(Some(false)) => {
            Json(ApiResponse::err("learning non-rejetable (statut ≠ pending — déjà promu/rejeté)"))
        }
        Ok(None) => Json(ApiResponse::err("learning introuvable")),
        Err(e) => Json(ApiResponse::err(format!("{e}"))),
    }
}
