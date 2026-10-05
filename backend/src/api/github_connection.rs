//! `/api/projects/{id}/github` — whether a project's agents receive a GitHub
//! token (design note `agent-secret-boundary.md` §4.5). Responses describe the
//! state and the scope; they never carry a token value.

use axum::{
    extract::{Path, State},
    Json,
};
use zeroize::Zeroizing;

use crate::core::github_connection as gh;
use crate::db::github_connections::{self, GithubConnectionRow};
use crate::models::{
    ApiResponse, GithubConnectionMode, GithubConnectionState, GithubScope, GithubTokenKind,
    ProjectGithubConnection, SetProjectGithubConnectionRequest,
};
use crate::AppState;

async fn load(
    state: &AppState,
    project_id: &str,
) -> Result<(crate::models::Project, Option<GithubConnectionRow>), String> {
    let pid = project_id.to_string();
    let found = state
        .db
        .with_read_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &pid)?;
            let row = github_connections::get(conn, &pid)?;
            Ok(project.map(|project| (project, row)))
        })
        .await
        .map_err(|error| format!("DB error: {error}"))?;
    found.ok_or_else(|| "Project not found".to_string())
}

async fn view(state: &AppState, project_id: &str) -> Result<ProjectGithubConnection, String> {
    let (project, row) = load(state, project_id).await?;
    let (path, repo_url) = (project.path.clone(), project.repo_url.clone());
    let on_github =
        tokio::task::spawn_blocking(move || gh::project_is_on_github(&path, repo_url.as_deref()))
            .await
            .unwrap_or(false);
    let machine = gh::machine_token().await;
    let mode = row
        .as_ref()
        .map_or(GithubConnectionMode::NotConnected, |row| row.mode);
    let connection_state = match mode {
        GithubConnectionMode::NotConnected if machine.is_some() => {
            GithubConnectionState::AvailableButOff
        }
        GithubConnectionMode::NotConnected => GithubConnectionState::NotConnected,
        GithubConnectionMode::GhLogin => GithubConnectionState::ConnectedGhLogin,
        GithubConnectionMode::StoredToken => GithubConnectionState::ConnectedStoredToken,
    };
    Ok(ProjectGithubConnection {
        project_id: project.id,
        mode,
        state: connection_state,
        machine_token_available: machine.is_some(),
        machine_token_source: machine.map(|(_, source)| source),
        on_github,
        scope: row.as_ref().and_then(|row| row.scope.clone()),
        connected_on_upgrade: row.as_ref().is_some_and(|row| row.connected_on_upgrade),
        updated_at: row.and_then(|row| row.updated_at),
    })
}

fn respond(
    result: Result<ProjectGithubConnection, String>,
) -> Json<ApiResponse<ProjectGithubConnection>> {
    match result {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectGithubConnection>> {
    respond(view(&state, &id).await)
}

async fn encryption_secret(state: &AppState) -> Result<String, String> {
    let secret = state.config.read().await.encryption_secret.clone();
    secret.ok_or_else(|| "The encryption key is unavailable; a token cannot be stored".into())
}

async fn store_scope(state: &AppState, project_id: &str, scope: GithubScope) -> Result<(), String> {
    let pid = project_id.to_string();
    state
        .db
        .with_conn(move |conn| github_connections::set_scope(conn, &pid, &scope))
        .await
        .map_err(|error| format!("DB error: {error}"))
}

async fn set_connection(
    state: &AppState,
    project_id: &str,
    request: SetProjectGithubConnectionRequest,
) -> Result<ProjectGithubConnection, String> {
    load(state, project_id).await?;
    let (token, cipher): (Option<Zeroizing<String>>, Option<String>) = match request.mode {
        GithubConnectionMode::NotConnected => (None, None),
        GithubConnectionMode::GhLogin => {
            gh::forget_machine_token().await;
            let (token, _) = gh::machine_token().await.ok_or(
                "No GitHub token on this machine: sign in with `gh auth login`, or paste a token",
            )?;
            (Some(token), None)
        }
        GithubConnectionMode::StoredToken => {
            let pasted = request
                .token
                .as_deref()
                .ok_or("A token is required to connect with a stored token")?;
            let token = Zeroizing::new(gh::validate_pasted_token(pasted)?);
            let secret = encryption_secret(state).await?;
            let cipher = gh::encrypt_stored_token(&token, &secret)?;
            (Some(token), Some(cipher))
        }
    };

    let pid = project_id.to_string();
    let mode = request.mode;
    state
        .db
        .with_conn(move |conn| github_connections::set_mode(conn, &pid, mode, cipher.as_deref()))
        .await
        .map_err(|error| format!("DB error: {error}"))?;
    let grant_token = match mode {
        GithubConnectionMode::StoredToken => token.as_deref().map(String::as_str),
        _ => None,
    };
    gh::set_grant(project_id, mode, grant_token);
    tracing::info!(
        target: "kronn::github",
        project_id,
        mode = mode.as_str(),
        "project GitHub connection changed"
    );

    if let Some(token) = token {
        let scope = gh::verify_scope(&gh::api_base(), &token).await;
        store_scope(state, project_id, scope).await?;
    }
    view(state, project_id).await
}

pub async fn set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetProjectGithubConnectionRequest>,
) -> Json<ApiResponse<ProjectGithubConnection>> {
    respond(set_connection(&state, &id, request).await)
}

async fn refresh(state: &AppState, project_id: &str) -> Result<ProjectGithubConnection, String> {
    let (_, row) = load(state, project_id).await?;
    let stored = row
        .filter(|row| row.mode == GithubConnectionMode::StoredToken)
        .and_then(|row| row.token_encrypted);
    // A project that is off still shows the machine token's scope, so the
    // connect dialog can recommend a narrower token.
    let token = match stored {
        Some(cipher) => {
            let secret = encryption_secret(state).await?;
            Some(
                gh::decrypt_stored_token(&cipher, &secret)
                    .map_err(|_| "The stored GitHub token could not be decrypted".to_string())?,
            )
        }
        None => {
            gh::forget_machine_token().await;
            gh::machine_token().await.map(|(token, _)| token)
        }
    };
    let scope = match token {
        Some(token) => gh::verify_scope(&gh::api_base(), &token).await,
        None => GithubScope {
            verified: false,
            token_kind: GithubTokenKind::Unknown,
            login: None,
            scopes: Vec::new(),
            repositories: Vec::new(),
            repositories_truncated: false,
            broad: false,
            reason: Some("No GitHub token is available on this machine".into()),
            checked_at: chrono::Utc::now(),
        },
    };
    store_scope(state, project_id, scope).await?;
    view(state, project_id).await
}

pub async fn refresh_scope(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectGithubConnection>> {
    respond(refresh(&state, &id).await)
}
