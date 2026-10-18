//! The bridge-token gate, as one decision any caller can ask for: the HTTP
//! middleware for CLI agents and the in-process executor of HTTP agents run
//! the same checks, so one rule protects both (KT-1346).
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::Value;

use crate::core::bridge_token::{self, BridgeCaller, BridgeGrant, BridgeRoute, Reach};
use crate::db::Database;

/// A request the gate let through, with what the handler and the response
/// scoping need.
pub struct Admitted {
    pub route: &'static BridgeRoute,
    pub grant: Arc<BridgeGrant>,
    pub caller: BridgeCaller,
    pub reach: Reach,
    /// The body the handler must receive: the caller's, or the gate's
    /// prepared one (project forced or kept).
    pub body: Option<Value>,
}

#[derive(Debug)]
pub struct Denied {
    pub status: StatusCode,
    pub message: String,
}

fn denied(status: StatusCode, message: impl Into<String>) -> Denied {
    Denied {
        status,
        message: message.into(),
    }
}

/// Decide one request: resolve and freeze the token's project, build its
/// reach (room grants, the run's validated workflow scope), prepare the
/// body, and check every id it names.
pub async fn admit(
    db: &Database,
    grant: Arc<BridgeGrant>,
    route: &'static BridgeRoute,
    path_params: Vec<(String, String)>,
    query: Option<String>,
    body: Option<Value>,
) -> Result<Admitted, Denied> {
    let resolver = grant.clone();
    let binding = db
        // Re-read on every call: a deleted discussion or run kills the token.
        .with_read_conn(move |conn| bridge_token::resolve_grant_project(conn, &resolver))
        .await;
    let project = match binding {
        Ok(bridge_token::ScopeBinding::Bound(project)) => grant
            .freeze(project)
            .map_err(|refusal| denied(StatusCode::UNAUTHORIZED, refusal.0))?,
        Ok(bridge_token::ScopeBinding::Dead) => {
            return Err(denied(
                StatusCode::UNAUTHORIZED,
                "bridge token's discussion or run no longer exists",
            ))
        }
        Ok(bridge_token::ScopeBinding::Conflict) => {
            return Err(denied(
                StatusCode::UNAUTHORIZED,
                "bridge token's discussions belong to different projects",
            ))
        }
        Err(error) => {
            tracing::warn!(target: "kronn::bridge_token", token = %grant.id, "scope resolution failed: {error}");
            return Err(denied(
                StatusCode::SERVICE_UNAVAILABLE,
                "bridge scope check failed",
            ));
        }
    };
    if grant.scope.owns_nothing()
        && project.is_none()
        && !bridge_token::unbound_route_allowed(route)
    {
        return Err(denied(
            StatusCode::FORBIDDEN,
            "this launch has no discussion, run or project: its bridge token reads catalogues only",
        ));
    }
    // The room grants (KT-1270), re-read on every call: a revoke applies to
    // the next request, and a failed read refuses rather than guessing.
    let reach = if project.is_some() {
        let owned = grant.scope.room_grant_discussions.clone();
        let run = grant.scope.workflow_run_id.clone();
        let granted = db
            .with_read_conn(move |conn| {
                // A room's own agents reach its grants; a run's steps reach their
                // workflow's validated scope; nothing else widens a token.
                let mut granted = crate::db::discussion_scope::granted_projects(conn, &owned)?;
                if let Some(run) = run {
                    granted.merge(crate::db::discussion_scope::run_granted_projects(
                        conn, &run,
                    )?);
                }
                Ok(granted)
            })
            .await
            .map_err(|error| {
                tracing::warn!(target: "kronn::bridge_token", token = %grant.id, "grant lookup failed: {error}");
                denied(StatusCode::SERVICE_UNAVAILABLE, "bridge scope check failed")
            })?;
        Reach {
            bound: project.clone(),
            granted,
        }
    } else {
        Reach::bound_only(None)
    };
    let refused = |refusal: bridge_token::Refusal| {
        tracing::warn!(target: "kronn::bridge_token", token = %grant.id, method = route.method,
            route = route.pattern, reason = %refusal.0, "bridge token refused");
        denied(StatusCode::FORBIDDEN, refusal.0)
    };
    let body = match bridge_token::prepare_body_in(route, &reach, body.as_ref()) {
        Ok(Some(prepared)) => Some(prepared),
        Ok(None) => body,
        Err(refusal) => return Err(refused(refusal)),
    };
    let ids = bridge_token::collect_ids(route, &path_params, query.as_deref(), body.as_ref())
        .map_err(|refusal| denied(StatusCode::FORBIDDEN, refusal.0))?;
    let checked = grant.clone();
    let gate_reach = reach.clone();
    let gate_body = body.clone();
    let decision = db
        .with_read_conn(move |conn| {
            let mut ids = ids;
            ids.extend(bridge_token::credential_targets(
                conn,
                route,
                query.as_deref(),
                gate_body.as_ref(),
            )?);
            let ids = bridge_token::canonical_ids(conn, ids)?;
            Ok(bridge_token::authorize_in(
                &checked,
                route,
                &gate_reach,
                &ids,
                |kind, id| {
                    bridge_token::residence(conn, kind, id).map_err(|error| {
                        bridge_token::Refusal(format!("scope lookup failed: {error}"))
                    })
                },
            ))
        })
        .await;
    match decision {
        Ok(Ok(())) => {}
        Ok(Err(refusal)) => return Err(refused(refusal)),
        Err(error) => {
            tracing::warn!(target: "kronn::bridge_token", token = %grant.id, "scope check failed: {error}");
            return Err(denied(
                StatusCode::SERVICE_UNAVAILABLE,
                "bridge scope check failed",
            ));
        }
    }
    if route.rule == bridge_token::Rule::Effect {
        tracing::info!(target: "kronn::bridge_token", token = %grant.id, method = route.method,
            route = route.pattern, "bridge token effect");
    }
    let caller = BridgeCaller {
        token_id: grant.id.clone(),
        project,
        own_discussions: grant.own_discussions(),
        own_run: grant.scope.workflow_run_id.clone(),
        agent: grant.scope.agent.clone(),
    };
    Ok(Admitted {
        route,
        grant,
        caller,
        reach,
        body,
    })
}

/// Scope a handler's JSON response the way the bridge does: entries outside
/// the token's scope are dropped (counts follow), an object outside it is
/// refused. A discussion the launch just created is adopted first, so it
/// reads as its own.
pub async fn scope_json(
    db: &Database,
    admitted: &Admitted,
    envelope: &mut Value,
) -> Result<(), Denied> {
    let route = admitted.route;
    let grant = &admitted.grant;
    if !bridge_token::scopes_response(route) {
        return Ok(());
    }
    // Only a fresh row counts: an idempotent replay returns a discussion
    // chosen by caller-supplied session ids, which proves nothing.
    if route.pattern == "/api/disc/create" && envelope["data"]["created"] == true {
        if let Some(id) = envelope["data"]["disc_id"].as_str() {
            grant.adopt_discussion(id);
        }
    }
    // A media job the token started in a new discussion: that discussion is
    // the token's, like one from `disc/create`.
    if route.pattern == "/api/media/generate" && envelope["data"]["discussion_created"] == true {
        if let Some(id) = envelope["data"]["discussion_id"].as_str() {
            grant.adopt_discussion(id);
        }
    }
    // A Quick Prompt run opens its own discussion for the launch.
    if route.pattern == "/api/mcp/qp-run" && envelope["success"] == true {
        if let Some(id) = envelope["data"]["disc_id"].as_str() {
            grant.adopt_discussion(id);
        }
    }
    // An `ApiResponse` envelope is scoped through its `data`; any other JSON
    // body is scoped as a whole.
    let is_envelope = envelope
        .as_object()
        .is_some_and(|fields| fields.contains_key("success") && fields.contains_key("data"));
    let data = if is_envelope {
        match envelope.get_mut("data") {
            Some(data) if !data.is_null() => data,
            _ => return Ok(()),
        }
    } else {
        envelope
    };
    let wanted = bridge_token::response_ids(route, data);
    let residences = db
        .with_read_conn(move |conn| bridge_token::resolve_residences(conn, wanted))
        .await
        .map_err(|_| denied(StatusCode::SERVICE_UNAVAILABLE, "bridge scope check failed"))?;
    // An export is a bundle: one dependency the token may not see refuses it
    // whole rather than leaving a bundle that imports half a workflow.
    let whole = bridge_token::scoped_whole_or_refused(route).then(|| data.clone());
    bridge_token::scope_response_in(route, data, grant, &admitted.reach, &residences)
        .map_err(|refusal| denied(StatusCode::FORBIDDEN, refusal.0))?;
    if whole.is_some_and(|whole| whole != *data) {
        return Err(denied(
            StatusCode::FORBIDDEN,
            "this export bundles a resource outside this bridge token's scope",
        ));
    }
    Ok(())
}
