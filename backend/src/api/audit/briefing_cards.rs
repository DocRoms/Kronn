//! KT-1192 — the briefing as `kronn-question` cards, for an audit launched
//! through MCP on a project without a briefing. The agent posts them in its
//! discussion; once every card is answered there, the answers are saved
//! through the same validation as the form.

use crate::models::*;

use super::briefing::{BriefingDefaults, BriefingFormRequest};

/// Every briefing card carries `audit-briefing:<project>:<discussion>:<set>`:
/// the answers are accepted only in that discussion, for that project, and
/// only for the latest set posted there.
pub(crate) const TASK_REF_PREFIX: &str = "audit-briefing:";

pub(crate) fn task_ref(project_id: &str, discussion_id: &str, set: &str) -> String {
    format!("{TASK_REF_PREFIX}{project_id}:{discussion_id}:{set}")
}

/// `(project, discussion, set)` of a briefing card's task reference.
pub(crate) fn parse_task_ref(task_ref: &str) -> Option<(&str, &str, &str)> {
    let mut parts = task_ref.strip_prefix(TASK_REF_PREFIX)?.splitn(3, ':');
    let parsed = (parts.next()?, parts.next()?, parts.next()?);
    (!parsed.2.is_empty()).then_some(parsed)
}

/// A card's key is `briefing-<field>.<set>`: question keys are unique per
/// discussion, so a second request there needs keys of its own.
fn field_key(key: &str) -> &str {
    key.split_once('.').map_or(key, |(field, _)| field)
}

const FREE_FIELDS: [&str; 5] = ["purpose", "team", "maturity", "dependencies", "traps"];
const NOT_PROVIDED: &str = "not_provided";
const DOCS_DIR_DEFAULT: &str = "default";
const EXISTING_OTHER: &str = "other_folder";
const EXISTING_LEGACY: &str = "move_to_legacy";

struct Wording {
    free: [&'static str; 5],
    additional: &'static str,
    not_provided: &'static str,
    nothing_more: &'static str,
    free_context: &'static str,
    docs_dir: &'static str,
    docs_dir_context: &'static str,
    docs_dir_exists: &'static str,
    existing: &'static str,
    existing_other: &'static str,
    existing_legacy: &'static str,
    existing_warning: &'static str,
    existing_site: &'static str,
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
            existing: "The folder {0} holds documentation Kronn did not write. What should Kronn do?",
            existing_other: "Put Kronn's docs in another folder (answer the folder card with its path)",
            existing_legacy: "Move the existing docs to {0}legacy/",
            existing_warning: "{1} file(s) move to {0}legacy/ and stay there until a human reviews them; the audit reads them as its primary source. Links to them will break.",
            existing_site: " A static-site generator config ({0}) references the folder: it will need updating.",
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
            existing: "La carpeta {0} contiene documentación que Kronn no escribió. ¿Qué debe hacer Kronn?",
            existing_other: "Poner la documentación de Kronn en otra carpeta (responde a la tarjeta de la carpeta con su ruta)",
            existing_legacy: "Mover la documentación existente a {0}legacy/",
            existing_warning: "{1} archivo(s) pasan a {0}legacy/ y se quedan ahí hasta que un humano los revise; la auditoría los lee como fuente principal. Los enlaces hacia ellos se romperán.",
            existing_site: " Una configuración de generador de sitio estático ({0}) apunta a la carpeta: habrá que actualizarla.",
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
            existing: "Le dossier {0} contient une documentation que Kronn n'a pas écrite. Que doit faire Kronn ?",
            existing_other: "Placer la doc Kronn dans un autre dossier (réponds à la carte du dossier avec son chemin)",
            existing_legacy: "Déplacer la doc existante dans {0}legacy/",
            existing_warning: "{1} fichier(s) passent dans {0}legacy/ et y restent jusqu'à l'audit humain ; l'audit les lit comme source principale. Les liens vers eux casseront.",
            existing_site: " Une configuration de générateur de site statique ({0}) référence le dossier : il faudra la mettre à jour.",
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

/// The briefing as ready-to-post cards, in `language`: a new set each call.
pub(crate) fn briefing_cards(
    project_id: &str,
    discussion_id: &str,
    language: &str,
    defaults: &BriefingDefaults,
) -> Vec<serde_json::Value> {
    let set = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    briefing_cards_set(project_id, discussion_id, language, defaults, &set)
}

fn briefing_cards_set(
    project_id: &str,
    discussion_id: &str,
    language: &str,
    defaults: &BriefingDefaults,
    set: &str,
) -> Vec<serde_json::Value> {
    let w = wording(language);
    let task_ref = task_ref(project_id, discussion_id, set);
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
    if let crate::core::docs_folder::DocsFolderState::HumanDocs {
        files,
        site_generator,
        ..
    } = &defaults.folder
    {
        let mut warning = w
            .existing_warning
            .replace("{0}", &folder)
            .replace("{1}", &files.to_string());
        if let Some(site) = site_generator {
            warning.push_str(&w.existing_site.replace("{0}", site));
        }
        cards.push(card(
            "briefing-existing-docs".into(),
            &w.existing.replace("{0}", &folder),
            Some(warning.clone()),
            vec![
                option(EXISTING_OTHER, w.existing_other),
                serde_json::json!({
                    "id": EXISTING_LEGACY,
                    "label": w.existing_legacy.replace("{0}", &folder),
                    "description": warning,
                }),
            ],
            EXISTING_OTHER,
            &task_ref,
        ));
    }
    for card in &mut cards {
        let key = format!("{}.{set}", card["key"].as_str().unwrap_or_default());
        card["key"] = serde_json::json!(key);
    }
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
        move_existing_to_legacy: find("existing-docs").and_then(|a| a.option)
            == Some(EXISTING_LEGACY),
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
        // Unattended defaults never move anything: a folder holding a
        // human's docs stops the launch (the safer of stopping and writing
        // somewhere unexpected).
        let defaults = briefing_defaults_for(state, &project).await;
        if let crate::core::docs_folder::DocsFolderState::HumanDocs { files, .. } = defaults.folder
        {
            return Gate::Refuse(serde_json::json!({
                "error": format!(
                    "No briefing, and {}/ holds {files} file(s) Kronn did not write: an unattended \
                     launch never moves them. Fill in the briefing and choose another folder or a move \
                     to {}/legacy/.",
                    defaults.docs_dir, defaults.docs_dir
                ),
                "error_code": ApiErrorCode::BriefingRequired.as_str(),
            }));
        }
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
/// saves the briefing once every card of the latest set is answered, and
/// returns what must run on disk after the answer is committed.
pub(crate) fn on_card_answered(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    task_ref: &str,
    question: &crate::db::discussion_questions::DiscussionQuestion,
    option: Option<&str>,
    text: Option<&str>,
) -> Result<Option<super::briefing::BriefingAfterCommit>, String> {
    let (project_id, card_discussion, _) =
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
    // A later request superseded this set: its answers stay, nothing is saved.
    let latest = questions.iter().rev().find_map(|q| {
        let task_ref = q.task_ref.as_deref()?;
        let (project, discussion, _) = parse_task_ref(task_ref)?;
        (project == project_id && discussion == discussion_id).then_some(task_ref)
    });
    if latest != Some(task_ref) {
        return Ok(None);
    }
    let set: Vec<_> = questions
        .iter()
        .filter(|q| q.task_ref.as_deref() == Some(task_ref))
        .collect();
    let mut answers = Vec::with_capacity(set.len());
    let mut docs_default = String::new();
    for q in &set {
        if field_key(&q.key) == "briefing-docs-dir" {
            docs_default = q
                .options
                .first()
                .map(|o| o.label.clone())
                .unwrap_or_default();
        }
        if q.id == question.id {
            answers.push(CardAnswer {
                key: field_key(&q.key),
                option,
                text,
            });
        } else if let Some(answer) = q.answer.as_ref() {
            answers.push(CardAnswer {
                key: field_key(&q.key),
                option: answer.selected_option_ids.first().map(String::as_str),
                text: answer.text.as_deref(),
            });
        } else {
            // Another card of the set still waits for the human.
            return Ok(None);
        }
    }
    let Some(form) = answers_to_form(&answers, &docs_default) else {
        return Ok(None);
    };
    let project = crate::db::projects::get_project(conn, project_id)
        .map_err(|e| e.to_string())?
        .ok_or("project not found")?;
    super::briefing::save_briefing_in(conn, &project, &form, false, None)
        .map(Some)
        .map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> BriefingDefaults {
        BriefingDefaults {
            docs_dir: "docs".into(),
            docs_dir_source: super::super::briefing::BriefingDocsDirSource::Default,
            folder_exists: true,
            folder: crate::core::docs_folder::DocsFolderState::KronnManaged,
        }
    }

    #[test]
    fn the_cards_are_valid_kronn_questions_bound_to_their_discussion() {
        let cards = briefing_cards_set("p1", "d1", "fr", &defaults(), "s1");
        assert_eq!(cards.len(), 15);
        let mut keys = std::collections::HashSet::new();
        for card in &cards {
            assert_eq!(card["version"], 1);
            assert_eq!(card["task_ref"], "audit-briefing:p1:d1:s1");
            let key = card["key"].as_str().unwrap();
            assert!(key.ends_with(".s1"), "{key}");
            assert!(keys.insert(key.to_string()));
            assert!(card["options"].as_array().unwrap().len() <= 8);
        }
        let docs = cards.last().unwrap();
        assert_eq!(docs["question"], "Dossier de la documentation IA");
        assert!(docs["context"]
            .as_str()
            .unwrap()
            .contains("Le dossier docs/ utilisé par Kronn existe déjà"));
        assert_eq!(
            parse_task_ref("audit-briefing:p1:d1:s1"),
            Some(("p1", "d1", "s1"))
        );
        assert_eq!(parse_task_ref("audit-briefing:p1:d1"), None);
        // Each request is a set of its own: its keys never collide.
        let again = briefing_cards("p1", "d1", "fr", &defaults());
        assert_ne!(
            again[0]["key"],
            briefing_cards("p1", "d1", "fr", &defaults())[0]["key"]
        );
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
        assert!(!form.move_existing_to_legacy, "no card, no move");
        answers.push(CardAnswer {
            key: "briefing-existing-docs",
            option: Some(EXISTING_LEGACY),
            text: None,
        });
        assert!(
            answers_to_form(&answers, d)
                .unwrap()
                .move_existing_to_legacy
        );
        answers.pop();
        answers.push(CardAnswer {
            key: "briefing-existing-docs",
            option: Some(EXISTING_OTHER),
            text: None,
        });
        assert!(
            !answers_to_form(&answers, d)
                .unwrap()
                .move_existing_to_legacy
        );
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
            session_continuity: None,
        };
        crate::db::discussions::insert_message(conn, discussion, &msg).unwrap();
    }

    fn answer(
        conn: &rusqlite::Connection,
        discussion: &str,
        question: &crate::db::discussion_questions::DiscussionQuestion,
    ) -> Result<(), String> {
        let (selected, text) = match field_key(&question.key) {
            "briefing-docs-dir" => (vec![], Some(".ai".to_string())),
            "briefing-tracker" => (vec!["jira".to_string()], None),
            "briefing-additional" => (vec![NOT_PROVIDED.to_string()], None),
            "briefing-existing-docs" => (vec![EXISTING_LEGACY.to_string()], None),
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

    #[test]
    fn a_folder_of_human_docs_adds_the_explicit_choice_with_its_warning() {
        let d = BriefingDefaults {
            docs_dir: "docs".into(),
            docs_dir_source: super::super::briefing::BriefingDocsDirSource::Existing,
            folder_exists: true,
            folder: crate::core::docs_folder::DocsFolderState::HumanDocs {
                files: 7,
                site_generator: Some("mkdocs.yml".into()),
                legacy_exists: false,
            },
        };
        let cards = briefing_cards("p1", "d1", "fr", &d);
        let existing = cards.last().unwrap();
        assert_eq!(
            field_key(existing["key"].as_str().unwrap()),
            "briefing-existing-docs"
        );
        let ids: Vec<_> = existing["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].clone())
            .collect();
        assert_eq!(ids, vec!["other_folder", "move_to_legacy"]);
        let warning = existing["context"].as_str().unwrap();
        assert!(warning.contains("7 fichier(s)"), "{warning}");
        assert!(warning.contains("casseront"), "{warning}");
        assert!(warning.contains("mkdocs.yml"), "{warning}");
        assert_eq!(
            existing["recommended_option_ids"],
            serde_json::json!(["other_folder"])
        );
    }

    fn human_docs_defaults() -> BriefingDefaults {
        BriefingDefaults {
            docs_dir: ".ai".into(),
            docs_dir_source: super::super::briefing::BriefingDocsDirSource::Existing,
            folder_exists: true,
            folder: crate::core::docs_folder::DocsFolderState::HumanDocs {
                files: 1,
                site_generator: None,
                legacy_exists: false,
            },
        }
    }

    fn notes(conn: &rusqlite::Connection) -> Option<String> {
        crate::db::projects::get_project_briefing_notes(conn, "p-cards").unwrap()
    }

    #[tokio::test]
    async fn only_the_latest_set_of_cards_in_a_discussion_is_saved() {
        let fx = fixture().await;
        let path = fx.root.to_string_lossy().into_owned();
        let old = briefing_cards_set("p-cards", "d-cards", "fr", &defaults(), "s1");
        let new = briefing_cards_set("p-cards", "d-cards", "fr", &defaults(), "s2");
        let outcome = fx
            .state
            .db
            .with_conn(move |conn| {
                post_cards(conn, "d-cards", "m-old", &old);
                post_cards(conn, "d-cards", "m-new", &new);
                let questions = crate::db::discussion_questions::list(conn, "d-cards")?.questions;
                assert_eq!(questions.len(), 30, "both sets are recorded");
                let in_set = |set: &str| {
                    questions
                        .iter()
                        .filter(|q| q.task_ref.as_deref().is_some_and(|r| r.ends_with(set)))
                        .cloned()
                        .collect::<Vec<_>>()
                };
                for q in in_set(":s1") {
                    answer(conn, "d-cards", &q).map_err(|e| anyhow::anyhow!(e))?;
                }
                let after_superseded = notes(conn);
                for q in in_set(":s2") {
                    answer(conn, "d-cards", &q).map_err(|e| anyhow::anyhow!(e))?;
                }
                Ok((after_superseded, notes(conn)))
            })
            .await
            .unwrap();
        crate::core::docs_location::set_project_override(&path, None);
        let (after_superseded, saved) = outcome;
        assert_eq!(after_superseded, None, "a superseded set saves nothing");
        assert!(saved
            .expect("the latest set is saved")
            .contains("answer to briefing-purpose"));
    }

    #[tokio::test]
    async fn a_failed_form_save_moves_nothing_and_keeps_the_folder() {
        let fx = fixture().await;
        let path = fx.root.to_string_lossy().into_owned();
        std::fs::create_dir_all(fx.root.join(".ai")).unwrap();
        std::fs::write(fx.root.join(".ai/install.md"), "# Install\n").unwrap();
        fx.state
            .db
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER fail_notes BEFORE UPDATE OF briefing_notes ON projects \
                     BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let form = BriefingFormRequest {
            purpose: "P".into(),
            team: "T".into(),
            maturity: "M".into(),
            dependencies: "D".into(),
            traps: "Tr".into(),
            docs_dir: Some(".ai".into()),
            move_existing_to_legacy: true,
            ..BriefingFormRequest::default()
        };
        let saved = super::super::briefing::save_briefing_answers(&fx.state, "p-cards", form).await;
        let dir = fx
            .state
            .db
            .with_conn(|conn| crate::db::projects::project_docs_dir(conn, "p-cards"))
            .await
            .unwrap();
        let root = crate::core::docs_location::DocsRoot::of(&fx.root);
        crate::core::docs_location::set_project_override(&path, None);
        assert!(saved.is_err(), "the injected DB failure fails the save");
        assert!(fx.root.join(".ai/install.md").is_file());
        assert!(!fx.root.join(".ai/legacy").exists());
        assert_eq!(dir, None, "the folder choice is rolled back");
        assert!(root.is_default(), "no override survives a failed save");
    }

    #[tokio::test]
    async fn a_failed_card_answer_moves_nothing_and_a_committed_one_moves_on_choice() {
        let fx = fixture().await;
        let path = fx.root.to_string_lossy().into_owned();
        std::fs::create_dir_all(fx.root.join(".ai")).unwrap();
        std::fs::write(fx.root.join(".ai/install.md"), "# Install\n").unwrap();
        let cards = briefing_cards_set("p-cards", "d-cards", "fr", &human_docs_defaults(), "s1");
        assert_eq!(cards.len(), 16);
        let root = fx.root.clone();
        let outcome = fx
            .state
            .db
            .with_conn(move |conn| {
                post_cards(conn, "d-cards", "m-cards", &cards);
                let questions = crate::db::discussion_questions::list(conn, "d-cards")?.questions;
                let (last, rest) = questions.split_last().unwrap();
                for q in rest {
                    answer(conn, "d-cards", q).map_err(|e| anyhow::anyhow!(e))?;
                }
                conn.execute_batch(
                    "CREATE TRIGGER fail_answer BEFORE UPDATE OF state ON discussion_questions \
                     BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                let failed = answer(conn, "d-cards", last);
                let kept =
                    root.join(".ai/install.md").is_file() && !root.join(".ai/legacy").exists();
                let notes_after_failure = notes(conn);
                conn.execute_batch("DROP TRIGGER fail_answer")?;
                answer(conn, "d-cards", last).map_err(|e| anyhow::anyhow!(e))?;
                Ok((failed, kept, notes_after_failure, notes(conn)))
            })
            .await
            .unwrap();
        crate::core::docs_location::set_project_override(&path, None);
        let (failed, kept, notes_after_failure, saved) = outcome;
        assert!(failed.is_err(), "the injected DB failure fails the answer");
        assert!(kept, "nothing moves before the answer is committed");
        assert_eq!(notes_after_failure, None);
        assert!(saved.is_some());
        assert!(fx.root.join(".ai/legacy/install.md").is_file());
        assert!(!fx.root.join(".ai/install.md").exists());
    }
}
