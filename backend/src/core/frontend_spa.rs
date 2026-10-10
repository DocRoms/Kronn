//! Serving the built frontend as a single-page app.
//!
//! The frontend routes on the client: `/discussions` or `/workflows/<id>` are
//! addresses of the app, not files of its build. Under Docker, nginx answers
//! them with `try_files … /index.html`. The desktop app has no nginx, so its
//! embedded server applies the same rule through this module — without it, a
//! reload on any address but `/` is a 404.

use std::path::Path;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use tower_http::services::{ServeDir, ServeFile};

/// Subtrees where a miss is a real 404, never a client-side route: `/api`, so
/// a wrong endpoint keeps answering like an API; `/assets`, so a tab still
/// running the previous build gets an error for its missing chunk instead of
/// the app's HTML with a 200.
const NEVER_CLIENT_ROUTES: [&str; 2] = ["/api", "/assets"];

fn is_client_route(path: &str) -> bool {
    !NEVER_CLIENT_ROUTES.iter().any(|subtree| {
        path.strip_prefix(subtree)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

async fn only_client_routes(request: Request, next: Next) -> Response {
    if is_client_route(request.uri().path()) {
        next.run(request).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// The frontend built in `dist_dir`: its files, and `index.html` for every
/// address that is a client-side route.
pub fn frontend_spa_service(dist_dir: &Path) -> ServeDir<Router> {
    let client_routes = Router::new()
        .fallback_service(ServeFile::new(dist_dir.join("index.html")))
        .layer(middleware::from_fn(only_client_routes));
    ServeDir::new(dist_dir)
        .append_index_html_on_directories(true)
        .fallback(client_routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Method};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const INDEX: &str = "<!doctype html><title>Kronn</title>";
    const CHUNK: &str = "console.log('chunk')";

    fn dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), INDEX).unwrap();
        std::fs::write(dir.path().join("favicon.svg"), "<svg/>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/index-abc123.js"), CHUNK).unwrap();
        dir
    }

    /// Mounted the way the desktop server mounts it: behind the API router,
    /// as the fallback of everything the API does not match.
    fn app(dist_dir: &Path) -> Router {
        Router::new()
            .route("/api/health", axum::routing::get(|| async { "healthy" }))
            .fallback_service(frontend_spa_service(dist_dir))
    }

    async fn request(dist_dir: &Path, method: Method, uri: &str) -> (StatusCode, String) {
        let response = app(dist_dir)
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get(dist_dir: &Path, uri: &str) -> (StatusCode, String) {
        request(dist_dir, Method::GET, uri).await
    }

    #[tokio::test]
    async fn serves_the_files_of_the_build() {
        let dist = dist();

        assert_eq!(get(dist.path(), "/").await, (StatusCode::OK, INDEX.into()));
        assert_eq!(
            get(dist.path(), "/index.html").await,
            (StatusCode::OK, INDEX.into())
        );
        assert_eq!(
            get(dist.path(), "/assets/index-abc123.js").await,
            (StatusCode::OK, CHUNK.into())
        );
        assert_eq!(
            get(dist.path(), "/favicon.svg").await,
            (StatusCode::OK, "<svg/>".into())
        );
    }

    #[tokio::test]
    async fn answers_a_client_side_route_with_the_app() {
        let dist = dist();

        for uri in [
            "/projects",
            "/discussions/",
            "/discussions/0b6f3a52-7f4e-4d6b-9a3e-2f1c8d5e7a90",
            "/workflows/wf-1/runs/run-2",
            "/config?section=agents",
            "/projects/%C3%A9t%C3%A9",
            // Look-alikes of the excluded subtrees are ordinary addresses.
            "/apiary",
            "/assets-of-mine",
        ] {
            assert_eq!(
                get(dist.path(), uri).await,
                (StatusCode::OK, INDEX.into()),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn keeps_an_unknown_api_path_a_plain_404() {
        let dist = dist();

        assert_eq!(
            get(dist.path(), "/api/health").await,
            (StatusCode::OK, "healthy".into())
        );
        for uri in ["/api", "/api/", "/api/nope", "/api/projects/unknown/thing"] {
            let (status, body) = get(dist.path(), uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert!(
                !body.contains("Kronn"),
                "{uri} must not answer with the app"
            );
        }
    }

    #[tokio::test]
    async fn keeps_a_missing_build_asset_a_plain_404() {
        let dist = dist();

        // A tab still running the previous build asks for a chunk that the
        // new one no longer ships.
        let (status, body) = get(dist.path(), "/assets/index-0ld999.js").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!body.contains("Kronn"));
    }

    #[tokio::test]
    async fn refuses_to_write_to_a_client_side_route() {
        let dist = dist();

        let (status, body) = request(dist.path(), Method::POST, "/projects").await;

        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert!(!body.contains("Kronn"));
    }

    #[tokio::test]
    async fn never_serves_a_file_outside_the_build() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("secret.txt"), "top secret").unwrap();
        let dist_dir = parent.path().join("dist");
        std::fs::create_dir(&dist_dir).unwrap();
        std::fs::write(dist_dir.join("index.html"), INDEX).unwrap();

        for uri in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/assets/../../secret.txt",
        ] {
            let (_, body) = get(&dist_dir, uri).await;
            assert!(!body.contains("top secret"), "{uri}");
        }
    }

    #[tokio::test]
    async fn is_a_plain_404_when_the_build_has_no_index() {
        let empty = tempfile::tempdir().unwrap();

        let (status, _) = get(empty.path(), "/projects").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
