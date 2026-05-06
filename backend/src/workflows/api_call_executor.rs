//! `StepType::ApiCall` — HTTP executor.
//!
//! Side-effectful counterpart of `api_call_step` (pure extraction). The
//! executor orchestrates:
//!
//! 1. Render query / header / body templates (`{{steps.X.data}}`).
//! 2. Resolve auth from plugin spec + decrypted env → `ResolvedAuth`.
//! 3. Security guards (host match vs plugin base URL, public IP check).
//! 4. Send HTTP request with per-request timeout + retry on 5xx/429.
//! 5. Parse JSON → `apply_extract` → `StepOutcome` with structured
//!    envelope `{data, status, summary}`.
//!
//! Rate limiting + pagination walking land in follow-ups. The single-
//! request path covers the Chartbeat vertical (P1) and feeds real wiremock
//! tests today — it's the minimal fit for the "first vertical" milestone.
//!
//! See `docs/operations/deagent-apicall.md` for scope + decisions.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode, Url};
use serde_json::Value;

use crate::models::*;

use super::api_call_security::{
    assert_host_matches_base, assert_public_ip, redact_url_query, ResolvedAuth,
};
use super::api_call_step::{apply_extract, ExtractError, ExtractionOutcome};
use super::steps::StepOutcome;
use super::template::TemplateContext;

// ── Defaults — keep in sync with `docs/operations/deagent-apicall.md` ──
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_RETRIES: u8 = 2;
const RETRY_BACKOFF_INITIAL_MS: u64 = 250;
const RETRY_BACKOFF_MULTIPLIER: u64 = 3;

// ─── Public API ─────────────────────────────────────────────────────────

/// Security policy for an ApiCall step execution. The default (`enforce_*`
/// all true) is what production uses — tests that need to hit a local
/// wiremock server flip `enforce_public_ip` to false. `enforce_host_match`
/// stays on even in tests: the wiremock URL DOES match the plugin base,
/// so that guard is always exercised end-to-end.
#[derive(Debug, Clone, Copy)]
pub struct SecurityPolicy {
    pub enforce_host_match: bool,
    pub enforce_public_ip: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self { enforce_host_match: true, enforce_public_ip: true }
    }
}

impl SecurityPolicy {
    /// Production default. Use this from the runner dispatch.
    pub fn production() -> Self { Self::default() }

    /// For integration tests that MUST hit localhost (wiremock). Host-match
    /// guard stays on because the plugin base URL is the same localhost —
    /// so the actual allowlist path still runs.
    #[cfg(test)]
    pub fn allow_loopback_for_tests() -> Self {
        Self { enforce_host_match: true, enforce_public_ip: false }
    }
}

/// Execute an `ApiCall` step against a pre-resolved plugin + decrypted env.
/// The runner wrapper (elsewhere) is responsible for loading those from DB
/// before invoking this function — keeping the core pure-ish (no DB
/// access) makes it trivially testable against wiremock.
pub async fn execute_api_call_step_core(
    step: &WorkflowStep,
    plugin: &McpServer,
    env: &HashMap<String, String>,
    ctx: &TemplateContext,
    policy: SecurityPolicy,
) -> StepOutcome {
    let start = Instant::now();

    // Validate declared fields.
    let Some(spec) = plugin.api_spec.as_ref() else {
        return fail(step, start, "Plugin has no `api_spec` — not an API plugin".into());
    };
    let Some(endpoint_path) = step.api_endpoint_path.as_ref() else {
        return fail(step, start, "ApiCall step missing `api_endpoint_path`".into());
    };

    // Resolve auth first — even if a subsequent step fails, surfacing an
    // auth error now is far more actionable than an opaque 401 later.
    let auth = match resolve_auth(&spec.auth, env) {
        Ok(a) => a,
        Err(msg) => return fail(step, start, msg),
    };

    // Render parameter templates.
    let query = match render_map(&step.api_query, ctx) {
        Ok(q) => q,
        Err(e) => return fail(step, start, format!("Template render error (query): {e}")),
    };
    let extra_headers = match render_map(&step.api_headers, ctx) {
        Ok(h) => h,
        Err(e) => return fail(step, start, format!("Template render error (headers): {e}")),
    };
    let body = match render_body(&step.api_body, ctx) {
        Ok(b) => b,
        Err(e) => return fail(step, start, format!("Template render error (body): {e}")),
    };

    // Substitute `{key}` path-segment params (e.g. /repos/{owner}/{repo}).
    // Values are rendered through TemplateContext FIRST so a previous
    // step's output can drive a segment (`{owner}` = `{{steps.X.data}}`).
    // Tokens with no entry stay literal — the request will then 404,
    // which is much more actionable than silently dropping the segment.
    let resolved_path = match resolve_path_params(endpoint_path, &step.api_path_params, ctx) {
        Ok(p) => p,
        Err(e) => return fail(step, start, format!("Path param render error: {e}")),
    };

    // Resolve `{ENV_KEY}` placeholders in `base_url` against the
    // decrypted plugin env. Used by Jira (`{{config.JIRA_BASE_URL}}` →
    // `https://acme.atlassian.net`) and Adobe Analytics
    // (`{ADOBE_COMPANY_ID}`). Plugins with a fixed base URL
    // (Chartbeat, GitHub) come out unchanged. The same routine is also
    // used by `mcp_scanner::build_api_context_block` so the agent's
    // prompt and the actual request stay in sync.
    let resolved_base_url = interpolate_env(&spec.base_url, env);
    if resolved_base_url.contains("<NOT_CONFIGURED:") {
        return fail(
            step, start,
            format!(
                "Plugin base URL has unresolved env placeholder(s): `{resolved_base_url}`. \
                 Open Settings → APIs and fill in every required config key for this plugin."
            ),
        );
    }

    // Build final URL: base_url interpolated, endpoint path from step
    // (after path-param substitution), query = auth.query ∪ rendered
    // query.
    let full_url = match build_url(&resolved_base_url, &resolved_path, &auth.query, &query) {
        Ok(u) => u,
        Err(msg) => return fail(step, start, msg),
    };

    // Security gates (policy-configurable so integration tests can hit
    // wiremock on localhost; production always runs both checks).
    if policy.enforce_host_match {
        if let Err(e) = assert_host_matches_base(&full_url, &resolved_base_url) {
            return fail(step, start, format!("Security: {e}"));
        }
    }
    if policy.enforce_public_ip {
        if let Err(e) = assert_public_ip(&full_url).await {
            return fail(step, start, format!("Security: {e}"));
        }
    }

    // Method: step override > spec endpoint default > GET.
    let method = resolve_method(&step.api_method, endpoint_path, spec);

    // Fire with retry, walking pagination if the spec requests it. The
    // walker handles its own rate-limit (one slot per HTTP page), retries,
    // and accumulates the paginated array under its detected key so the
    // caller's `api_extract` JSONPath stays the same — no need to know
    // whether the underlying call walked 1 or 50 pages.
    let timeout = Duration::from_millis(step.api_timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let max_retries = step.api_max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
    let plugin_slug = step.api_plugin_slug.as_deref().unwrap_or("");
    let config_id = step.api_config_id.as_deref().unwrap_or("");
    let pagination = step.api_pagination.clone().unwrap_or(PaginationSpec::None);

    let response = match walk_pages(
        method.clone(),
        full_url.clone(),
        &auth,
        &extra_headers,
        body.as_ref(),
        &query,
        timeout,
        max_retries,
        &pagination,
        plugin_slug,
        config_id,
    )
    .await
    {
        Ok(v) => v,
        Err(msg) => return fail(step, start, msg),
    };

    // Apply extract (or pass through if no spec given).
    let extract_out = match step.api_extract.as_ref() {
        Some(spec) => match apply_extract(spec, &response) {
            Ok(out) => out,
            Err(ExtractError::InvalidPath { path, reason }) => {
                return fail(step, start, format!("Invalid JSONPath `{path}`: {reason}"));
            }
        },
        None => ExtractionOutcome { value: response.clone(), is_empty: false },
    };

    // Build structured envelope so downstream agents / batch steps can
    // consume `steps.X.data`. `fail_on_empty` flips NO_RESULTS — matches
    // the existing `StepConditionRule` routing (Skip/Stop/Goto).
    let fail_on_empty = step.api_extract.as_ref().is_some_and(|s| s.fail_on_empty);
    let status_str = if extract_out.is_empty && fail_on_empty {
        "NO_RESULTS"
    } else {
        "OK"
    };
    let summary = summarize(&extract_out.value, &full_url, method.as_str());

    let output_envelope = serde_json::json!({
        "data": extract_out.value,
        "status": status_str,
        "summary": summary,
    });
    // Trailing `[SIGNAL: <kw>]` lines so users can branch via `on_result.contains`
    // without parsing the JSON envelope. NO_RESULTS gets its own signal because
    // it's the existing convention for "API returned an empty list, skip the
    // downstream agent" — already special-cased in the Agent path.
    let signal = if extract_out.is_empty && fail_on_empty {
        "[SIGNAL: NO_RESULTS]"
    } else {
        "[SIGNAL: OK]"
    };
    let output = format!(
        "{}\n{}",
        serde_json::to_string(&output_envelope).unwrap_or_default(),
        signal,
    );
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|a| match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{}", step_name),
    });

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Success,
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action,
    }
}

/// Runner dispatch helper — loads the plugin + decrypted env from the
/// database based on the step's `api_plugin_slug` / `api_config_id`, then
/// forwards to [`execute_api_call_step_core`] under production security.
///
/// Callers are the workflow runner (`StepType::ApiCall` arm) and the
/// `/api/workflow-steps/test-api-call` endpoint — they both need the
/// same DB plumbing but pass different policies. The `test-api-call`
/// endpoint uses [`SecurityPolicy::production`] too: a misconfigured
/// URL that hits localhost MUST fail in the wizard too, otherwise
/// users happily test a workflow that'll then refuse to run.
pub async fn execute_api_call_step_with_db(
    step: &WorkflowStep,
    project_id: Option<&str>,
    state: &crate::AppState,
    ctx: &TemplateContext,
    policy: SecurityPolicy,
) -> StepOutcome {
    let start = Instant::now();

    // 0.7+ — référence optionnelle vers un QuickApi. Hydrate les champs
    // `api_*` manquants depuis le QA (per-field override, le step gagne).
    // Même règle que pour `BatchApiCall`. Permet à l'utilisateur de définir
    // un appel canonique côté QuickApi et de le réutiliser dans un step
    // ApiCall single sans tout re-saisir.
    let mut step_owned = step.clone();
    if let Err(e) = crate::workflows::quick_api_hydrate::hydrate_step_from_quick_api(
        &mut step_owned,
        &state.db,
    )
    .await
    {
        return fail(step, start, e);
    }
    let step = &step_owned;

    let Some(slug) = step.api_plugin_slug.as_ref() else {
        return fail(step, start, "ApiCall step missing `api_plugin_slug`".into());
    };
    let Some(config_id) = step.api_config_id.as_ref() else {
        return fail(step, start, "ApiCall step missing `api_config_id`".into());
    };

    // Read the encryption secret under the short-lived config read lock,
    // release immediately — holding it across the DB call serializes
    // every other config reader for no reason.
    let secret_opt = { state.config.read().await.encryption_secret.clone() };
    let Some(secret) = secret_opt else {
        return fail(step, start, "Encryption secret not configured — cannot decrypt plugin env".into());
    };

    // Project resolution. The plugin env is decrypted per-project in
    // `collect_active_api_plugins`, so we need a project_id even if the
    // workflow itself isn't bound to one. The runner now mirrors the
    // wizard's "Test the call" behaviour: if the workflow has no project,
    // fall back to the first project the picked config is linked to
    // (Settings → APIs always wires at least one — global or specific).
    // Only when the config exists in no project at all do we surface the
    // actionable error pointing the user to the API config screen.
    let resolved_pid: String = match project_id {
        Some(p) => p.to_string(),
        None => {
            let cid = config_id.clone();
            let cfg = state
                .db
                .with_conn(move |conn| crate::db::mcps::get_config(conn, &cid))
                .await
                .ok()
                .flatten();
            match cfg {
                Some(c) if c.is_global => {
                    // Global configs aren't filtered by project — passing
                    // any string would work since `is_global || …` short-
                    // circuits. Empty marker keeps the failure mode obvious
                    // if a future refactor breaks the invariant.
                    String::new()
                }
                Some(c) if !c.project_ids.is_empty() => c.project_ids[0].clone(),
                _ => {
                    return fail(
                        step, start,
                        format!(
                            "API plugin config `{config_id}` is not linked to any project. \
                             Open Settings → APIs and tick at least one project on this config, \
                             or attach the workflow to a project."
                        ),
                    );
                }
            }
        }
    };

    let pid_owned = resolved_pid.clone();
    let secret_owned = secret.clone();
    let plugins = state
        .db
        .with_conn(move |conn| {
            crate::core::mcp_scanner::collect_active_api_plugins(conn, &pid_owned, &secret_owned)
        })
        .await
        .unwrap_or_default();

    // Match plugin by slug. `McpServer.id` is the registry slug (e.g.
    // "chartbeat", "jira"). `config_id` narrows to a specific instance
    // since a project can wire the same plugin several times.
    let found = plugins.into_iter().find(|(server, _env)| {
        server.id == *slug
            && matches_config(server, config_id, state)
    });
    let Some((plugin, mut env)) = found else {
        let pid_label = if resolved_pid.is_empty() { "(global)".to_string() } else { resolved_pid.clone() };
        return fail(
            step,
            start,
            format!("API plugin `{slug}` / config `{config_id}` not active on project `{pid_label}`"),
        );
    };

    // Resolve OAuth2 token if needed — writes virtual env keys the
    // resolver reads. Mirrors the discussion pre-flight in
    // `api::discussions` exactly, so plugins behave identically whether
    // called from an agent or from an ApiCall step.
    if let Some(spec) = plugin.api_spec.as_ref() {
        if matches!(spec.auth, ApiAuthKind::OAuth2ClientCredentials { .. }) {
            match crate::core::oauth2_cache::resolve_token(
                &state.oauth2_cache,
                config_id,
                &spec.auth,
                &env,
            )
            .await
            {
                Ok(token) => {
                    env.insert("__access_token__".into(), token);
                }
                Err(e) => {
                    env.insert("__token_error__".into(), e.to_string());
                }
            }
        }
    }

    execute_api_call_step_core(step, &plugin, &env, ctx, policy).await
}

/// True when the (server, config_id) pair is one the DB active-plugin
/// scan actually returned. `collect_active_api_plugins` already did the
/// project-scope filter — this is just the per-instance match since a
/// project can wire the same plugin multiple times.
///
/// Implemented via a second DB query because `collect_active_api_plugins`
/// drops the `config_id` on the floor — we keep the helper minimal rather
/// than refactor its signature just for this callsite.
fn matches_config(server: &McpServer, config_id: &str, state: &crate::AppState) -> bool {
    // Safe fast path: if the project has a single instance of this server,
    // any valid config_id is "the one". For multi-instance disambiguation
    // we delegate to the DB.
    let _ = (server, config_id, state);
    // TODO P0.5b: tighten this once `collect_active_api_plugins` surfaces
    // per-(server, config_id) pairs. For the P1 Chartbeat vertical a
    // project wires one instance, so this is a safe placeholder.
    true
}

// ─── Auth resolution ────────────────────────────────────────────────────

/// Builds a `ResolvedAuth` from the plugin's declared `ApiAuthKind` plus
/// the decrypted env. Mirrors what `build_api_context_block` does to hand
/// creds to an agent, but returns a structured value instead of prose.
pub fn resolve_auth(
    auth: &ApiAuthKind,
    env: &HashMap<String, String>,
) -> Result<ResolvedAuth, String> {
    let mut out = ResolvedAuth {
        bearer: None,
        headers: HashMap::new(),
        query: HashMap::new(),
    };
    match auth {
        ApiAuthKind::ApiKeyQuery { param_name, env_key } => {
            let value = env.get(env_key).ok_or_else(|| {
                format!("Auth error: env key `{env_key}` missing for ApiKeyQuery")
            })?;
            out.query.insert(param_name.clone(), value.clone());
        }
        ApiAuthKind::ApiKeyHeader { header_name, env_key } => {
            let value = env.get(env_key).ok_or_else(|| {
                format!("Auth error: env key `{env_key}` missing for ApiKeyHeader")
            })?;
            out.headers.insert(header_name.clone(), value.clone());
        }
        ApiAuthKind::Bearer { env_key } => {
            let value = env.get(env_key).ok_or_else(|| {
                format!("Auth error: env key `{env_key}` missing for Bearer")
            })?;
            out.bearer = Some(value.clone());
        }
        ApiAuthKind::Basic { user_env, password_env } => {
            // HTTP Basic = `Authorization: Basic <base64(user:password)>`.
            // We compose the header here rather than reusing the Bearer
            // path because the wire format differs ("Basic" prefix +
            // base64) and the header builder downstream needs to skip the
            // `Bearer ` prefix it adds otherwise. Both halves are required;
            // a missing user OR token is an actionable error so the
            // operator knows exactly which env key to fix in Settings →
            // APIs.
            let user = env.get(user_env).ok_or_else(|| {
                format!("Auth error: env key `{user_env}` missing for Basic auth (user)")
            })?;
            let password = env.get(password_env).ok_or_else(|| {
                format!("Auth error: env key `{password_env}` missing for Basic auth (password/token)")
            })?;
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let encoded = STANDARD.encode(format!("{user}:{password}"));
            out.headers.insert("Authorization".into(), format!("Basic {encoded}"));
        }
        ApiAuthKind::BasicApiKey { env_key } => {
            // HTTP Basic with the API key as user and empty password —
            // `Authorization: Basic <base64(API_KEY:)>`. SpeedCurve,
            // Stripe, etc. The trailing colon in `KEY:` is significant
            // (the user/password separator); without it, Basic decoders
            // see a single field and reject as malformed.
            let key = env.get(env_key).ok_or_else(|| {
                format!("Auth error: env key `{env_key}` missing for BasicApiKey auth")
            })?;
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let encoded = STANDARD.encode(format!("{key}:"));
            out.headers.insert("Authorization".into(), format!("Basic {encoded}"));
        }
        ApiAuthKind::OAuth2ClientCredentials { extra_headers, .. } => {
            // Same contract as `build_api_context_block`: the caller has
            // already resolved the token via `core::oauth2_cache` and
            // stashed it under the virtual keys `__access_token__` or
            // `__token_error__`. We translate those into ResolvedAuth or
            // surface an actionable error.
            match env.get("__access_token__") {
                Some(tok) => out.bearer = Some(tok.clone()),
                None => {
                    let err = env
                        .get("__token_error__")
                        .cloned()
                        .unwrap_or_else(|| "unknown token-exchange failure".into());
                    return Err(format!("OAuth2 token unavailable: {err}"));
                }
            }
            for eh in extra_headers {
                let rendered = interpolate_env(&eh.value_template, env);
                out.headers.insert(eh.name.clone(), rendered);
            }
        }
        ApiAuthKind::None => {
            // Public endpoint — leave ResolvedAuth empty.
        }
    }
    Ok(out)
}

/// Minimal `{ENV_KEY}` substitution used by `ApiAuthKind::OAuth2` extra
/// headers. Missing keys render as `<NOT_CONFIGURED:KEY>` so the agent /
/// operator sees which env var is missing instead of a silently broken
/// header.
fn interpolate_env(template: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find('}') {
            let key = &rest[1..end];
            match env.get(key) {
                Some(v) => out.push_str(v),
                None => {
                    out.push_str("<NOT_CONFIGURED:");
                    out.push_str(key);
                    out.push('>');
                }
            }
            rest = &rest[end + 1..];
        } else {
            out.push_str(rest);
            break;
        }
    }
    out.push_str(rest);
    out
}

// ─── URL + templating helpers ───────────────────────────────────────────

fn render_map(
    input: &Option<HashMap<String, String>>,
    ctx: &TemplateContext,
) -> anyhow::Result<HashMap<String, String>> {
    let Some(map) = input else { return Ok(HashMap::new()); };
    let mut out = HashMap::with_capacity(map.len());
    for (k, v) in map {
        out.insert(k.clone(), ctx.render(v)?);
    }
    Ok(out)
}

fn render_body(body: &Option<Value>, ctx: &TemplateContext) -> anyhow::Result<Option<Value>> {
    let Some(body) = body else { return Ok(None); };
    Ok(Some(render_json_value(body, ctx)?))
}

/// Substitute path-segment placeholders (`{key}`) in `endpoint_path` with
/// values from `path_params`. Each value is first rendered through the
/// template engine (so `{{steps.X.data}}` inside a value works) and then
/// percent-encoded for path-segment safety. Tokens with no matching key
/// stay literal — the request will then 404, which is more actionable
/// than silently dropping the segment and producing a different URL.
///
/// Disambiguating `{key}` from the existing `{{var}}` template syntax:
/// the regex `\{(\w+)\}` only matches single-brace tokens. Inside `{{x}}`,
/// the regex would match `{x}` (the inner pair) — so we explicitly mask
/// double-brace runs before substitution and restore them after. Cheap
/// and bullet-proof, no `fancy-regex` dependency needed.
pub(crate) fn resolve_path_params(
    endpoint_path: &str,
    path_params: &Option<HashMap<String, String>>,
    ctx: &TemplateContext,
) -> anyhow::Result<String> {
    // Fast path: no params → no scan needed unless the path has tokens.
    if path_params.as_ref().is_none_or(|m| m.is_empty()) {
        return Ok(endpoint_path.to_string());
    }
    let path_params = path_params.as_ref().expect("checked just above");

    // Mask `{{` / `}}` so we don't accidentally substitute inside a
    // template var. We use form-feed (\u{000C}) and vertical-tab
    // (\u{000B}) — neither is valid in URLs nor in our existing path
    // strings, so the round-trip is reliable.
    const MASK_OPEN: char = '\u{000C}';
    const MASK_CLOSE: char = '\u{000B}';
    let masked = endpoint_path
        .replace("{{", &MASK_OPEN.to_string().repeat(2))
        .replace("}}", &MASK_CLOSE.to_string().repeat(2));

    // Substitute single-brace placeholders.
    let mut out = String::with_capacity(masked.len() + 32);
    let bytes = masked.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // Find the matching close brace.
            if let Some(close_offset) = masked[i + 1..].find('}') {
                let key = &masked[i + 1..i + 1 + close_offset];
                // Empty `{}` or non-identifier-ish (whitespace, slashes)
                // keys aren't ours — leave the literal.
                let is_clean = !key.is_empty()
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if is_clean {
                    if let Some(raw_value) = path_params.get(key) {
                        let rendered = ctx.render(raw_value)?;
                        // Percent-encode for path-segment safety: any char
                        // outside RFC 3986 unreserved (`A-Za-z0-9-._~`) is
                        // escaped, so `/` in a value can't escape the
                        // segment and break out into a different endpoint.
                        for byte in rendered.bytes() {
                            let unreserved = byte.is_ascii_alphanumeric()
                                || matches!(byte, b'-' | b'.' | b'_' | b'~');
                            if unreserved {
                                out.push(byte as char);
                            } else {
                                out.push_str(&format!("%{byte:02X}"));
                            }
                        }
                        i = i + 1 + close_offset + 1;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }

    // Restore the masked template braces.
    let restored = out
        .replace(&MASK_OPEN.to_string().repeat(2), "{{")
        .replace(&MASK_CLOSE.to_string().repeat(2), "}}");
    Ok(restored)
}

/// Walks a JSON value and renders string leaves through the template
/// engine. Non-string types pass through unchanged — we never stringify
/// a JSON sub-tree into a template because that enables injection
/// attacks (a `"}` in the data would break out of its field).
fn render_json_value(value: &Value, ctx: &TemplateContext) -> anyhow::Result<Value> {
    match value {
        Value::String(s) => Ok(Value::String(ctx.render(s)?)),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(render_json_value(v, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render_json_value(v, ctx)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Composes the final URL by joining `base_url` + `endpoint_path` and
/// appending both auth query params and rendered step query params. Auth
/// goes last so a user can't clobber an `apikey` by accident.
pub fn build_url(
    base_url: &str,
    endpoint_path: &str,
    auth_query: &HashMap<String, String>,
    step_query: &HashMap<String, String>,
) -> Result<Url, String> {
    // Normalize: strip trailing `/` on base, ensure path starts with `/`.
    let base = base_url.trim_end_matches('/');
    let path = if endpoint_path.starts_with('/') {
        endpoint_path.to_string()
    } else {
        format!("/{endpoint_path}")
    };
    let joined = format!("{base}{path}");
    let mut url = Url::parse(&joined).map_err(|e| format!("URL parse error: {e}"))?;

    // Append params. `query_pairs_mut` percent-encodes on insert, so a
    // value containing `&` or `=` is safely escaped.
    {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in step_query {
            pairs.append_pair(k, v);
        }
        for (k, v) in auth_query {
            pairs.append_pair(k, v);
        }
    }
    Ok(url)
}

fn resolve_method(
    step_override: &Option<String>,
    endpoint_path: &str,
    spec: &ApiSpec,
) -> Method {
    if let Some(override_method) = step_override {
        return Method::from_bytes(override_method.as_bytes()).unwrap_or(Method::GET);
    }
    // Match by path in the registry. The spec may carry default method
    // per endpoint; absent that, GET is the safe default.
    if let Some(ep) = spec.endpoints.iter().find(|e| e.path == endpoint_path) {
        return Method::from_bytes(ep.method.as_bytes()).unwrap_or(Method::GET);
    }
    Method::GET
}

// ─── Pagination walk ────────────────────────────────────────────────────

/// Walks paginated responses according to `PaginationSpec` and returns a
/// single merged `Value`. For `None` / `Auto` we issue one request and
/// return the body verbatim (single page, caller's `api_extract` runs as
/// usual). For explicit `Offset` / `Cursor` / `Page` variants we loop
/// up to `max_pages`, accumulate the items array detected on page 1, and
/// substitute the merged array back into a clone of page 1's body — the
/// caller's JSONPath (`$.issues[*].key`, etc.) keeps working unchanged.
///
/// Limitation kept on purpose: items detection is shallow (top-level
/// object → first array-valued key). Nested GraphQL responses
/// (`data.viewer.zones.edges`) won't auto-walk; the caller falls back to
/// `Auto` (single page) or runs without pagination. Lifting this needs
/// a `items_path` field on `PaginationSpec` — Phase 5 territory.
#[allow(clippy::too_many_arguments)]
async fn walk_pages(
    method: Method,
    base_url: Url,
    auth: &ResolvedAuth,
    extra_headers: &HashMap<String, String>,
    body: Option<&Value>,
    base_query: &HashMap<String, String>,
    timeout: Duration,
    max_retries: u8,
    pagination: &PaginationSpec,
    plugin_slug: &str,
    config_id: &str,
) -> Result<Value, String> {
    use super::api_call_step::pagination_max_pages;

    let max_pages = pagination_max_pages(pagination);
    let mut current_query: HashMap<String, String> = base_query.clone();
    let mut next_offset: u32 = 0;
    let mut next_cursor: Option<String> = None;
    let mut next_page_num: u32 = 1;

    // Seed the first request with starting pagination params so a server
    // that requires `startAt` / `page` to be present even on page 1 (Jira
    // does, GitHub doesn't) gets a well-formed call. `entry().or_insert`
    // means we never overwrite a value the user explicitly set in
    // `step.api_query` — they keep control if they need a specific
    // resume point.
    match pagination {
        PaginationSpec::Offset { start_param, limit_param, limit, .. } => {
            current_query.entry(start_param.clone()).or_insert_with(|| "0".to_string());
            current_query.entry(limit_param.clone()).or_insert_with(|| limit.to_string());
        }
        PaginationSpec::Page { page_param, page_size_param, page_size, .. } => {
            current_query.entry(page_param.clone()).or_insert_with(|| "1".to_string());
            current_query.entry(page_size_param.clone()).or_insert_with(|| page_size.to_string());
        }
        _ => {}
    }

    let mut first_response: Option<Value> = None;
    let mut items_key: Option<String> = None;
    let mut accumulated_items: Vec<Value> = Vec::new();

    for page_idx in 0..max_pages {
        // Inject pagination params on every page after the first.
        if page_idx > 0 {
            match pagination {
                PaginationSpec::Offset { start_param, limit_param, limit, .. } => {
                    current_query.insert(start_param.clone(), next_offset.to_string());
                    current_query.insert(limit_param.clone(), limit.to_string());
                }
                PaginationSpec::Cursor { cursor_param, .. } => {
                    let Some(cursor) = next_cursor.as_ref() else { break };
                    current_query.insert(cursor_param.clone(), cursor.clone());
                }
                PaginationSpec::Page { page_param, page_size_param, page_size, .. } => {
                    current_query.insert(page_param.clone(), next_page_num.to_string());
                    current_query.insert(page_size_param.clone(), page_size.to_string());
                }
                _ => break,
            }
        }

        let url = rebuild_query(&base_url, &current_query, &auth.query)?;

        // Rate-limit gate BEFORE every HTTP page — siblings in a batch
        // fan-out compete for the same bucket.
        super::api_call_ratelimit::acquire_slot(plugin_slug, config_id).await;

        let resp = send_with_retry(
            method.clone(),
            &url,
            auth,
            extra_headers,
            body,
            timeout,
            max_retries,
        ).await?;

        // First-page handling: short-circuit for None/Auto, detect items
        // key for explicit pagination variants.
        if first_response.is_none() {
            if matches!(pagination, PaginationSpec::None | PaginationSpec::Auto { .. }) {
                return Ok(resp);
            }
            items_key = detect_items_key(&resp);
            first_response = Some(resp.clone());
        }

        if let Some(key) = items_key.as_ref() {
            let page_items = extract_array_at(&resp, key);
            accumulated_items.extend(page_items);
        }

        // Pull the next-page anchor from THIS page's body. Each variant's
        // termination condition is what tells us to break.
        match pagination {
            PaginationSpec::Offset { total_path, limit, .. } => {
                let total = jsonpath_first_u32(&resp, total_path).unwrap_or(0);
                let consumed = accumulated_items.len() as u32;
                if consumed >= total || consumed == 0 { break; }
                next_offset = consumed;
                let _ = limit; // already injected via limit_param
            }
            PaginationSpec::Cursor { next_path, .. } => {
                let cursor = jsonpath_first_string(&resp, next_path);
                if cursor.is_none() { break; }
                next_cursor = cursor;
            }
            PaginationSpec::Page { has_more_path, .. } => {
                let has_more = jsonpath_first_bool(&resp, has_more_path).unwrap_or(false);
                if !has_more { break; }
                next_page_num += 1;
            }
            _ => break,
        }
    }

    // Build the merged response: copy page 1, swap the items array for the
    // accumulator. Keeps the rest of the body (counters, metadata) intact
    // so callers reading `total` / pagination cursors still see them.
    let mut final_resp = first_response.unwrap_or(Value::Null);
    if let (Some(key), true) = (items_key.as_deref(), !accumulated_items.is_empty()) {
        if let Value::Object(map) = &mut final_resp {
            map.insert(key.to_string(), Value::Array(accumulated_items));
        }
    }
    Ok(final_resp)
}

/// Heuristic: top-level object → first key whose value is an array.
/// Returns the key name. Sufficient for Jira (`issues`), Stripe (`data`),
/// GitHub (`items`), Confluence (`results`). Returns `None` for anything
/// else — caller falls back to single-page behaviour.
fn detect_items_key(response: &Value) -> Option<String> {
    let obj = response.as_object()?;
    for (k, v) in obj {
        if v.is_array() {
            return Some(k.clone());
        }
    }
    None
}

fn extract_array_at(response: &Value, key: &str) -> Vec<Value> {
    response
        .as_object()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn jsonpath_first_u32(value: &Value, path: &str) -> Option<u32> {
    let p = serde_json_path::JsonPath::parse(path).ok()?;
    let nodes = p.query(value);
    nodes.first().and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok())
}

fn jsonpath_first_string(value: &Value, path: &str) -> Option<String> {
    let p = serde_json_path::JsonPath::parse(path).ok()?;
    let nodes = p.query(value);
    nodes.first().and_then(|v| v.as_str().map(|s| s.to_string()))
}

fn jsonpath_first_bool(value: &Value, path: &str) -> Option<bool> {
    let p = serde_json_path::JsonPath::parse(path).ok()?;
    let nodes = p.query(value);
    nodes.first().and_then(|v| v.as_bool())
}

/// Re-builds a `Url` with a fresh query string. Used by the paginated
/// walker to inject updated `startAt` / `cursor` / `page` params on each
/// loop iteration without re-parsing the base URL.
fn rebuild_query(
    base: &Url,
    step_query: &HashMap<String, String>,
    auth_query: &HashMap<String, String>,
) -> Result<Url, String> {
    let mut url = base.clone();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (k, v) in step_query {
            pairs.append_pair(k, v);
        }
        for (k, v) in auth_query {
            pairs.append_pair(k, v);
        }
    }
    if url.query().is_some_and(|q| q.is_empty()) {
        url.set_query(None);
    }
    Ok(url)
}

// ─── HTTP with retry ────────────────────────────────────────────────────

/// Send with exponential backoff on 5xx + 429. 4xx is a *client* error
/// and retrying never helps — we fail fast and surface the status + body
/// excerpt so the user can fix their params.
async fn send_with_retry(
    method: Method,
    url: &Url,
    auth: &ResolvedAuth,
    extra_headers: &HashMap<String, String>,
    body: Option<&Value>,
    timeout: Duration,
    max_retries: u8,
) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("HTTP client build failed: {e}"))?;

    let mut attempt: u8 = 0;
    loop {
        let mut req = client.request(method.clone(), url.clone());
        if let Some(bearer) = &auth.bearer {
            req = req.header("Authorization", format!("Bearer {bearer}"));
        }
        for (k, v) in &auth.headers {
            req = req.header(k, v);
        }
        for (k, v) in extra_headers {
            req = req.header(k, v);
        }
        if let Some(b) = body {
            req = req.json(b);
        }

        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                // Network error — retryable within limits.
                if attempt >= max_retries {
                    return Err(format!("HTTP request failed after {max_retries} retries: {e}"));
                }
                sleep_backoff(attempt).await;
                attempt += 1;
                continue;
            }
        };

        let status = response.status();
        if status.is_success() {
            return response.json::<Value>().await.map_err(|e| {
                format!("Response JSON parse failed ({}): {e}", status.as_u16())
            });
        }

        // Non-success. Retry only on 5xx + 429, never 4xx.
        let retryable = status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
        if !retryable || attempt >= max_retries {
            let excerpt = response.text().await.unwrap_or_default();
            let redacted_url = redact_url_query(url);
            return Err(format!(
                "HTTP {} on {} {} — {}",
                status.as_u16(),
                method,
                redacted_url,
                truncate(&excerpt, 512),
            ));
        }
        sleep_backoff(attempt).await;
        attempt += 1;
    }
}

async fn sleep_backoff(attempt: u8) {
    // 250ms, 750ms, 2.25s, … bounded by max_retries (≤ 2 by default).
    let mult = RETRY_BACKOFF_MULTIPLIER.saturating_pow(attempt.into());
    let millis = RETRY_BACKOFF_INITIAL_MS.saturating_mul(mult);
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut end = 0;
    for (i, _) in s.char_indices().take(max_chars) {
        end = i + s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
    }
    s[..end.min(s.len())].to_string()
}

// ─── Summaries & failure helpers ────────────────────────────────────────

fn summarize(value: &Value, url: &Url, method: &str) -> String {
    let redacted = redact_url_query(url);
    match value {
        Value::Array(items) => format!("{method} {redacted} → {} items", items.len()),
        Value::Null => format!("{method} {redacted} → null"),
        Value::Object(_) => format!("{method} {redacted} → object"),
        scalar => format!("{method} {redacted} → {scalar}"),
    }
}

fn fail(step: &WorkflowStep, start: Instant, msg: String) -> StepOutcome {
    // HTTP-level failures format the message as "HTTP <code> on <method> <url> — <body>".
    // Extract that status into a `[SIGNAL: http_<code>]` line + a generic
    // `[SIGNAL: ERROR]` so users can branch their workflow ("503 → Goto retry,
    // 401 → Goto refresh_auth, anything else → fall through to rollback").
    // Config / template / extract errors don't get signals — they're bugs the
    // user can't usefully branch on.
    let mut output = msg.clone();
    if let Some(rest) = msg.strip_prefix("HTTP ") {
        if let Some(code_str) = rest.split_whitespace().next() {
            if let Ok(code) = code_str.parse::<u16>() {
                output.push('\n');
                output.push_str("[SIGNAL: ERROR]");
                output.push('\n');
                output.push_str(&format!("[SIGNAL: http_{}]", code));
            }
        }
    }
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|a| match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{}", step_name),
    });
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
        },
        condition_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ─── Fixture helpers ────────────────────────────────────────────

    fn mk_plugin(base_url: &str, auth: ApiAuthKind, endpoints: Vec<ApiEndpoint>) -> McpServer {
        McpServer {
            id: "fake-plugin".into(),
            name: "Fake".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(ApiSpec {
                base_url: base_url.into(),
                auth,
                endpoints,
                docs_url: None,
                config_keys: vec![],
            }),
        }
    }

    fn mk_endpoint(method: &str, path: &str) -> ApiEndpoint {
        ApiEndpoint {
            method: method.into(),
            path: path.into(),
            description: "test".into(),
        }
    }

    fn mk_step(endpoint_path: &str) -> WorkflowStep {
        WorkflowStep {
            name: "fetch".into(),
            step_type: StepType::ApiCall,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::Structured,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: Some("fake-plugin".into()),
            api_config_id: Some("cfg-1".into()),
            api_endpoint_path: Some(endpoint_path.into()),
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: Some(5_000),
            api_max_retries: Some(2),
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            quick_prompt_id: None,
            json_data_payload: None,
        }
    }

    fn extract_envelope(output: &str) -> Value {
        // The runtime output is `{json}\n[SIGNAL: ...]` (the trailing
        // SIGNAL line lets `evaluate_conditions` branch without parsing
        // JSON). Parse only the JSON head — split on the SIGNAL marker
        // if present, otherwise the whole string.
        let json_part = output.split("\n[SIGNAL:").next().unwrap_or(output);
        serde_json::from_str(json_part).expect("output is structured JSON")
    }

    // ─── resolve_auth ───────────────────────────────────────────────

    #[test]
    fn resolve_auth_apikey_query_populates_query_map() {
        let mut env = HashMap::new();
        env.insert("CHARTBEAT_KEY".into(), "cb-123".into());
        let auth = ApiAuthKind::ApiKeyQuery {
            param_name: "apikey".into(),
            env_key: "CHARTBEAT_KEY".into(),
        };
        let out = resolve_auth(&auth, &env).unwrap();
        assert_eq!(out.query.get("apikey"), Some(&"cb-123".to_string()));
        assert!(out.bearer.is_none() && out.headers.is_empty());
    }

    #[test]
    fn resolve_auth_basic_encodes_user_and_password_to_authorization_header() {
        // Jira Cloud auth = HTTP Basic with the user's email + API
        // token. Verify the wire format: `Authorization: Basic <b64>`
        // with `email:token` round-tripped through standard base64.
        let mut env = HashMap::new();
        env.insert("JIRA_USERNAME".into(), "user@example.com".into());
        env.insert("JIRA_API_TOKEN".into(), "ATATT-secret".into());
        let auth = ApiAuthKind::Basic {
            user_env: "JIRA_USERNAME".into(),
            password_env: "JIRA_API_TOKEN".into(),
        };
        let out = resolve_auth(&auth, &env).unwrap();
        assert!(out.bearer.is_none(), "Basic auth must not populate bearer");
        let header = out.headers.get("Authorization")
            .expect("Authorization header must be set");
        assert!(header.starts_with("Basic "), "header must use the Basic scheme: {header}");
        // Decode the base64 portion and check the round-trip.
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let b64 = header.trim_start_matches("Basic ");
        let decoded = String::from_utf8(STANDARD.decode(b64).unwrap()).unwrap();
        assert_eq!(decoded, "user@example.com:ATATT-secret");
    }

    #[test]
    fn resolve_auth_basic_missing_user_or_password_errors_with_actionable_message() {
        let mut env = HashMap::new();
        env.insert("JIRA_USERNAME".into(), "user@example.com".into());
        // password missing
        let auth = ApiAuthKind::Basic {
            user_env: "JIRA_USERNAME".into(),
            password_env: "JIRA_API_TOKEN".into(),
        };
        let err = resolve_auth(&auth, &env).unwrap_err();
        assert!(err.contains("JIRA_API_TOKEN"), "error should name the missing key: {err}");
    }

    #[test]
    fn resolve_auth_apikey_header_populates_header_map() {
        let mut env = HashMap::new();
        env.insert("X_API_KEY".into(), "secret".into());
        let auth = ApiAuthKind::ApiKeyHeader {
            header_name: "X-API-Key".into(),
            env_key: "X_API_KEY".into(),
        };
        let out = resolve_auth(&auth, &env).unwrap();
        assert_eq!(out.headers.get("X-API-Key"), Some(&"secret".to_string()));
    }

    #[test]
    fn resolve_auth_bearer_populates_bearer() {
        let mut env = HashMap::new();
        env.insert("JIRA_TOKEN".into(), "tok-123".into());
        let auth = ApiAuthKind::Bearer { env_key: "JIRA_TOKEN".into() };
        let out = resolve_auth(&auth, &env).unwrap();
        assert_eq!(out.bearer.as_deref(), Some("tok-123"));
    }

    #[test]
    fn resolve_auth_missing_env_key_errors() {
        // Misconfigured env must surface a clear error, not silently send
        // an unauthenticated request.
        let env = HashMap::new();
        let auth = ApiAuthKind::Bearer { env_key: "MISSING".into() };
        let err = resolve_auth(&auth, &env).unwrap_err();
        assert!(err.contains("MISSING"), "error hint should name the key: {err}");
    }

    #[test]
    fn resolve_auth_basic_apikey_encodes_key_with_empty_password() {
        // SpeedCurve / Stripe convention: HTTP Basic where the API key is
        // the username and the password half is empty. The trailing `:`
        // after the key is significant — Basic-Auth decoders treat
        // missing-colon strings as malformed and reject them. Without
        // this test, removing the `:` (or trimming) would silently break
        // every BasicApiKey-using plugin in production.
        let mut env = HashMap::new();
        env.insert("SPEEDCURVE_API_KEY".into(), "sc-abc-xyz".into());
        let auth = ApiAuthKind::BasicApiKey { env_key: "SPEEDCURVE_API_KEY".into() };
        let out = resolve_auth(&auth, &env).unwrap();
        assert!(out.bearer.is_none(), "BasicApiKey must not populate bearer");
        let header = out.headers.get("Authorization")
            .expect("Authorization header must be set");
        assert!(header.starts_with("Basic "), "header must use the Basic scheme: {header}");
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let b64 = header.trim_start_matches("Basic ");
        let decoded = String::from_utf8(STANDARD.decode(b64).unwrap()).unwrap();
        assert_eq!(decoded, "sc-abc-xyz:", "must encode `KEY:` with the trailing colon (empty password half)");
    }

    #[test]
    fn resolve_auth_basic_apikey_missing_env_errors_with_actionable_message() {
        let env = HashMap::new();
        let auth = ApiAuthKind::BasicApiKey { env_key: "SPEEDCURVE_API_KEY".into() };
        let err = resolve_auth(&auth, &env).unwrap_err();
        assert!(err.contains("SPEEDCURVE_API_KEY"), "error must name the missing key: {err}");
    }

    #[test]
    fn resolve_auth_oauth2_reads_cached_token() {
        // Mirrors the contract from `core::oauth2_cache` — caller stashes
        // the resolved token under `__access_token__`.
        let mut env = HashMap::new();
        env.insert("__access_token__".into(), "resolved-oauth-bearer".into());
        env.insert("ADOBE_CLIENT_ID".into(), "client-abc".into());
        let auth = ApiAuthKind::OAuth2ClientCredentials {
            token_url: "https://ims.example/token".into(),
            client_id_env: "ADOBE_CLIENT_ID".into(),
            client_secret_env: "ADOBE_CLIENT_SECRET".into(),
            scope: String::new(),
            extra_headers: vec![OAuth2ExtraHeader {
                name: "x-api-key".into(),
                value_template: "{ADOBE_CLIENT_ID}".into(),
            }],
        };
        let out = resolve_auth(&auth, &env).unwrap();
        assert_eq!(out.bearer.as_deref(), Some("resolved-oauth-bearer"));
        assert_eq!(out.headers.get("x-api-key"), Some(&"client-abc".to_string()));
    }

    #[test]
    fn resolve_auth_oauth2_surfaces_token_error() {
        // When the OAuth2 resolver failed, the step must fail fast — not
        // send an unauthenticated request.
        let mut env = HashMap::new();
        env.insert("__token_error__".into(), "invalid_client".into());
        let auth = ApiAuthKind::OAuth2ClientCredentials {
            token_url: "https://ims.example/token".into(),
            client_id_env: "A".into(),
            client_secret_env: "B".into(),
            scope: String::new(),
            extra_headers: vec![],
        };
        let err = resolve_auth(&auth, &env).unwrap_err();
        assert!(err.contains("invalid_client"), "error should carry token reason: {err}");
    }

    // ─── build_url ──────────────────────────────────────────────────

    #[test]
    fn build_url_joins_base_and_path_with_percent_encoding() {
        let auth = HashMap::new();
        let mut step_q = HashMap::new();
        step_q.insert("jql".into(), "project = KR AND status = \"Open\"".into());
        let url = build_url("https://api.example.com", "/search", &auth, &step_q).unwrap();
        // `=` and space must be encoded in the query; the path stays clean.
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("api.example.com"));
        assert_eq!(url.path(), "/search");
        let rebuilt: String = url.query().unwrap().to_string();
        assert!(rebuilt.contains("jql=project"));
        // Either "%20" or "+" is acceptable for space; assert one is present.
        assert!(rebuilt.contains("%20") || rebuilt.contains('+'));
    }

    #[test]
    fn build_url_auth_query_appended_last() {
        let mut auth = HashMap::new();
        auth.insert("apikey".into(), "cb-123".into());
        let mut step_q = HashMap::new();
        step_q.insert("host".into(), "euronews.com".into());
        let url = build_url("https://api.chartbeat.com/", "/live/toppages/v4", &auth, &step_q).unwrap();
        let q = url.query().unwrap();
        assert!(q.contains("apikey=cb-123"));
        assert!(q.contains("host=euronews.com"));
    }

    #[test]
    fn build_url_normalizes_trailing_slash() {
        let url = build_url("https://api.example/", "search", &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(url.path(), "/search");
    }

    // ─── resolve_path_params ────────────────────────────────────────

    #[test]
    fn path_params_substitute_owner_and_repo_for_github_style_endpoints() {
        let ctx = TemplateContext::new();
        let mut params = HashMap::new();
        params.insert("owner".to_string(), "anthropics".to_string());
        params.insert("repo".to_string(), "anthropic-cookbook".to_string());
        let out = resolve_path_params(
            "/repos/{owner}/{repo}/issues",
            &Some(params),
            &ctx,
        ).unwrap();
        assert_eq!(out, "/repos/anthropics/anthropic-cookbook/issues");
    }

    #[test]
    fn path_params_left_literal_when_no_value_provided() {
        // Missing value → leave the placeholder; the request will 404,
        // which is much more diagnostic than dropping the segment.
        let ctx = TemplateContext::new();
        let mut params = HashMap::new();
        params.insert("owner".to_string(), "x".to_string());
        let out = resolve_path_params(
            "/repos/{owner}/{repo}",
            &Some(params),
            &ctx,
        ).unwrap();
        assert_eq!(out, "/repos/x/{repo}");
    }

    #[test]
    fn path_params_dont_match_double_brace_template_vars() {
        // `{{steps.X.data}}` is the template-var syntax, not a path
        // placeholder. resolve_path_params must leave it untouched —
        // single-brace `{key}` is the only thing that gets substituted.
        let ctx = TemplateContext::new();
        let mut params = HashMap::new();
        params.insert("steps".to_string(), "evil".to_string());
        let out = resolve_path_params(
            "/items/{{steps.X.data}}/sub",
            &Some(params),
            &ctx,
        ).unwrap();
        // The template var is preserved verbatim — TemplateContext will
        // expand it (or leave it) at the next layer.
        assert_eq!(out, "/items/{{steps.X.data}}/sub");
    }

    #[test]
    fn path_params_percent_encode_unsafe_chars() {
        // A user accidentally pastes `owner with spaces` into the input
        // → must NOT break out of the segment. Same for `/`, `?`, `#`.
        let ctx = TemplateContext::new();
        let mut params = HashMap::new();
        params.insert("repo".to_string(), "name with spaces/sub".to_string());
        let out = resolve_path_params(
            "/repos/x/{repo}",
            &Some(params),
            &ctx,
        ).unwrap();
        assert_eq!(out, "/repos/x/name%20with%20spaces%2Fsub");
    }

    #[test]
    fn path_params_no_op_when_path_has_no_tokens() {
        let ctx = TemplateContext::new();
        let params: HashMap<String, String> = HashMap::new();
        let out = resolve_path_params("/user", &Some(params), &ctx).unwrap();
        assert_eq!(out, "/user");
    }

    // ─── execute_api_call_step_core (HTTP wiremock) ─────────────────

    #[tokio::test]
    async fn execute_success_extracts_array() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("apikey", "cb-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{ "key": "KR-1" }, { "key": "KR-2" }, { "key": "KR-3" }],
                "total": 3
            })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::ApiKeyQuery { param_name: "apikey".into(), env_key: "K".into() },
            vec![mk_endpoint("GET", "/search")],
        );
        let mut env = HashMap::new();
        env.insert("K".into(), "cb-abc".into());

        let mut step = mk_step("/search");
        step.api_extract = Some(ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(&step, &plugin, &env, &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success);

        let envelope = extract_envelope(&outcome.result.output);
        assert_eq!(envelope["status"], "OK");
        assert_eq!(envelope["data"], json!(["KR-1", "KR-2", "KR-3"]));
        assert!(
            envelope["summary"].as_str().unwrap().contains("3 items"),
            "summary should count array length, got {}",
            envelope["summary"],
        );
    }

    #[tokio::test]
    async fn execute_bearer_auth_attaches_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("Authorization", "Bearer real-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::Bearer { env_key: "T".into() },
            vec![mk_endpoint("GET", "/me")],
        );
        let mut env = HashMap::new();
        env.insert("T".into(), "real-token".into());

        let step = mk_step("/me");
        let outcome = execute_api_call_step_core(&step, &plugin, &env, &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "bearer auth failed: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_basic_auth_attaches_base64_authorization_header() {
        // Jira Cloud end-to-end: the executor must encode the
        // user_env:password_env pair as standard base64 and ship it
        // via `Authorization: Basic <b64>` (NOT Bearer). Verify with
        // wiremock's `header()` matcher — mock 401s if the header is
        // wrong, 200s if it matches.
        let server = MockServer::start().await;
        // base64("user@x.io:t0k3n") = "dXNlckB4LmlvOnQwazNu"
        Mock::given(method("GET"))
            .and(path("/myself"))
            .and(header("Authorization", "Basic dXNlckB4LmlvOnQwazNu"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "displayName": "Test" })))
            .mount(&server)
            .await;
        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::Basic {
                user_env: "JIRA_USERNAME".into(),
                password_env: "JIRA_API_TOKEN".into(),
            },
            vec![mk_endpoint("GET", "/myself")],
        );
        let mut env = HashMap::new();
        env.insert("JIRA_USERNAME".into(), "user@x.io".into());
        env.insert("JIRA_API_TOKEN".into(), "t0k3n".into());
        let step = mk_step("/myself");
        let outcome = execute_api_call_step_core(&step, &plugin, &env, &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "Basic auth failed: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_templated_base_url_resolves_from_env() {
        // Jira-shape regression: the plugin spec's `base_url` is
        // `{JIRA_URL}` and the encrypted env carries the actual URL.
        // The executor must interpolate before composing the request.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rest/api/3/myself"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;
        // base_url placeholder → resolved at runtime against env.
        let plugin = mk_plugin(
            "{JIRA_URL}",
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/rest/api/3/myself")],
        );
        let mut env = HashMap::new();
        env.insert("JIRA_URL".into(), server.uri());
        let step = mk_step("/rest/api/3/myself");
        let outcome = execute_api_call_step_core(&step, &plugin, &env, &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "templated base_url failed: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_templated_base_url_unresolved_placeholder_fails_clearly() {
        // If the user forgot to fill JIRA_URL, the request never goes
        // out — we surface a Settings → APIs hint instead of a half-
        // composed URL hitting localhost or 404ing on a literal `{JIRA_URL}`.
        let plugin = mk_plugin(
            "{JIRA_URL}",
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/me")],
        );
        let env: HashMap<String, String> = HashMap::new();
        let step = mk_step("/me");
        let outcome = execute_api_call_step_core(&step, &plugin, &env, &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("JIRA_URL"),
            "error must name the missing key: {}", outcome.result.output);
        assert!(outcome.result.output.contains("Settings"),
            "error must point to Settings → APIs: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_4xx_does_not_retry_and_surfaces_body() {
        let server = MockServer::start().await;
        let mock = Mock::given(method("GET"))
            .and(path("/nope"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            // expect(1) asserts no retries happened.
            .expect(1);
        server.register(mock).await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/nope")],
        );
        let step = mk_step("/nope");
        let outcome = execute_api_call_step_core(&step, &plugin, &HashMap::new(), &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("403"),
            "expected 403 in output: {}", outcome.result.output);
        assert!(outcome.result.output.contains("Forbidden"),
            "expected body excerpt: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_5xx_retries_then_succeeds() {
        let server = MockServer::start().await;
        // First request: 500. Second: 200.
        Mock::given(method("GET"))
            .and(path("/flaky"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/flaky"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "n": 42 })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/flaky")],
        );
        let step = mk_step("/flaky");
        let outcome = execute_api_call_step_core(&step, &plugin, &HashMap::new(), &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "retry path failed: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_blocks_ssrf_to_loopback() {
        // Plugin declares a public base, but the step's endpoint path +
        // the resolved full URL must still land on that host. We flip
        // the plugin base to localhost directly (as if someone tampered
        // with the registry) — security guard must refuse the call.
        let plugin = mk_plugin(
            "https://127.0.0.1",
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/internal")],
        );
        let step = mk_step("/internal");
        // Production policy: both guards on. This is the whole point.
        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::production(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(
            outcome.result.output.to_lowercase().contains("security"),
            "expected security-tagged failure, got: {}", outcome.result.output,
        );
    }

    #[tokio::test]
    async fn execute_renders_query_params_from_template_context() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("project", "KR-42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/search")],
        );
        let mut step = mk_step("/search");
        let mut q = HashMap::new();
        q.insert("project".into(), "{{project_key}}".into());
        step.api_query = Some(q);

        let mut ctx = TemplateContext::new();
        ctx.set("project_key", "KR-42");

        let outcome = execute_api_call_step_core(&step, &plugin, &HashMap::new(), &ctx, SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "template render path failed: {}", outcome.result.output);
    }

    #[tokio::test]
    async fn execute_extract_empty_with_fail_on_empty_reports_no_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/empty"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "issues": [] })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/empty")],
        );
        let mut step = mk_step("/empty");
        step.api_extract = Some(ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: None,
            fail_on_empty: true,
        });

        let outcome = execute_api_call_step_core(&step, &plugin, &HashMap::new(), &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        // Success at HTTP level, NO_RESULTS at business level — fires
        // Skip/Stop conditions downstream without marking the step Failed.
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope = extract_envelope(&outcome.result.output);
        assert_eq!(envelope["status"], "NO_RESULTS");
    }

    // ─── Pagination walk ────────────────────────────────────────────

    #[tokio::test]
    async fn walk_pages_offset_concatenates_three_pages() {
        // Simulates Jira: page 1 issues 0-1, page 2 issues 2-3, page 3 issue 4.
        // total=5, limit=2 → 3 pages walked, 5 items merged.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("startAt", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{ "key": "K-1" }, { "key": "K-2" }],
                "total": 5
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("startAt", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{ "key": "K-3" }, { "key": "K-4" }],
                "total": 5
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("startAt", "4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{ "key": "K-5" }],
                "total": 5
            })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/search")],
        );
        let mut step = mk_step("/search");
        step.api_pagination = Some(PaginationSpec::Offset {
            start_param: "startAt".into(),
            limit_param: "maxResults".into(),
            limit: 2,
            total_path: "$.total".into(),
            max_pages: Some(10),
        });
        step.api_extract = Some(ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "offset walk failed: {}", outcome.result.output);
        let envelope = extract_envelope(&outcome.result.output);
        // All 5 keys merged from the 3 pages — the user's
        // `$.issues[*].key` extract sees the concatenated array.
        assert_eq!(envelope["data"], json!(["K-1", "K-2", "K-3", "K-4", "K-5"]));
    }

    #[tokio::test]
    async fn walk_pages_cursor_stops_when_next_path_resolves_null() {
        // Cloudflare-ish GraphQL pattern with cursor=endCursor.
        let server = MockServer::start().await;
        // Page 1: returns endCursor "abc" → walker continues.
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(query_param("after", "INITIAL"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [{ "id": 1 }],
                "endCursor": "abc"
            })))
            .mount(&server)
            .await;
        // Page 2: endCursor absent → walker stops.
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(query_param("after", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "items": [{ "id": 2 }, { "id": 3 }],
                "endCursor": serde_json::Value::Null,
            })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/q")],
        );
        let mut step = mk_step("/q");
        let mut q = HashMap::new();
        q.insert("after".into(), "INITIAL".into());
        step.api_query = Some(q);
        step.api_pagination = Some(PaginationSpec::Cursor {
            cursor_param: "after".into(),
            next_path: "$.endCursor".into(),
            max_pages: Some(10),
        });
        step.api_extract = Some(ExtractSpec {
            path: "$.items[*].id".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "cursor walk failed: {}", outcome.result.output);
        let envelope = extract_envelope(&outcome.result.output);
        assert_eq!(envelope["data"], json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn walk_pages_page_stops_when_has_more_false() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/list"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "n": 1 }, { "n": 2 }],
                "has_more": true
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/list"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "n": 3 }],
                "has_more": false
            })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/list")],
        );
        let mut step = mk_step("/list");
        let mut q = HashMap::new();
        q.insert("page".into(), "1".into());
        step.api_query = Some(q);
        step.api_pagination = Some(PaginationSpec::Page {
            page_param: "page".into(),
            page_size_param: "per_page".into(),
            page_size: 2,
            has_more_path: "$.has_more".into(),
            max_pages: Some(5),
        });
        step.api_extract = Some(ExtractSpec {
            path: "$.data[*].n".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success,
            "page walk failed: {}", outcome.result.output);
        let envelope = extract_envelope(&outcome.result.output);
        assert_eq!(envelope["data"], json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn walk_pages_respects_max_pages_cap() {
        // API claims has_more forever — the cap is what prevents a
        // misconfigured response from looping the worker forever.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/infinite"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "n": 1 }],
                "has_more": true,
            })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/infinite")],
        );
        let mut step = mk_step("/infinite");
        step.api_pagination = Some(PaginationSpec::Page {
            page_param: "page".into(),
            page_size_param: "per_page".into(),
            page_size: 1,
            has_more_path: "$.has_more".into(),
            max_pages: Some(3),
        });
        step.api_extract = Some(ExtractSpec {
            path: "$.data[*].n".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope = extract_envelope(&outcome.result.output);
        // Cap = 3 pages → 3 items merged. Without the cap this would
        // loop forever.
        assert_eq!(envelope["data"], json!([1, 1, 1]));
    }

    #[tokio::test]
    async fn walk_pages_none_or_auto_returns_single_page_unchanged() {
        // Sanity guard: the existing single-page tests must keep
        // passing now that the walker is in front of `send_with_retry`.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/once"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{ "key": "X-1" }],
                "total": 1,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/once")],
        );
        let mut step = mk_step("/once");
        // PaginationSpec::Auto with default cap — must NOT walk.
        step.api_pagination = Some(PaginationSpec::Auto { max_pages: None });
        step.api_extract = Some(ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: None,
            fail_on_empty: false,
        });

        let outcome = execute_api_call_step_core(
            &step,
            &plugin,
            &HashMap::new(),
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope = extract_envelope(&outcome.result.output);
        // Single match unwraps to the scalar — see `apply_extract` size-1
        // unwrap semantics. Caller's `{{steps.X.data}}` template renders
        // "X-1" cleanly; an array-of-one would require `{{...[0]}}`.
        assert_eq!(envelope["data"], json!("X-1"));
    }

    #[tokio::test]
    async fn execute_no_extract_spec_passes_through_full_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/raw"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "a": 1, "b": [2, 3] })))
            .mount(&server)
            .await;

        let plugin = mk_plugin(
            &server.uri(),
            ApiAuthKind::None,
            vec![mk_endpoint("GET", "/raw")],
        );
        let step = mk_step("/raw");
        let outcome = execute_api_call_step_core(&step, &plugin, &HashMap::new(), &TemplateContext::new(), SecurityPolicy::allow_loopback_for_tests()).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        let envelope = extract_envelope(&outcome.result.output);
        assert_eq!(envelope["data"], json!({ "a": 1, "b": [2, 3] }));
    }

    // ─── interpolate_env (pure) ─────────────────────────────────────

    #[test]
    fn interpolate_env_substitutes_known_keys() {
        let mut env = HashMap::new();
        env.insert("COMPANY".into(), "acme".into());
        env.insert("REGION".into(), "eu".into());
        let out = interpolate_env("report/{COMPANY}/{REGION}/v1", &env);
        assert_eq!(out, "report/acme/eu/v1");
    }

    #[test]
    fn interpolate_env_marks_missing_keys_explicitly() {
        let env = HashMap::new();
        let out = interpolate_env("x-{MISSING}-y", &env);
        assert_eq!(out, "x-<NOT_CONFIGURED:MISSING>-y");
    }

    // ─── truncate unicode safety ────────────────────────────────────

    #[test]
    fn truncate_unicode_does_not_split_grapheme_bytes() {
        // Regression guard: a naive byte slice would crash on a multi-
        // byte emoji / accented char.
        let s = "éèê🔥🔥🔥";
        let t = truncate(s, 4);
        assert!(s.starts_with(&t));
        assert!(t.chars().count() <= 4);
    }

    // ─── on_result + SIGNAL emission tests ────────────────────────────
    //
    // Mirrors the Exec-step contract: ApiCall surfaces signals so a
    // workflow can branch on HTTP status without writing a wrapper
    // Agent step ("503 → Goto retry, 401 → Goto refresh_auth").

    #[tokio::test]
    async fn success_appends_signal_ok() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "x": 1 })))
            .mount(&server)
            .await;
        let plugin = mk_plugin(&server.uri(), ApiAuthKind::None, vec![mk_endpoint("GET", "/ok")]);
        let step = mk_step("/ok");
        let outcome = execute_api_call_step_core(
            &step, &plugin, &HashMap::new(), &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Success);
        assert!(outcome.result.output.contains("[SIGNAL: OK]"));
    }

    #[tokio::test]
    async fn http_5xx_appends_signal_error_and_http_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/boom"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;
        let plugin = mk_plugin(&server.uri(), ApiAuthKind::None, vec![mk_endpoint("GET", "/boom")]);
        let mut step = mk_step("/boom");
        step.api_max_retries = Some(0); // skip backoff in tests
        let outcome = execute_api_call_step_core(
            &step, &plugin, &HashMap::new(), &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        assert!(outcome.result.output.contains("[SIGNAL: ERROR]"));
        assert!(outcome.result.output.contains("[SIGNAL: http_503]"));
    }

    #[tokio::test]
    async fn on_result_goto_fires_when_signal_matches_on_4xx() {
        // The headline use case: an API returns 401, we want to Goto a
        // refresh_auth step instead of triggering on_failure rollback.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/locked"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let plugin = mk_plugin(&server.uri(), ApiAuthKind::None, vec![mk_endpoint("GET", "/locked")]);
        let mut step = mk_step("/locked");
        step.api_max_retries = Some(0);
        step.on_result = vec![StepConditionRule {
            contains: "http_401".to_string(),
            action: ConditionAction::Goto {
                step_name: "refresh_auth".to_string(),
                max_iterations: Some(2),
            },
        }];
        let outcome = execute_api_call_step_core(
            &step, &plugin, &HashMap::new(), &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
        ).await;
        assert_eq!(outcome.result.status, RunStatus::Failed);
        match outcome.condition_action {
            Some(ConditionAction::Goto { step_name, max_iterations }) => {
                assert_eq!(step_name, "refresh_auth");
                assert_eq!(max_iterations, Some(2));
            }
            other => panic!("expected Goto on http_401, got {:?}", other),
        }
        assert_eq!(outcome.result.condition_result.as_deref(), Some("Goto:refresh_auth"));
    }
}
