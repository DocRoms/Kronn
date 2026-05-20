//! OAuth2 client-credentials + 0.8.6 generic token-exchange cache for API plugins.
//!
//! Each config using `ApiAuthKind::OAuth2ClientCredentials` OR
//! `ApiAuthKind::TokenExchange` ends up needing an `Authorization: Bearer
//! <access_token>` (or similar) header on every request. Tokens are short-
//! lived (Adobe: 24h, Google Cloud: ~1h, Didomi: 1h) but expensive to mint
//! — each exchange costs one HTTPS round-trip to the provider. We cache
//! them in memory keyed by `mcp_configs.id` and refresh lazily when about
//! to expire. Both auth flavours share the same `CachedToken` shape and
//! cache storage so future variants can plug in trivially.
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

use crate::models::{ApiAuthKind, TokenExchangeBodyFormat};

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

// ─── 0.8.6 — Generic TokenExchange ─────────────────────────────────────────
//
// Same cache, same TTL semantics, different exchange shape. The OAuth2
// variant hardcodes `grant_type=client_credentials` form-encoded body with
// `client_id`/`client_secret`/`scope` field names. TokenExchange lets the
// plugin spec declare ANY body shape (JSON or form-encoded) with arbitrary
// `${ENV.KEY}` substitutions, and any JSONPath for the token field. Unlocks
// APIs like Didomi (POST /sessions with `{type, key, secret}` JSON body
// → `access_token`) without forcing them into the OAuth2 mould.

/// Resolve a token for an `ApiAuthKind::TokenExchange` config, reusing
/// the cache when possible. Bubbles `Err(String)` on missing env vars,
/// non-2xx exchange response, malformed JSON, or JSONPath miss — each
/// case carries a human-readable message the agent / operator can act on.
pub async fn resolve_token_exchange(
    cache: &Arc<Mutex<HashMap<String, CachedToken>>>,
    config_id: &str,
    auth: &ApiAuthKind,
    base_url: &str,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let (endpoint, method, body_template, body_format, token_jsonpath, ttl_seconds) = match auth {
        ApiAuthKind::TokenExchange {
            endpoint,
            method,
            body_template,
            body_format,
            token_jsonpath,
            ttl_seconds,
            ..
        } => (endpoint, method, body_template, body_format, token_jsonpath, *ttl_seconds),
        _ => return Err("resolve_token_exchange called on a non-TokenExchange auth kind".into()),
    };

    // Cache hit: short-circuit before any HTTP.
    if ttl_seconds > 0 {
        let guard = cache.lock().await;
        if let Some(cached) = guard.get(config_id) {
            if cached.refresh_at > Instant::now() {
                return Ok(cached.access_token.clone());
            }
        }
    }

    // Substitute `${ENV.KEY}` placeholders in every string leaf of the
    // body template. Non-string leaves (numbers, bools, arrays of
    // objects) pass through untouched. Refuses missing env vars with a
    // clear error so operators see exactly which secret to fill.
    let rendered_body = substitute_env_in_value(body_template, env)?;

    // Build the URL. The endpoint is relative to the plugin's
    // `base_url` (e.g. `/sessions` → `https://api.didomi.io/v1/sessions`).
    // We do simple concatenation with an `/` separator — same pattern
    // as the executor's URL builder (cf. `build_url` in
    // api_call_executor.rs).
    let trimmed_base = base_url.trim_end_matches('/');
    let trimmed_endpoint = endpoint.trim_start_matches('/');
    let full_url = format!("{trimmed_base}/{trimmed_endpoint}");

    let http = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client init failed: {}", e))?;

    let req_builder = http.request(
        method.parse().map_err(|e| format!("Invalid HTTP method `{method}`: {e}"))?,
        &full_url,
    );

    // Body format dispatch. JSON sends the rendered Value as-is; form-
    // encoded flattens top-level scalar fields (the OAuth2-RFC-style
    // shape — nested objects would lose info on the wire anyway).
    let resp = match body_format {
        TokenExchangeBodyFormat::Json => {
            req_builder.json(&rendered_body).send().await
        }
        TokenExchangeBodyFormat::FormUrlEncoded => {
            let pairs = flatten_form_pairs(&rendered_body)?;
            req_builder.form(&pairs).send().await
        }
    }
    .map_err(|e| format!("token exchange HTTP error: {}", e))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "token exchange failed ({}): {}",
            status,
            &body.chars().take(300).collect::<String>(),
        ));
    }

    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!(
            "token response JSON parse error: {} — body was: {}",
            e,
            &body.chars().take(200).collect::<String>(),
        ))?;

    // Extract via JSONPath. We use `serde_json_path` (already a workspace
    // dep for ExtractSpec). A miss is a hard error — without the token
    // there's nothing to inject downstream.
    let access_token = extract_token_jsonpath(&json, token_jsonpath)
        .map_err(|e| format!("token extraction failed: {e} — body was: {}", &body.chars().take(200).collect::<String>()))?;

    // Cache (unless ttl=0, which only test code should use).
    if ttl_seconds > 0 {
        let refresh_at = Instant::now()
            + Duration::from_secs(ttl_seconds).saturating_sub(SAFETY_MARGIN);
        let mut guard = cache.lock().await;
        guard.insert(
            config_id.to_string(),
            CachedToken { access_token: access_token.clone(), refresh_at },
        );
    }

    tracing::info!(
        "TokenExchange token minted for config {} — refresh in {}s",
        config_id, ttl_seconds.saturating_sub(SAFETY_MARGIN.as_secs()),
    );

    Ok(access_token)
}

/// Walk a `serde_json::Value` and replace `${ENV.KEY}` placeholders in
/// every string leaf with the corresponding env value. A missing env
/// key returns `Err` with the key name so operators see exactly which
/// secret to fill before retrying. Non-string leaves pass through.
fn substitute_env_in_value(
    value: &serde_json::Value,
    env: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::String(s) => Ok(serde_json::Value::String(substitute_env_in_string(s, env)?)),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(substitute_env_in_value(it, env)?);
            }
            Ok(serde_json::Value::Array(out))
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), substitute_env_in_value(v, env)?);
            }
            Ok(serde_json::Value::Object(out))
        }
        // Numbers, bools, null pass through.
        _ => Ok(value.clone()),
    }
}

/// Substitute `${ENV.KEY}` placeholders in a single string. Supports
/// multiple occurrences in the same string. Missing keys produce an
/// error naming the missing variable.
///
/// 0.8.6 — `pub` since the `api_call_executor` reuses it to interpolate
/// `${ENV.X}` placeholders in endpoint paths, query params, headers, and
/// body string leaves. Same syntax everywhere = predictable for users
/// (Didomi-style: `?organization_id=${ENV.ORGANIZATION_ID}`).
pub fn substitute_env_in_string(
    template: &str,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    loop {
        // 0.8.6 — accept both `${ENV.X}` and `${env.X}` (case-insensitive
        // prefix) — agents naturally type lowercase. The KEY itself is
        // normalised to UPPER for the env lookup since the storage slug
        // convention is UPPER_SNAKE (cf. `slug_env_key` in api/mcps.rs).
        // Without this, `${env.organization_id}` ended up percent-encoded
        // as `%24%7Benv.organization_id%7D` and broke the agent's URL.
        // Caught live 2026-05-20 on Didomi audit.
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find("${env.") else { break };
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 6..]; // skip "${env." / "${ENV."
        if let Some(end) = after_open.find('}') {
            let raw_key = &after_open[..end];
            let normalised_key = raw_key.to_ascii_uppercase();
            let value = env.get(&normalised_key)
                .ok_or_else(|| format!(
                    "missing env var ${{ENV.{normalised_key}}} (also accepted as ${{env.{}}})",
                    raw_key.to_ascii_lowercase()
                ))?;
            out.push_str(value);
            rest = &after_open[end + 1..];
        } else {
            // Unclosed `${ENV.…}` — pass the rest through literally
            // (defensive against user typos; the exchange will fail
            // downstream with a clear vendor error).
            out.push_str(&rest[start..]);
            break;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Flatten a JSON object into form-urlencoded key=value pairs. Only
/// top-level scalar fields are encoded — nested objects produce an
/// error so the spec author knows they're using the wrong body_format.
fn flatten_form_pairs(
    value: &serde_json::Value,
) -> Result<Vec<(String, String)>, String> {
    let map = value.as_object()
        .ok_or_else(|| "FormUrlEncoded body must be a JSON object at the top level".to_string())?;
    let mut pairs = Vec::with_capacity(map.len());
    for (k, v) in map {
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => return Err(format!(
                "FormUrlEncoded body cannot encode nested value at key `{k}` — switch to body_format: Json or flatten the field"
            )),
        };
        pairs.push((k.clone(), s));
    }
    Ok(pairs)
}

/// Extract a token via JSONPath. Falls back to a minimal hand-parser
/// for the `$.foo.bar` dotted-path shape so we don't pull in
/// `serde_json_path` just for this. Bracket / wildcard / filter syntax
/// is NOT supported — token paths in practice are always dotted scalars.
fn extract_token_jsonpath(
    response: &serde_json::Value,
    path: &str,
) -> Result<String, String> {
    let trimmed = path.trim();
    let dotted = trimmed.strip_prefix("$.").unwrap_or(trimmed.strip_prefix('$').unwrap_or(trimmed));
    let mut cur = response;
    for segment in dotted.split('.') {
        if segment.is_empty() { continue; }
        cur = cur.get(segment)
            .ok_or_else(|| format!("JSONPath `{path}` — segment `{segment}` not found"))?;
    }
    cur.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("JSONPath `{path}` — value is not a string"))
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

    // ─── substitute_env_in_string (0.8.6) ──────────────────────────
    //
    // Regression guards for the bug that broke Didomi live on 2026-05-20:
    // the agent wrote `${env.organization_id}` (lowercase) and the
    // substitution was uppercase-only → the literal `${env.…}` was
    // percent-encoded into the URL and the API returned a misleading
    // 404. The fix is case-insensitive on BOTH the `env.` prefix and
    // the KEY (the env HashMap is stored UPPER_SNAKE per slug_env_key).

    #[test]
    fn substitute_env_lowercase_prefix_and_key_resolve() {
        let mut env = HashMap::new();
        env.insert("ORGANIZATION_ID".into(), "euronews".into());
        let out = substitute_env_in_string("?organization_id=${env.organization_id}", &env).unwrap();
        assert_eq!(out, "?organization_id=euronews");
    }

    #[test]
    fn substitute_env_uppercase_prefix_and_key_resolve() {
        let mut env = HashMap::new();
        env.insert("ORGANIZATION_ID".into(), "euronews".into());
        let out = substitute_env_in_string("?organization_id=${ENV.ORGANIZATION_ID}", &env).unwrap();
        assert_eq!(out, "?organization_id=euronews");
    }

    #[test]
    fn substitute_env_mixed_case_prefix_still_resolves() {
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "k-123".into());
        // `${Env.api_KEY}` — both prefix and key are mixed case.
        let out = substitute_env_in_string("Bearer ${Env.api_KEY}", &env).unwrap();
        assert_eq!(out, "Bearer k-123");
    }

    #[test]
    fn substitute_env_multiple_placeholders_in_one_string() {
        let mut env = HashMap::new();
        env.insert("A".into(), "alpha".into());
        env.insert("B".into(), "beta".into());
        let out = substitute_env_in_string("/${ENV.A}/x/${env.b}/y", &env).unwrap();
        assert_eq!(out, "/alpha/x/beta/y");
    }

    #[test]
    fn substitute_env_missing_var_returns_error_naming_key() {
        let env = HashMap::new();
        let err = substitute_env_in_string("${ENV.SECRET}", &env).unwrap_err();
        assert!(err.contains("SECRET"), "error must name the missing key: {}", err);
    }

    #[test]
    fn substitute_env_unclosed_brace_passes_through_literally() {
        // Defensive: a typo `${ENV.FOO` (no `}`) shouldn't crash the
        // substitution. The downstream HTTP call will fail with a clear
        // vendor error anyway.
        let env = HashMap::new();
        let out = substitute_env_in_string("before ${ENV.FOO bar", &env).unwrap();
        assert!(out.contains("${ENV.FOO"), "expected literal passthrough, got: {}", out);
    }

    #[test]
    fn substitute_env_no_placeholders_is_identity() {
        let env = HashMap::new();
        let out = substitute_env_in_string("plain string", &env).unwrap();
        assert_eq!(out, "plain string");
    }

    // ─── flatten_form_pairs (0.8.6 — TokenExchange helper) ─────────

    #[test]
    fn flatten_form_pairs_encodes_scalar_string_fields() {
        let v = serde_json::json!({
            "client_id": "abc",
            "client_secret": "xyz",
        });
        let pairs = flatten_form_pairs(&v).unwrap();
        // Order isn't part of the contract — sort for deterministic compare.
        let mut got: Vec<(String, String)> = pairs;
        got.sort();
        assert_eq!(got, vec![
            ("client_id".to_string(), "abc".to_string()),
            ("client_secret".to_string(), "xyz".to_string()),
        ]);
    }

    #[test]
    fn flatten_form_pairs_stringifies_numbers_and_bools() {
        let v = serde_json::json!({ "n": 42, "live": true });
        let pairs = flatten_form_pairs(&v).unwrap();
        let mut got: Vec<(String, String)> = pairs;
        got.sort();
        assert_eq!(got, vec![
            ("live".to_string(), "true".to_string()),
            ("n".to_string(), "42".to_string()),
        ]);
    }

    #[test]
    fn flatten_form_pairs_rejects_nested_object() {
        let v = serde_json::json!({ "creds": { "id": "a", "secret": "b" } });
        let err = flatten_form_pairs(&v).unwrap_err();
        assert!(err.contains("creds"), "error must name the offending key: {}", err);
        assert!(err.contains("Json"), "error must point to Json as the fix: {}", err);
    }

    #[test]
    fn flatten_form_pairs_rejects_non_object_top_level() {
        let v = serde_json::json!(["a", "b"]);
        let err = flatten_form_pairs(&v).unwrap_err();
        assert!(err.contains("object at the top level"));
    }

    // ─── extract_token_jsonpath (0.8.6) ────────────────────────────

    #[test]
    fn extract_token_jsonpath_dotted_root_key() {
        let v = serde_json::json!({ "access_token": "tok-123" });
        let got = extract_token_jsonpath(&v, "$.access_token").unwrap();
        assert_eq!(got, "tok-123");
    }

    #[test]
    fn extract_token_jsonpath_nested_path() {
        let v = serde_json::json!({ "data": { "token": "deep-tok" } });
        let got = extract_token_jsonpath(&v, "$.data.token").unwrap();
        assert_eq!(got, "deep-tok");
    }

    #[test]
    fn extract_token_jsonpath_without_dollar_prefix_still_works() {
        // Some operators write `access_token` (no `$.`). We accept it.
        let v = serde_json::json!({ "access_token": "raw" });
        let got = extract_token_jsonpath(&v, "access_token").unwrap();
        assert_eq!(got, "raw");
    }

    #[test]
    fn extract_token_jsonpath_missing_segment_errors_with_path() {
        let v = serde_json::json!({ "access_token": "tok" });
        let err = extract_token_jsonpath(&v, "$.data.token").unwrap_err();
        assert!(err.contains("data"), "must name the missing segment: {}", err);
    }

    #[test]
    fn extract_token_jsonpath_non_string_value_errors_clearly() {
        let v = serde_json::json!({ "access_token": 42 });
        let err = extract_token_jsonpath(&v, "$.access_token").unwrap_err();
        assert!(err.contains("not a string"));
    }

    // ─── resolve_token_exchange (0.8.6) — end-to-end via wiremock ──

    #[tokio::test]
    async fn token_exchange_cache_hit_returns_without_http() {
        // Token URL points at a closed port — if the cache short-circuit
        // is broken the test fails immediately on the HTTP attempt.
        let cache = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut g = cache.lock().await;
            g.insert(
                "didomi-cfg".into(),
                CachedToken {
                    access_token: "cached-tx".into(),
                    refresh_at: Instant::now() + Duration::from_secs(300),
                },
            );
        }
        let auth = ApiAuthKind::TokenExchange {
            endpoint: "/sessions".into(),
            method: "POST".into(),
            body_template: serde_json::json!({"type":"api-key"}),
            body_format: TokenExchangeBodyFormat::Json,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 3600,
            inject: crate::models::TokenInjection::BearerHeader,
            creds_env_keys: vec![],
        };
        let env = HashMap::new();
        let tok = resolve_token_exchange(
            &cache,
            "didomi-cfg",
            &auth,
            "http://127.0.0.1:1/unused",
            &env,
        ).await.unwrap();
        assert_eq!(tok, "cached-tx");
    }

    #[tokio::test]
    async fn token_exchange_wrong_auth_kind_typed_error() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let env = HashMap::new();
        let non_tx = ApiAuthKind::Bearer { env_key: "X".into() };
        let err = resolve_token_exchange(&cache, "cfg", &non_tx, "http://x", &env).await.unwrap_err();
        assert!(err.contains("non-TokenExchange"));
    }

    #[tokio::test]
    async fn token_exchange_missing_env_var_in_body_template() {
        // body_template references ${ENV.API_KEY} but env is empty —
        // substitute_env_in_value must bubble the missing-var error.
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let auth = ApiAuthKind::TokenExchange {
            endpoint: "/sessions".into(),
            method: "POST".into(),
            body_template: serde_json::json!({"type":"api-key","key":"${ENV.API_KEY}"}),
            body_format: TokenExchangeBodyFormat::Json,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 3600,
            inject: crate::models::TokenInjection::BearerHeader,
            creds_env_keys: vec![],
        };
        let env = HashMap::new(); // no API_KEY
        let err = resolve_token_exchange(&cache, "cfg", &auth, "http://x", &env).await.unwrap_err();
        assert!(err.contains("API_KEY"));
    }

    #[tokio::test]
    async fn token_exchange_json_body_extracts_token_from_response() {
        use wiremock::matchers::{method as wm_method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(wm_method("POST"))
            .and(wm_path("/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-tx-123",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let cache = Arc::new(Mutex::new(HashMap::new()));
        let auth = ApiAuthKind::TokenExchange {
            endpoint: "/sessions".into(),
            method: "POST".into(),
            body_template: serde_json::json!({
                "type": "api-key",
                "key": "${ENV.API_KEY}",
                "secret": "${ENV.API_SECRET}"
            }),
            body_format: TokenExchangeBodyFormat::Json,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 3600,
            inject: crate::models::TokenInjection::BearerHeader,
            creds_env_keys: vec![],
        };
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "kid".into());
        env.insert("API_SECRET".into(), "ksecret".into());

        let tok = resolve_token_exchange(
            &cache,
            "cfg-tx",
            &auth,
            &server.uri(),
            &env,
        ).await.unwrap();
        assert_eq!(tok, "fresh-tx-123");

        // Cached for next call.
        let cached = {
            let g = cache.lock().await;
            g.get("cfg-tx").cloned()
        }.expect("token must be cached");
        assert_eq!(cached.access_token, "fresh-tx-123");
        assert!(cached.refresh_at > Instant::now() + Duration::from_secs(3000));
    }

    #[tokio::test]
    async fn token_exchange_form_urlencoded_body_works() {
        use wiremock::matchers::{body_string_contains, header, method as wm_method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(wm_method("POST"))
            .and(wm_path("/oauth/token"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("grant_type=client_credentials"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "form-tok"
            })))
            .mount(&server)
            .await;

        let cache = Arc::new(Mutex::new(HashMap::new()));
        let auth = ApiAuthKind::TokenExchange {
            endpoint: "/oauth/token".into(),
            method: "POST".into(),
            body_template: serde_json::json!({
                "grant_type": "client_credentials",
                "client_id": "${ENV.CID}",
            }),
            body_format: TokenExchangeBodyFormat::FormUrlEncoded,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 0, // skip cache so we re-mint
            inject: crate::models::TokenInjection::BearerHeader,
            creds_env_keys: vec![],
        };
        let mut env = HashMap::new();
        env.insert("CID".into(), "client-42".into());

        let tok = resolve_token_exchange(
            &cache,
            "cfg-form",
            &auth,
            &server.uri(),
            &env,
        ).await.unwrap();
        assert_eq!(tok, "form-tok");
    }

    #[tokio::test]
    async fn token_exchange_non_2xx_surfaces_error_with_status() {
        use wiremock::matchers::{method as wm_method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(wm_method("POST"))
            .and(wm_path("/sessions"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"invalid_key"}"#))
            .mount(&server)
            .await;

        let cache = Arc::new(Mutex::new(HashMap::new()));
        let auth = ApiAuthKind::TokenExchange {
            endpoint: "/sessions".into(),
            method: "POST".into(),
            body_template: serde_json::json!({"type":"api-key"}),
            body_format: TokenExchangeBodyFormat::Json,
            token_jsonpath: "$.access_token".into(),
            ttl_seconds: 0,
            inject: crate::models::TokenInjection::BearerHeader,
            creds_env_keys: vec![],
        };
        let env = HashMap::new();

        let err = resolve_token_exchange(
            &cache,
            "cfg-bad",
            &auth,
            &server.uri(),
            &env,
        ).await.unwrap_err();
        assert!(err.contains("401"), "error must surface status: {}", err);
        assert!(err.contains("invalid_key"), "error must include body excerpt: {}", err);
    }
}
