//! KT-1192 — the briefing as `kronn-question` cards, for an audit launched
//! through MCP on a project without a briefing. The agent posts them in its
//! discussion; once every card is answered there, the answers are saved
//! through the same validation as the form.

use crate::models::*;

use super::briefing::{BriefingDefaults, BriefingFormRequest};

/// Every briefing card carries `audit-briefing:<project>:<discussion>`: the
/// answers are accepted only in that discussion, for that project.
pub(crate) const TASK_REF_PREFIX: &str = "audit-briefing:";

pub(crate) fn task_ref(project_id: &str, discussion_id: &str) -> String {
    format!("{TASK_REF_PREFIX}{project_id}:{discussion_id}")
}

/// `(project, discussion)` of a briefing card's task reference.
pub(crate) fn parse_task_ref(task_ref: &str) -> Option<(&str, &str)> {
    task_ref.strip_prefix(TASK_REF_PREFIX)?.split_once(':')
}

const FREE_FIELDS: [&str; 5] = ["purpose", "team", "maturity", "dependencies", "traps"];
const NOT_PROVIDED: &str = "not_provided";
const DOCS_DIR_DEFAULT: &str = "default";

struct Wording {
    free: [&'static str; 5],
    additional: &'static str,
    not_provided: &'static str,
    nothing_more: &'static str,
    free_context: &'static str,
    docs_dir: &'static str,
    docs_dir_context: &'static str,
    docs_dir_exists: &'static str,
    unknown: &'static str,
    facts: [&'static str; 8],
}

fn wording(language: &str) -> Wording {
    match language {
        "en" => Wording {
            free: [
                "What is this project for, and who uses it?",
                "Who works on it (team, roles)?",
                "How mature is it (prototype, production, legacy…)?",
                "Which external services or repositories does it depend on?",
                "Which areas are fragile or known traps?",
            ],
            additional: "Anything else the audit should know?",
            not_provided: "Not provided",
            nothing_more: "Nothing to add",
            free_context: "Answer in your own words, or pick « Not provided ».",
            docs_dir: "AI documentation folder",
            docs_dir_context: "Relative to the project root. Pick the proposal or answer with another folder.",
            docs_dir_exists: "The folder {0} used by Kronn already exists.",
            unknown: "I don't know",
            facts: [
                "Issue tracker",
                "Code forge",
                "Default branch",
                "Commit convention",
                "Signed-off commits required?",
                "Merge strategy",
                "Review policy",
                "Release cadence",
            ],
        },
        "es" => Wording {
            free: [
                "¿Para qué sirve este proyecto y quién lo usa?",
                "¿Quién trabaja en él (equipo, roles)?",
                "¿Qué madurez tiene (prototipo, producción, legado…)?",
                "¿De qué servicios o repositorios externos depende?",
                "¿Qué zonas son frágiles o trampas conocidas?",
            ],
            additional: "¿Algo más que la auditoría deba saber?",
            not_provided: "No indicado",
            nothing_more: "Nada que añadir",
            free_context: "Responde con tus palabras o elige « No indicado ».",
            docs_dir: "Carpeta de la documentación IA",
            docs_dir_context: "Relativa a la raíz del proyecto. Elige la propuesta o responde con otra carpeta.",
            docs_dir_exists: "La carpeta {0} usada por Kronn ya existe.",
            unknown: "No lo sé",
            facts: [
                "Gestor de tickets",
                "Forja de código",
                "Rama por defecto",
                "Convención de commits",
                "¿Commits firmados (Signed-off-by) obligatorios?",
                "Estrategia de merge",
                "Política de revisión",
                "Ritmo de publicación",
            ],
        },
        _ => Wording {
            free: [
                "À quoi sert ce projet, et pour qui ?",
                "Qui y travaille (équipe, rôles) ?",
                "Quelle maturité (prototype, production, legacy…) ?",
                "De quels services ou dépôts externes dépend-il ?",
                "Quelles zones sont fragiles ou des pièges connus ?",
            ],
            additional: "Autre chose que l'audit doit savoir ?",
            not_provided: "Non renseigné",
            nothing_more: "Rien à ajouter",
            free_context: "Réponds avec tes mots, ou choisis « Non renseigné ».",
            docs_dir: "Dossier de la documentation IA",
            docs_dir_context: "Relatif à la racine du projet. Choisis la proposition ou réponds avec un autre dossier.",
            docs_dir_exists: "Le dossier {0} utilisé par Kronn existe déjà.",
            unknown: "Je ne sais pas",
            facts: [
                "Gestionnaire de tickets",
                "Forge de code",
                "Branche par défaut",
                "Convention de commit",
                "Commits signés (Signed-off-by) obligatoires ?",
                "Stratégie de merge",
                "Politique de revue",
                "Rythme de livraison",
            ],
        },
    }
}

/// The closed project facts (KT-1173): card id and the option ids the
/// backend enums accept, `unknown` last.
const FACTS: [(&str, &[(&str, &str)]); 8] = [
    (
        "tracker",
        &[
            ("jira", "Jira"),
            ("github_issues", "GitHub Issues"),
            ("gitlab_issues", "GitLab Issues"),
            ("linear", "Linear"),
            ("other", "Other"),
            ("none", "None"),
        ],
    ),
    (
        "forge",
        &[
            ("github", "GitHub"),
            ("gitlab", "GitLab"),
            ("bitbucket", "Bitbucket"),
            ("azure_devops", "Azure DevOps"),
            ("other", "Other"),
        ],
    ),
    (
        "default_branch",
        &[
            ("main", "main"),
            ("master", "master"),
            ("develop", "develop"),
            ("trunk", "trunk"),
        ],
    ),
    (
        "commit_convention",
        &[
            ("conventional", "Conventional Commits"),
            ("ticket_prefixed", "Ticket prefix"),
            ("free", "Free form"),
        ],
    ),
    ("signoff_required", &[("yes", "Yes"), ("no", "No")]),
    (
        "merge_strategy",
        &[
            ("squash", "Squash"),
            ("rebase", "Rebase"),
            ("merge_commit", "Merge commit"),
        ],
    ),
    (
        "review_policy",
        &[
            ("no_review", "No review"),
            ("one_approval", "One approval"),
            ("two_approvals", "Two approvals"),
        ],
    ),
    (
        "release_cadence",
        &[
            ("continuous", "Continuous"),
            ("scheduled", "Scheduled"),
            ("on_demand", "On demand"),
        ],
    ),
];

fn option(id: &str, label: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "label": label, "description": null })
}

fn card(
    key: String,
    question: &str,
    context: Option<String>,
    options: Vec<serde_json::Value>,
    recommended: &str,
    task_ref: &str,
) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "key": key,
        "question": question,
        "context": context,
        "options": options,
        "multiple": false,
        "recommended_option_ids": [recommended],
        "task_ref": task_ref,
    })
}

/// The briefing as ready-to-post cards, in `language`.
pub(crate) fn briefing_cards(
    project_id: &str,
    discussion_id: &str,
    language: &str,
    defaults: &BriefingDefaults,
) -> Vec<serde_json::Value> {
    let w = wording(language);
    let task_ref = task_ref(project_id, discussion_id);
    let mut cards = Vec::new();
    for (field, question) in FREE_FIELDS.iter().zip(w.free) {
        cards.push(card(
            format!("briefing-{field}"),
            question,
            Some(w.free_context.to_string()),
            vec![option(NOT_PROVIDED, w.not_provided)],
            NOT_PROVIDED,
            &task_ref,
        ));
    }
    cards.push(card(
        "briefing-additional".into(),
        w.additional,
        Some(w.free_context.to_string()),
        vec![option(NOT_PROVIDED, w.nothing_more)],
        NOT_PROVIDED,
        &task_ref,
    ));
    for ((field, choices), question) in FACTS.iter().zip(w.facts) {
        let mut options: Vec<_> = choices
            .iter()
            .map(|(id, label)| option(id, label))
            .collect();
        options.push(option("unknown", w.unknown));
        cards.push(card(
            format!("briefing-{field}"),
            question,
            None,
            options,
            "unknown",
            &task_ref,
        ));
    }
    let folder = format!("{}/", defaults.docs_dir);
    let mut context = w.docs_dir_context.to_string();
    if defaults.folder_exists {
        context = format!("{} {context}", w.docs_dir_exists.replace("{0}", &folder));
    }
    cards.push(card(
        "briefing-docs-dir".into(),
        w.docs_dir,
        Some(context),
        vec![option(DOCS_DIR_DEFAULT, &folder)],
        DOCS_DIR_DEFAULT,
        &task_ref,
    ));
    cards
}

/// One answered card: the option picked and the free text, if any.
pub(crate) struct CardAnswer<'a> {
    pub key: &'a str,
    pub option: Option<&'a str>,
    pub text: Option<&'a str>,
}

fn fact<T: serde::de::DeserializeOwned + Default>(answer: Option<&CardAnswer<'_>>) -> T {
    answer
        .and_then(|a| a.option)
        .and_then(|id| serde_json::from_value(serde_json::Value::String(id.to_string())).ok())
        .unwrap_or_default()
}

/// The briefing the answered cards describe. `None` while a card is missing.
pub(crate) fn answers_to_form(
    answers: &[CardAnswer<'_>],
    default_docs_dir: &str,
) -> Option<BriefingFormRequest> {
    let find = |field: &str| {
        answers
            .iter()
            .find(|a| a.key == format!("briefing-{field}"))
    };
    let free = |field: &str| -> Option<String> {
        let answer = find(field)?;
        Some(match answer.text.map(str::trim).filter(|t| !t.is_empty()) {
            Some(text) => text.to_string(),
            None => "Not provided".to_string(),
        })
    };
    let docs = find("docs-dir")?;
    let docs_dir = docs
        .text
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_docs_dir.trim_end_matches('/').to_string());
    let additional = find("additional")?
        .text
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    for (field, _) in FACTS {
        find(field)?;
    }
    Some(BriefingFormRequest {
        purpose: free("purpose")?,
        team: free("team")?,
        maturity: free("maturity")?,
        dependencies: free("dependencies")?,
        traps: free("traps")?,
        additional,
        project: Some(BriefingProjectAnswers {
            tracker: fact(find("tracker")),
            forge: fact(find("forge")),
            default_branch: fact(find("default_branch")),
            commit_convention: fact(find("commit_convention")),
            signoff_required: fact(find("signoff_required")),
            merge_strategy: fact(find("merge_strategy")),
            review_policy: fact(find("review_policy")),
            release_cadence: fact(find("release_cadence")),
            ..BriefingProjectAnswers::default()
        }),
        td_to_tasks: false,
        docs_dir: Some(docs_dir),
        ..BriefingFormRequest::default()
    })
}

// ─── The gate ───────────────────────────────────────────────────────────────

/// Why an audit was refused for want of a briefing, as the SSE `error`
/// payload: a typed code, what to do, and the cards when a discussion can
/// carry them.
fn refusal(cards: Option<(String, Vec<serde_json::Value>)>) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "error": "This project has no briefing: fill it in first (the briefing form, \
                  or one click on « Utiliser les valeurs par défaut »), then launch the audit.",
        "error_code": ApiErrorCode::BriefingRequired.as_str(),
    });
    if let Some((discussion_id, cards)) = cards {
        payload["error"] = serde_json::json!(
            "This project has no briefing. Post each card of `briefing_cards` in this \
             discussion as a ```kronn-question fence; once the human has answered them all, \
             Kronn saves the briefing and the audit can be launched again."
        );
        payload["discussion_id"] = serde_json::json!(discussion_id);
        payload["briefing_cards"] = serde_json::json!(cards);
    }
    payload
}

/// The discussion a bridge launch can post the cards in, with its language:
/// one of its own discussions on this project.
async fn caller_discussion(
    state: &crate::AppState,
    bridge: &crate::core::bridge_token::BridgeCaller,
    project_id: &str,
) -> Option<(String, String)> {
    let own = bridge.own_discussions.clone();
    let project_id = project_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            for id in own {
                if let Some((project, language)) = discussion_project(conn, &id)? {
                    if project.as_deref() == Some(project_id.as_str()) {
                        return Ok(Some((id, language)));
                    }
                }
            }
            Ok(None)
        })
        .await
        .ok()
        .flatten()
}

/// `(project_id, language)` of a discussion, without its messages.
fn discussion_project(
    conn: &rusqlite::Connection,
    id: &str,
) -> anyhow::Result<Option<(Option<String>, String)>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT project_id, language FROM discussions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub(crate) enum Gate {
    Proceed,
    Refuse(serde_json::Value),
}

/// No audit starts without a briefing (KT-1192). A UI launch is refused (the
/// form opens); an MCP launch gets the briefing as cards for its discussion;
/// an unattended one (a workflow run, no discussion) gets the default answers,
/// flagged on the project card.
pub(crate) async fn briefing_gate(
    state: &crate::AppState,
    project_id: &str,
    bridge: Option<&crate::core::bridge_token::BridgeCaller>,
) -> Gate {
    let lookup = project_id.to_string();
    let project = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
        .await
        .ok()
        .flatten();
    let Some(project) = project else {
        // The audit handler reports the missing project itself.
        return Gate::Proceed;
    };
    let root = crate::core::scanner::resolve_host_path(&project.path);
    if crate::api::projects::resolve_briefing_notes(&root, &project.briefing_notes).is_some() {
        return Gate::Proceed;
    }
    let Some(bridge) = bridge else {
        return Gate::Refuse(refusal(None));
    };
    if let Some((discussion_id, language)) = caller_discussion(state, bridge, project_id).await {
        let defaults = briefing_defaults_for(state, &project).await;
        let cards = briefing_cards(project_id, &discussion_id, &language, &defaults);
        return Gate::Refuse(refusal(Some((discussion_id, cards))));
    }
    if bridge.own_run.is_some() {
        return match super::briefing::save_briefing_with(
            state,
            project_id,
            super::briefing::default_answers(),
            true,
        )
        .await
        {
            Ok(()) => Gate::Proceed,
            Err(error) => Gate::Refuse(serde_json::json!({
                "error": format!("No briefing, and the default one could not be written: {}", error.message()),
                "error_code": ApiErrorCode::BriefingRequired.as_str(),
            })),
        };
    }
    Gate::Refuse(refusal(None))
}

async fn briefing_defaults_for(state: &crate::AppState, project: &Project) -> BriefingDefaults {
    let id = project.id.clone();
    let stored = state
        .db
        .with_read_conn(move |conn| crate::db::projects::project_docs_dir(conn, &id))
        .await
        .ok()
        .flatten();
    let global = state.config.read().await.scan.default_docs_dir.clone();
    let root = crate::core::scanner::resolve_host_path(&project.path);
    super::briefing::briefing_defaults(&root, stored.as_deref(), global.as_deref())
}

fn sse_refusal(payload: serde_json::Value) -> axum::response::sse::Sse<super::SseStream> {
    let stream: super::SseStream = Box::pin(futures::stream::once(async move {
        Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default()
                .event("error")
                .data(payload.to_string()),
        )
    }));
    axum::response::sse::Sse::new(stream)
}

type Bridge = Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>;

/// POST /api/projects/:id/full-audit — the briefing gate, then the audit.
pub async fn full_audit_route(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    bridge: Bridge,
    axum::Json(req): axum::Json<LaunchAuditRequest>,
) -> axum::response::sse::Sse<super::SseStream> {
    if super::agent_can_audit(&req.agent) {
        if let Gate::Refuse(payload) = briefing_gate(&state, &id, bridge.as_deref()).await {
            return sse_refusal(payload);
        }
    }
    super::full::full_audit(
        axum::extract::State(state),
        axum::extract::Path(id),
        axum::Json(req),
    )
    .await
}

/// POST /api/projects/:id/partial-audit — the briefing gate, then the refresh.
pub async fn partial_audit_route(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    bridge: Bridge,
    axum::Json(req): axum::Json<PartialAuditRequest>,
) -> axum::response::sse::Sse<super::SseStream> {
    if super::agent_can_audit(&req.agent) {
        if let Gate::Refuse(payload) = briefing_gate(&state, &id, bridge.as_deref()).await {
            return sse_refusal(payload);
        }
    }
    super::drift::partial_audit(
        axum::extract::State(state),
        axum::extract::Path(id),
        axum::Json(req),
    )
    .await
}

/// POST /api/projects/:id/revalidate — the briefing gate, then the revalidation.
pub async fn revalidate_audit_route(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    bridge: Bridge,
    axum::Json(req): axum::Json<RevalidateAuditRequest>,
) -> axum::response::sse::Sse<super::SseStream> {
    if super::agent_can_audit(&req.agent) {
        if let Gate::Refuse(payload) = briefing_gate(&state, &id, bridge.as_deref()).await {
            return sse_refusal(payload);
        }
    }
    super::revalidate::revalidate_audit(
        axum::extract::State(state),
        axum::extract::Path(id),
        axum::Json(req),
    )
    .await
}

/// What `audit_prepare` reports: whether a briefing exists and, when it does
/// not, the cards to post in the caller's discussion.
#[derive(Debug, serde::Serialize)]
pub struct BriefingStatus {
    pub present: bool,
    pub docs_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discussion_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub briefing_cards: Vec<serde_json::Value>,
}

/// GET /api/projects/:id/briefing-status
pub async fn briefing_status(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    bridge: Bridge,
) -> axum::Json<ApiResponse<BriefingStatus>> {
    let lookup = id.clone();
    let project = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
        .await
        .ok()
        .flatten();
    let Some(project) = project else {
        return axum::Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        ));
    };
    let root = crate::core::scanner::resolve_host_path(&project.path);
    let defaults = briefing_defaults_for(&state, &project).await;
    let present =
        crate::api::projects::resolve_briefing_notes(&root, &project.briefing_notes).is_some();
    let mut status = BriefingStatus {
        present,
        docs_dir: defaults.docs_dir.clone(),
        error_code: (!present).then_some(ApiErrorCode::BriefingRequired.as_str()),
        discussion_id: None,
        briefing_cards: Vec::new(),
    };
    if !present {
        if let Some(bridge) = bridge.as_deref() {
            if let Some((discussion_id, language)) = caller_discussion(&state, bridge, &id).await {
                status.briefing_cards = briefing_cards(&id, &discussion_id, &language, &defaults);
                status.discussion_id = Some(discussion_id);
            }
        }
    }
    axum::Json(ApiResponse::ok(status))
}

// ─── Answers ────────────────────────────────────────────────────────────────

/// Called inside the answer transaction of a card whose task reference is a
/// briefing's. Refuses a card answered outside its discussion or project;
/// saves the briefing once every card of the set is answered.
pub(crate) fn on_card_answered(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    task_ref: &str,
    question: &crate::db::discussion_questions::DiscussionQuestion,
    option: Option<&str>,
    text: Option<&str>,
) -> Result<(), String> {
    let (project_id, card_discussion) =
        parse_task_ref(task_ref).ok_or("malformed briefing card reference")?;
    if card_discussion != discussion_id {
        return Err("this briefing card belongs to another discussion".into());
    }
    let (disc_project, _) = discussion_project(conn, discussion_id)
        .map_err(|e| e.to_string())?
        .ok_or("discussion not found")?;
    if disc_project.as_deref() != Some(project_id) {
        return Err("this briefing card belongs to another project".into());
    }
    let questions = crate::db::discussion_questions::list(conn, discussion_id)
        .map_err(|e| e.to_string())?
        .questions;
    let set: Vec<_> = questions
        .iter()
        .filter(|q| q.task_ref.as_deref() == Some(task_ref))
        .collect();
    let mut answers = Vec::with_capacity(set.len());
    let mut docs_default = String::new();
    for q in &set {
        if q.key == "briefing-docs-dir" {
            docs_default = q
                .options
                .first()
                .map(|o| o.label.clone())
                .unwrap_or_default();
        }
        if q.id == question.id {
            answers.push(CardAnswer {
                key: &q.key,
                option,
                text,
            });
        } else if let Some(answer) = q.answer.as_ref() {
            answers.push(CardAnswer {
                key: &q.key,
                option: answer.selected_option_ids.first().map(String::as_str),
                text: answer.text.as_deref(),
            });
        } else {
            // Another card of the set still waits for the human.
            return Ok(());
        }
    }
    let Some(form) = answers_to_form(&answers, &docs_default) else {
        return Ok(());
    };
    let project = crate::db::projects::get_project(conn, project_id)
        .map_err(|e| e.to_string())?
        .ok_or("project not found")?;
    super::briefing::save_briefing_in(conn, &project, &form, false, None).map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> BriefingDefaults {
        BriefingDefaults {
            docs_dir: "docs".into(),
            docs_dir_source: super::super::briefing::BriefingDocsDirSource::Default,
            folder_exists: true,
        }
    }

    #[test]
    fn the_cards_are_valid_kronn_questions_bound_to_their_discussion() {
        let cards = briefing_cards("p1", "d1", "fr", &defaults());
        assert_eq!(cards.len(), 15);
        let mut keys = std::collections::HashSet::new();
        for card in &cards {
            assert_eq!(card["version"], 1);
            assert_eq!(card["task_ref"], "audit-briefing:p1:d1");
            assert!(keys.insert(card["key"].as_str().unwrap().to_string()));
            assert!(card["options"].as_array().unwrap().len() <= 8);
        }
        let docs = cards.last().unwrap();
        assert_eq!(docs["question"], "Dossier de la documentation IA");
        assert!(docs["context"]
            .as_str()
            .unwrap()
            .contains("Le dossier docs/ utilisé par Kronn existe déjà"));
        assert_eq!(parse_task_ref("audit-briefing:p1:d1"), Some(("p1", "d1")));
        assert_eq!(
            briefing_cards("p1", "d1", "en", &defaults())[0]["question"],
            "What is this project for, and who uses it?"
        );
    }

    #[test]
    fn answered_cards_become_a_briefing_and_a_missing_one_waits() {
        let d = "docs/";
        let mut answers: Vec<CardAnswer<'_>> = FREE_FIELDS
            .iter()
            .map(|f| CardAnswer {
                key: Box::leak(format!("briefing-{f}").into_boxed_str()),
                option: None,
                text: Some("typed"),
            })
            .collect();
        answers.push(CardAnswer {
            key: "briefing-additional",
            option: Some(NOT_PROVIDED),
            text: None,
        });
        for (field, _) in FACTS {
            answers.push(CardAnswer {
                key: Box::leak(format!("briefing-{field}").into_boxed_str()),
                option: Some("unknown"),
                text: None,
            });
        }
        assert!(
            answers_to_form(&answers, d).is_none(),
            "the folder card is missing"
        );
        answers.push(CardAnswer {
            key: "briefing-docs-dir",
            option: None,
            text: Some(".ai"),
        });
        answers[6].option = Some("jira");
        let form = answers_to_form(&answers, d).unwrap();
        assert_eq!(form.purpose, "typed");
        assert_eq!(form.docs_dir.as_deref(), Some(".ai"));
        assert_eq!(form.project.unwrap().tracker, BriefingTracker::Jira);
    }

    // ─── Gate and answers, on a real database ──────────────────────────────

    struct Fixture {
        state: crate::AppState,
        _repo: tempfile::TempDir,
        root: std::path::PathBuf,
    }

    async fn fixture() -> Fixture {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO projects (id, name, path, created_at, updated_at) \
                     VALUES ('p-cards', 'p', ?1, 'now', 'now')",
                    [&path],
                )?;
                for (id, lang) in [("d-cards", "fr"), ("d-other", "en")] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, agent, language, project_id, created_at, updated_at) \
                         VALUES (?1, 'D', 'ClaudeCode', ?2, 'p-cards', 'now', 'now')",
                        [id, lang],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        Fixture {
            state,
            _repo: repo,
            root,
        }
    }

    fn bridge(discussions: &[&str], run: Option<&str>) -> crate::core::bridge_token::BridgeCaller {
        crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: Some("p-cards".into()),
            own_discussions: discussions.iter().map(|d| d.to_string()).collect(),
            own_run: run.map(str::to_string),
            agent: None,
        }
    }

    fn post_cards(
        conn: &rusqlite::Connection,
        discussion: &str,
        message: &str,
        cards: &[serde_json::Value],
    ) {
        let content: String = cards
            .iter()
            .map(|c| format!("```kronn-question\n{c}\n```\n\n"))
            .collect();
        let msg = crate::models::DiscussionMessage {
            id: message.into(),
            role: MessageRole::Agent,
            channel: MessageChannel::Main,
            content,
            agent_type: Some(AgentType::ClaudeCode),
            timestamp: chrono::Utc::now(),
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
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
        };
        crate::db::discussions::insert_message(conn, discussion, &msg).unwrap();
    }

    fn answer(
        conn: &rusqlite::Connection,
        discussion: &str,
        question: &crate::db::discussion_questions::DiscussionQuestion,
    ) -> Result<(), String> {
        let (selected, text) = match question.key.as_str() {
            "briefing-docs-dir" => (vec![], Some(".ai".to_string())),
            "briefing-tracker" => (vec!["jira".to_string()], None),
            "briefing-additional" => (vec![NOT_PROVIDED.to_string()], None),
            key if key.starts_with("briefing-") && question.options.len() == 1 => {
                (vec![], Some(format!("answer to {key}")))
            }
            _ => (vec!["unknown".to_string()], None),
        };
        crate::db::discussion_questions::answer(
            conn,
            discussion,
            &question.id,
            &crate::db::discussion_questions::AnswerDiscussionQuestionRequest {
                selected_option_ids: selected,
                item_answers: vec![],
                text,
                idempotency_key: format!("k-{}", question.id),
            },
            "human",
            None,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn a_launch_without_briefing_is_refused_from_the_ui() {
        let fx = fixture().await;
        let Gate::Refuse(payload) = briefing_gate(&fx.state, "p-cards", None).await else {
            panic!("a UI launch without briefing must be refused");
        };
        assert_eq!(payload["error_code"], "briefing_required");
        assert!(payload.get("briefing_cards").is_none());
    }

    #[tokio::test]
    async fn an_mcp_launch_gets_the_cards_and_answering_them_saves_the_briefing() {
        let fx = fixture().await;
        let caller = bridge(&["d-cards"], None);
        let Gate::Refuse(payload) = briefing_gate(&fx.state, "p-cards", Some(&caller)).await else {
            panic!("an MCP launch without briefing gets the cards");
        };
        assert_eq!(payload["error_code"], "briefing_required");
        assert_eq!(payload["discussion_id"], "d-cards");
        let cards: Vec<serde_json::Value> =
            serde_json::from_value(payload["briefing_cards"].clone()).unwrap();
        assert_eq!(cards.len(), 15);
        assert_eq!(cards[0]["question"], "À quoi sert ce projet, et pour qui ?");

        let path = fx.root.to_string_lossy().into_owned();
        let outcome = fx
            .state
            .db
            .with_conn(move |conn| {
                post_cards(conn, "d-cards", "m-cards", &cards);
                post_cards(conn, "d-other", "m-other", &cards);
                // A card answered in a discussion that did not post it is refused.
                let foreign = crate::db::discussion_questions::list(conn, "d-other")?.questions;
                let refused = answer(conn, "d-other", &foreign[0]);
                let questions = crate::db::discussion_questions::list(conn, "d-cards")?.questions;
                assert_eq!(questions.len(), 15);
                let mut notes_before_last = None;
                for (i, q) in questions.iter().enumerate() {
                    if i == questions.len() - 1 {
                        notes_before_last =
                            crate::db::projects::get_project_briefing_notes(conn, "p-cards")?;
                    }
                    answer(conn, "d-cards", q).map_err(|e| anyhow::anyhow!(e))?;
                }
                let notes = crate::db::projects::get_project_briefing_notes(conn, "p-cards")?;
                let dir = crate::db::projects::project_docs_dir(conn, "p-cards")?;
                Ok((refused, notes_before_last, notes, dir))
            })
            .await
            .unwrap();
        crate::core::docs_location::set_project_override(&path, None);
        let (refused, before_last, notes, dir) = outcome;
        assert!(refused.unwrap_err().contains("another discussion"));
        assert_eq!(before_last, None, "nothing is saved before the last card");
        let notes = notes.expect("the briefing is saved");
        assert!(notes.contains("answer to briefing-purpose"), "{notes}");
        assert!(notes.contains("Jira"), "{notes}");
        assert_eq!(dir.as_deref(), Some(".ai"));
        assert!(matches!(
            briefing_gate(&fx.state, "p-cards", Some(&caller)).await,
            Gate::Proceed
        ));
    }

    #[tokio::test]
    async fn an_unattended_launch_gets_the_flagged_defaults() {
        let fx = fixture().await;
        let caller = bridge(&[], Some("run-1"));
        assert!(matches!(
            briefing_gate(&fx.state, "p-cards", Some(&caller)).await,
            Gate::Proceed
        ));
        let notes = fx
            .state
            .db
            .with_conn(|conn| crate::db::projects::get_project_briefing_notes(conn, "p-cards"))
            .await
            .unwrap()
            .unwrap();
        assert!(
            notes.contains(super::super::briefing::DEFAULTS_MARKER),
            "{notes}"
        );
        assert!(notes.contains("to be completed"), "{notes}");
    }
}
