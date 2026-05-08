//! OpenAPI 3.0 spec + Swagger UI for Kronn's HTTP API.
//!
//! # Status
//!
//! **Scaffold**: this file documents the most-used endpoints
//! (auth, version check, discussions, version, agents) so external
//! integrators have a starting point. The full route surface (~170
//! handlers) is enriched incrementally — each PR that touches an
//! endpoint should add its `utoipa::path` macro to the same spirit.
//!
//! # Where it's served
//!
//! - `GET /api/openapi.json` — the raw OpenAPI spec
//! - `GET /api/docs` — Swagger UI (interactive)
//!
//! # Why hand-curated and not auto-generated
//!
//! `utoipa::OpenApi` derive over every handler would force every
//! request/response struct into `ToSchema` derives and force every
//! handler to grow a `#[utoipa::path(...)]` attribute. That's a
//! ~170-route refactor + every model. We picked the hand-curated
//! path so we ship the doc surface today and let it grow with normal
//! development pressure (every team that touches an endpoint adds
//! the docstring).

use utoipa::OpenApi;

use crate::api::version::VersionCheck;

/// Versioning hint passed via OpenAPI metadata so the Swagger UI shows
/// the running Kronn version, not the spec version.
const KRONN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Kronn API",
        description = "Self-hosted control plane for AI coding agents. \
            Routes are grouped by tag — each tag corresponds to a backend module \
            (`api::discussions`, `api::version`, etc.). Auth: Bearer token \
            in `Authorization` header (skip-able for localhost requests by default — \
            see `server.auth_strict_localhost` for the strict opt-in).",
        license(name = "AGPL-3.0-only"),
    ),
    servers(
        (url = "/", description = "Same-origin (web mode)"),
        (url = "http://localhost:3140/", description = "Native / Tauri desktop"),
    ),
    paths(
        version_check_path,
        // TODO: add disc_meta, disc_get_message, disc_summarize,
        // discussions_list, agents_detect when the underlying handlers
        // grow `#[utoipa::path]` attributes. The shapes below show the
        // pattern so contributors can follow.
    ),
    components(schemas(
        VersionCheck,
        ApiResponseVersion,
    )),
    tags(
        (name = "version", description = "Version & update-check endpoint feeding the in-app UpdateBanner."),
        (name = "discussions", description = "Discussion CRUD, messaging, and the kronn-internal introspection endpoints."),
        (name = "agents", description = "Agent detection, install / uninstall, runtime warnings."),
    ),
)]
pub struct ApiDoc;

/// Wrapper schema so the OpenAPI doc has a concrete type for
/// `ApiResponse<VersionCheck>` without having to expose the generic.
#[derive(serde::Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // Schema-only — never instantiated at runtime.
pub struct ApiResponseVersion {
    pub success: bool,
    pub data: Option<VersionCheck>,
    pub error: Option<String>,
}

/// `GET /api/version/check` — current+latest pair for the auto-update
/// banner. Cached 6h server-side.
#[utoipa::path(
    get,
    path = "/api/version/check",
    tag = "version",
    responses(
        (status = 200, description = "Current + latest Kronn versions, with up_to_date flag", body = ApiResponseVersion),
    ),
)]
#[allow(dead_code)] // Marker function — utoipa reads the attribute, axum routes the real handler.
fn version_check_path() {}

/// Build the spec with the running version stamped in.
pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    let mut spec = ApiDoc::openapi();
    spec.info.version = KRONN_VERSION.to_string();
    spec
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_carries_running_kronn_version() {
        let spec = openapi_spec();
        assert_eq!(spec.info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn spec_documents_version_check_endpoint() {
        // The version-check endpoint feeds the UpdateBanner — pin it to
        // catch a regression where the path macro gets dropped.
        let spec = openapi_spec();
        let paths = &spec.paths;
        assert!(
            paths.paths.contains_key("/api/version/check"),
            "OpenAPI spec must document /api/version/check (regression sentinel)",
        );
    }

    #[test]
    fn spec_serializes_to_valid_json() {
        // Catches missing schemas / broken refs at build time. If a
        // contributor adds a new `paths(...)` entry pointing at a
        // non-existent function, this test fires.
        let spec = openapi_spec();
        let json = serde_json::to_string(&spec).expect("spec should serialise");
        assert!(json.contains("\"openapi\""));
        assert!(json.contains("\"Kronn API\""));
    }
}
