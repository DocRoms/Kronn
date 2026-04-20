//! OAuth2 client-credentials token cache for API plugins.
//!
//! Each config using `ApiAuthKind::OAuth2ClientCredentials` ends up needing
//! an `Authorization: Bearer <access_token>` header on every request. Tokens
//! are short-lived (Adobe: 24h, Google Cloud: ~1h) but expensive to mint —
//! each exchange costs one HTTPS round-trip to the provider. We cache them
//! in memory keyed by `mcp_configs.id` and refresh lazily when about to
//! expire.
//!
//! **Why not persist to DB?** Tokens are disposable by design; the worst
//! case after a backend restart is one extra HTTPS round-trip per active
//! plugin, no user-visible impact. Persistence would add schema churn and
//! a crypto path for very little win.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use tokio::sync::Mutex;

use crate::models::ApiAuthKind;

/// A cached bearer token with an absolute refresh deadline.
///
/// `refresh_at` is set to `(issued_at + expires_in) - SAFETY_MARGIN` so we
/// re-mint slightly ahead of the provider's real expiry, avoiding races
/// where the token is valid when we check but expired when the API receives
/// the request. 30s is plenty for network drift.
#[derive(Debug, Clone)]
pub struct CachedToken {
    pub access_token: String,
    pub refresh_at: Instant,
}

const SAFETY_MARGIN: Duration = Duration::from_secs(30);

/// Fetch a valid bearer token for an OAuth2 config, reusing the cache when
/// possible. Blocks only on the exchange HTTP call when a refresh is needed.
///
/// Returns `Err` with a human-readable message on:
/// - missing `client_id` / `client_secret` in the decrypted env
/// - non-2xx response from the token endpoint
/// - malformed response (no `access_token` field)
///
/// Callers downstream surface this error inline in the prompt injection
/// block so the agent sees *"Adobe Analytics: token exchange failed — …"*
/// and can tell the user, rather than sending an unauthenticated request
/// that will 401 without context.
pub async fn resolve_token(
    cache: &Arc<Mutex<HashMap<String, CachedToken>>>,
    config_id: &str,
    auth: &ApiAuthKind,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let (token_url, client_id_env, client_secret_env, scope) = match auth {
        ApiAuthKind::OAuth2ClientCredentials { token_url, client_id_env, client_secret_env, scope, .. } => {
            (token_url, client_id_env, client_secret_env, scope)
        }
        _ => return Err("resolve_token called on a non-OAuth2 auth kind".into()),
    };

    // Cache hit (still valid): return without round-trip.
    {
        let guard = cache.lock().await;
        if let Some(cached) = guard.get(config_id) {
            if cached.refresh_at > Instant::now() {
                return Ok(cached.access_token.clone());
            }
        }
    }

    let client_id = env.get(client_id_env)
        .ok_or_else(|| format!("missing env var {}", client_id_env))?;
    let client_secret = env.get(client_secret_env)
        .ok_or_else(|| format!("missing env var {}", client_secret_env))?;

    // Adobe IMS uses comma-separated scopes in `scope=` param; Google uses
    // space-separated. We forward the literal value provided by the
    // registry entry — conversion, if any, is the plugin spec's job.
    let mut params = vec![
        ("grant_type", "client_credentials"),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
    ];
    if !scope.is_empty() {
        params.push(("scope", scope.as_str()));
    }

    let http = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client init failed: {}", e))?;

    let resp = http.post(token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("token exchange HTTP error: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "token exchange failed ({}): {}",
            status,
            // Trim the body so we don't dump kilobytes of Adobe HTML into logs.
            &body.chars().take(300).collect::<String>(),
        ));
    }

    // Accept both `access_token` (RFC 6749 canonical) and providers that
    // drift slightly. `expires_in` is seconds — default 3600 if absent.
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("token response JSON parse error: {} — body was: {}", e, &body.chars().take(200).collect::<String>()))?;

    let access_token = json.get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("token response missing `access_token` field: {}", &body.chars().take(200).collect::<String>()))?
        .to_string();

    let expires_in = json.get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    let refresh_at = Instant::now() + Duration::from_secs(expires_in).saturating_sub(SAFETY_MARGIN);

    // Insert into cache for subsequent requests on this config.
    {
        let mut guard = cache.lock().await;
        guard.insert(
            config_id.to_string(),
            CachedToken { access_token: access_token.clone(), refresh_at },
        );
    }

    tracing::info!(
        "OAuth2 token minted for config {} — refresh in {}s",
        config_id, expires_in.saturating_sub(SAFETY_MARGIN.as_secs()),
    );

    Ok(access_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ApiAuthKind;

    fn sample_auth() -> ApiAuthKind {
        ApiAuthKind::OAuth2ClientCredentials {
            token_url: "http://127.0.0.1:1/unused".into(),
            client_id_env: "CLIENT_ID".into(),
            client_secret_env: "CLIENT_SECRET".into(),
            scope: "read".into(),
            extra_headers: vec![],
        }
    }

    #[tokio::test]
    async fn cache_hit_returns_without_http() {
        // Pre-populate the cache with a still-valid token. The token URL
        // points at a closed port — if the cache short-circuit is broken
        // the test would flake on that HTTP attempt.
        let cache = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut g = cache.lock().await;
            g.insert(
                "cfg-1".into(),
                CachedToken {
                    access_token: "cached-abc".into(),
                    refresh_at: Instant::now() + Duration::from_secs(300),
                },
            );
        }
        let mut env = HashMap::new();
        env.insert("CLIENT_ID".into(), "x".into());
        env.insert("CLIENT_SECRET".into(), "y".into());

        let tok = resolve_token(&cache, "cfg-1", &sample_auth(), &env).await.unwrap();
        assert_eq!(tok, "cached-abc");
    }

    #[tokio::test]
    async fn missing_env_var_returns_clear_error() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let env = HashMap::new(); // no CLIENT_ID
        let err = resolve_token(&cache, "cfg-2", &sample_auth(), &env).await.unwrap_err();
        assert!(err.contains("CLIENT_ID"), "error must name the missing env key: {}", err);
    }

    #[tokio::test]
    async fn wrong_auth_kind_returns_typed_error() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let env = HashMap::new();
        let non_oauth = ApiAuthKind::Bearer { env_key: "X".into() };
        let err = resolve_token(&cache, "cfg-3", &non_oauth, &env).await.unwrap_err();
        assert!(err.contains("non-OAuth2"));
    }

    #[test]
    fn cached_token_obeys_refresh_at_boundary() {
        // Sanity: an Instant in the past means "must refresh"; in the
        // future means "still valid". We don't want the ordering semantics
        // to drift if Instant's contract ever changes.
        let past = CachedToken { access_token: "old".into(), refresh_at: Instant::now() - Duration::from_secs(1) };
        let future = CachedToken { access_token: "new".into(), refresh_at: Instant::now() + Duration::from_secs(60) };
        assert!(past.refresh_at <= Instant::now());
        assert!(future.refresh_at > Instant::now());
    }
}
