// MCP (Model Context Protocol) — server registry, transports, configs,
// + the API capability declaration (REST endpoints) that lets a plugin
// expose curl-based access alongside (or instead of) an MCP transport.
//
// Also hosts the small MCP context-files types — the optional, manually
// editable `<docs>/operations/mcp-servers/<slug>.md` notes managed from the
// MCP page UI. (No longer auto-generated; agents get MCP capabilities via the
// injected `=== AVAILABLE APIs ===` block built from `api_spec`.)

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─── MCP server registry + transport ──────────────────────────────────────

/// An MCP server type (e.g. "GitHub", "Atlassian", "Context7").
///
/// A plugin can have MCP capability (via `transport`), API capability (via
/// `api_spec`), or BOTH — e.g. Jira exposes both a `@modelcontextprotocol`
/// server and a REST API, and a hybrid plugin lets the agent pick the
/// right tool. API-only plugins use `McpTransport::ApiOnly` as a sentinel.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub source: McpSource,
    /// When present, the plugin exposes a REST API. Emitted into the agent's
    /// `--append-system-prompt` as a `=== AVAILABLE APIs ===` block so the
    /// agent can call it via curl (vs. MCP-style tools). NULL = MCP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_spec: Option<ApiSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
    Streamable { url: String },
    /// Plugin has no MCP transport — it's API-only. The sync code MUST skip
    /// these when writing `.mcp.json`; everything relevant lives in
    /// `api_spec` and gets injected into the prompt instead.
    ApiOnly,
}

// ─── REST API capability ──────────────────────────────────────────────────

/// REST API capability for a plugin.
///
/// Stored on `McpServer` to let a plugin expose an HTTP API alongside (or
/// instead of) an MCP transport. The value is serialized into the
/// `mcp_servers.api_spec_json` column (migration 035) and reused by the
/// prompt-injection path that emits `=== AVAILABLE APIs ===` blocks.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiSpec {
    pub base_url: String,
    pub auth: ApiAuthKind,
    /// Short list of the most useful endpoints. Not exhaustive — the UI
    /// surfaces `docs_url` for the full reference. Agents may call
    /// undocumented endpoints if they know the path; this list is
    /// primarily a hint + curl example.
    pub endpoints: Vec<ApiEndpoint>,
    /// URL of the vendor's API reference documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// Additional config keys the user must provide on top of the credential
    /// (e.g. Chartbeat's `host=example.com`). Stored alongside the secret
    /// in the config's encrypted env, surfaced in the prompt injection so
    /// the agent has the exact curl arguments to use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_keys: Vec<ApiConfigKey>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApiAuthKind {
    /// API key passed as a query parameter, e.g. `?apikey=...` (Chartbeat).
    ApiKeyQuery { param_name: String, env_key: String },
    /// API key passed as a header, e.g. `X-API-Key: ...`.
    ApiKeyHeader { header_name: String, env_key: String },
    /// Bearer token in `Authorization: Bearer ...`.
    Bearer { env_key: String },
    /// HTTP Basic — `Authorization: Basic <base64(user:password)>`. Used by
    /// Jira Cloud (`email:api_token`), Bitbucket Cloud, and any other
    /// vendor that ships a "user + secret" token pair. Both halves come
    /// from the encrypted config env so Kronn never stores them in
    /// plaintext.
    Basic { user_env: String, password_env: String },
    /// HTTP Basic with the API key as the username and an empty password
    /// — `Authorization: Basic <base64(API_KEY:)>`. The flavor used by
    /// SpeedCurve, Stripe, and a few other API-key-only providers that
    /// chose Basic as the wire format. Reduces the env-key footprint to
    /// 1 (the secret) and avoids forcing operators to type a placeholder
    /// "empty password" in Settings → APIs.
    BasicApiKey { env_key: String },
    /// OAuth2 client-credentials grant — Kronn exchanges `client_id` +
    /// `client_secret` against `token_url` to get a short-lived
    /// `access_token`, caches it until expiry, and injects a fresh
    /// `Authorization: Bearer <token>` into the agent's prompt.
    ///
    /// `extra_headers` lets the spec declare other headers Kronn should
    /// surface to the agent (e.g. Adobe's `x-api-key: <client_id>` +
    /// `x-proxy-global-company-id: <company_id>`). Values can reference
    /// any env key from the config via `{ENV_KEY}` placeholders and are
    /// substituted at injection time.
    OAuth2ClientCredentials {
        token_url: String,
        client_id_env: String,
        client_secret_env: String,
        scope: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_headers: Vec<OAuth2ExtraHeader>,
    },
    /// 0.8.6 — generic token-exchange auth. Generalises
    /// `OAuth2ClientCredentials` to support APIs that ship a custom
    /// 2-step auth flow with non-standard body shape, field names, or
    /// token JSON path. Use when the vendor doesn't conform to RFC 6749
    /// client-credentials but DOES follow the "POST creds → get
    /// access_token → Authorization: Bearer …" pattern. Reference
    /// implementation: Didomi's `POST /sessions` with JSON body
    /// `{type, key, secret}` and response `{access_token}`. Also fits
    /// many enterprise auth-0/Salesforce-flavored APIs.
    ///
    /// Token cache is reused from `OAuth2ClientCredentials` (same
    /// `AppState.oauth2_cache` Mutex<HashMap<config_id, CachedToken>>)
    /// so refresh / TTL semantics are identical.
    TokenExchange {
        /// Endpoint relative to the plugin's `base_url`. Examples:
        /// `/sessions`, `/oauth/token`, `/v1/auth/exchange`.
        endpoint: String,
        /// HTTP method — typically `POST`, occasionally `PUT`.
        #[serde(default = "default_token_exchange_method")]
        method: String,
        /// Request body template. String leaves support `${ENV.KEY}`
        /// substitution from the decrypted env (e.g. `"${ENV.API_KEY}"`).
        /// Non-string leaves pass through as-is.
        /// Didomi example:
        /// ```json
        /// {"type": "api-key", "key": "${ENV.API_KEY}", "secret": "${ENV.API_SECRET}"}
        /// ```
        body_template: serde_json::Value,
        /// Body serialization format on the wire.
        #[serde(default)]
        body_format: TokenExchangeBodyFormat,
        /// JSONPath to extract the token from the response. Examples:
        /// `$.access_token`, `$.data.token`, `$.session.bearer`.
        token_jsonpath: String,
        /// Cached-token TTL in seconds. Kronn refreshes at T-30s
        /// safety margin. `0` disables caching (re-exchange every call,
        /// only useful for testing).
        #[serde(default = "default_token_exchange_ttl")]
        ttl_seconds: u64,
        /// How to inject the resulting token into subsequent calls on
        /// THIS plugin's endpoints.
        #[serde(default)]
        inject: TokenInjection,
        /// Defensive: env_keys the spec needs the user to fill. Empty
        /// is permitted but the form/validator can use this to flag
        /// missing creds before the exchange fires.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        creds_env_keys: Vec<String>,
    },
    /// No auth (public endpoints).
    #[default]
    None,
}

fn default_token_exchange_method() -> String { "POST".to_string() }
fn default_token_exchange_ttl() -> u64 { 3600 }

/// 0.8.6 — Body serialization format for `TokenExchange`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub enum TokenExchangeBodyFormat {
    /// JSON body (`Content-Type: application/json`). Didomi, Auth0,
    /// most modern API platforms.
    #[default]
    Json,
    /// Form URL-encoded body (`application/x-www-form-urlencoded`).
    /// Matches the canonical OAuth2 RFC 6749 wire format.
    FormUrlEncoded,
}

/// 0.8.6 — How to inject a freshly-exchanged token into the actual API
/// call. The 90% case is Bearer header — but some vendors (Anthropic
/// itself, some legacy SOAP gateways) put it elsewhere.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TokenInjection {
    /// `Authorization: Bearer <token>` — the default and most common.
    #[default]
    BearerHeader,
    /// Custom header — `<name>: <token>` (no `Bearer ` prefix).
    CustomHeader { name: String },
    /// Query string param — `?<name>=<token>`.
    QueryParam { name: String },
}

/// Static header rendered alongside the `Authorization: Bearer` from an
/// OAuth2 exchange. Used for providers (Adobe Analytics, some Salesforce
/// endpoints) that require extra identification headers beyond the
/// bearer token. `value_template` supports `{ENV_KEY}` substitution from
/// the config's env map.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OAuth2ExtraHeader {
    pub name: String,
    pub value_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiEndpoint {
    pub path: String,
    /// `"GET"`, `"POST"`, etc. Kept free-form to avoid constraining agents
    /// that want to call a rare verb.
    pub method: String,
    pub description: String,
}

/// A non-secret parameter the plugin instance needs (e.g. host, workspace id).
/// Stored in the same encrypted env blob as the API key, but the UI renders
/// these as plain inputs (no mask), and they're surfaced in the prompt
/// injection alongside the auth so the agent can build full URLs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiConfigKey {
    pub env_key: String,
    pub label: String,
    pub placeholder: String,
    pub description: String,
}

// ─── Source / scope / config ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpSource {
    Registry,
    /// Surfaced from a project's `.mcp.json` (existing path).
    Detected,
    Manual,
    /// Adopted from a host CLI config file (`~/.claude.json`,
    /// `~/.gemini/settings.json`, etc.) via Phase-2 host-discovery import.
    HostImported,
}

/// How a config should be surfaced to the local CLIs (Claude Code, Gemini,
/// Codex, Copilot) when they run *outside* a Kronn-managed project.
///
/// Separated from `is_global` (which is Kronn-internal: "applied across all
/// Kronn projects") because the two concepts answer different questions:
/// `is_global` decides Kronn project visibility; `host_sync` decides
/// whether Kronn writes the entry into `~/.claude.json` & friends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum HostSyncMode {
    /// Kronn-only. Never written to a host CLI config file.
    None,
    /// Written to host config files. Not auto-applied to Kronn projects.
    GlobalOnly,
    /// Written to host config files AND auto-applied to all projects
    /// (preserves the pre-0.5.2 Codex/Copilot "everything global" UX).
    MirrorAll,
}

/// A configured instance of an MCP server — with label, env secrets, etc.
/// Multiple projects can share the same config (deduplication by config_hash).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpConfig {
    pub id: String,
    pub server_id: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_encrypted: String,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub include_general: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
    /// Migration 036 — opt-in outbound sync to host CLI files.
    /// Defaults to `None` for safety; existing rows get `None` from the
    /// SQL migration's `DEFAULT 'None'` clause.
    #[serde(default = "default_host_sync")]
    pub host_sync: HostSyncMode,
}

fn default_host_sync() -> HostSyncMode { HostSyncMode::None }

/// Display-safe version of McpConfig (secrets masked)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpConfigDisplay {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_masked: Vec<McpEnvEntry>,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub include_general: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
    pub project_names: Vec<String>,
    /// True when env_keys exist but decryption fails (secrets need re-entry).
    #[serde(default)]
    pub secrets_broken: bool,
    /// See `McpConfig::host_sync`.
    #[serde(default = "default_host_sync")]
    pub host_sync: HostSyncMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpEnvEntry {
    pub key: String,
    pub masked_value: String,
}

/// Registry entry — an MCP available for installation
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub env_keys: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_help: Option<String>,
    /// Who built this MCP server (e.g. "Anthropic", "Redis Labs", "Fastly").
    pub publisher: String,
    /// True when the MCP is built by the vendor of the service it connects to
    /// (e.g. Fastly MCP by Fastly = official, GitHub MCP by Anthropic = not official by vendor).
    pub official: bool,
    /// Alternative package names that map to this same MCP server.
    /// Used during scan to match detected .mcp.json entries that use a different
    /// runtime (e.g. npm package vs Go binary) to the canonical registry entry.
    /// Example: Fastly registry uses `fastly-mcp` (Go) but users may have `fastly-mcp-server` (npm).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub alt_packages: Vec<String>,
    /// Pre-filled MCP context content (best practices, token-saving tips).
    /// NOTE: no longer auto-materialised to disk (the per-MCP doc auto-gen was
    /// removed — agents get capabilities via the injected `api_spec` block).
    /// Currently unused; kept as reference data / potential manual-edit seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub default_context: Option<String>,
    /// API capability declaration when the plugin exposes a REST API
    /// (pure-API like Chartbeat, or hybrid like Jira which has both an MCP
    /// and a REST API). Mirrored onto the corresponding `McpServer.api_spec`
    /// at seed time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_spec: Option<ApiSpec>,
}

// ─── MCP context files ────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpContextEntry {
    pub slug: String,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateMcpContextRequest {
    pub content: String,
}

// ─── MCP API requests ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateMcpConfigRequest {
    pub server_id: String,
    pub label: String,
    #[ts(type = "Record<string, string>")]
    pub env: std::collections::HashMap<String, String>,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub project_ids: Vec<String>,
    /// Custom API plugin payload. Only honoured when `server_id == "api-custom"`.
    /// The backend materializes a new `McpServer` (API-only, `source = Manual`)
    /// from these fields, then proceeds with the normal config-creation path.
    /// Auth type is always `None` for custom plugins; the agent reads the
    /// description + docs_url + fields and figures out auth itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_spec: Option<CustomApiPayload>,
}

/// Free-form spec for a user-defined API plugin (the "Custom API" flow).
/// Captured from the frontend form; the backend turns it into an
/// `ApiSpec` + `McpServer` pair on submit.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CustomApiPayload {
    pub name: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// 0.8.6 — auth scheme. `ApiAuthKind: Default = None`, so the field
    /// is back-compat for any payload that omits it (pre-0.8.6 Custom
    /// plugins keep working unchanged). When set, the materialized
    /// `ApiSpec.auth` carries it instead of hardcoding `None` — which
    /// is what made Custom plugins muets côté auth pre-fix (caught
    /// 2026-05-19 on Didomi audit).
    #[serde(default)]
    pub auth: ApiAuthKind,
    /// List of `{label, value}` pairs. The backend slugifies each label
    /// into an `env_key` (UPPER_SNAKE_CASE) and stores the value in the
    /// encrypted env blob alongside the rest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<CustomApiField>,
    /// 0.8.6 — endpoints the user (often via the `CustomApiAiHelper`
    /// fetching the docs) wants declared on this plugin. Without these,
    /// the executor's allowlist refuses any agent-driven ApiCall — so
    /// declaring them at create time is what flips `mcp_list`'s hint
    /// from `NEEDS_RESEARCH` to `READY`. Blank-path entries are
    /// silently dropped at materialize time. Each entry: `{path,
    /// method, description}` (matches the existing `ApiEndpoint`
    /// shape).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<ApiEndpoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CustomApiField {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateMcpConfigRequest {
    pub label: Option<String>,
    #[ts(type = "Record<string, string> | null")]
    pub env: Option<std::collections::HashMap<String, String>>,
    pub args_override: Option<Vec<String>>,
    pub is_global: Option<bool>,
    pub include_general: Option<bool>,
    pub host_sync: Option<HostSyncMode>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct LinkMcpConfigRequest {
    pub project_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpOverview {
    pub servers: Vec<McpServer>,
    pub configs: Vec<McpConfigDisplay>,
    /// Set of "slug:projectId" pairs where the context file has been customized (not default template).
    #[serde(default)]
    pub customized_contexts: Vec<String>,
    /// Known incompatibilities between MCP servers and agents.
    #[serde(default)]
    pub incompatibilities: Vec<McpIncompatibility>,
    /// Configs that declare `env_keys` but have empty/missing values for
    /// at least one of them — those would fail handshake at agent boot
    /// (Connection closed, OAuth invalid_client, etc.) AND poison the
    /// per-agent startup latency. Kronn skips them at sync time and the
    /// UI surfaces them as warnings so the operator can complete the
    /// config or remove the entry.
    #[serde(default)]
    pub incomplete_configs: Vec<McpIncompleteConfig>,
}

/// A known incompatibility between an MCP server and a specific agent.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpIncompatibility {
    /// The MCP server ID (e.g. "mcp-gitlab", "detected:data-gouv-fr")
    pub server_id: String,
    /// The agent that is incompatible
    pub agent: super::AgentType,
    /// Human-readable explanation
    pub reason: String,
}

/// An MCP config whose declared env keys aren't all populated. The
/// scanner skips writing this entry into project-level config files
/// (`.mcp.json`, `.kiro/settings/mcp.json`, `.gemini/settings.json`,
/// `~/.codex/config.toml`) so the agent doesn't choke on a broken
/// MCP at startup. The UI surfaces this so the operator knows which
/// plugin to fix.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpIncompleteConfig {
    /// MCP config DB id.
    pub config_id: String,
    /// User-facing label of the config (e.g. "Adobe Analytics", "GitLab Euronews").
    pub label: String,
    /// Server name behind this config (helps the user remember which plugin).
    pub server_name: String,
    /// The declared env_keys whose decrypted value is missing or empty.
    /// Empty when the issue is decryption failure (the cipher itself is
    /// gone, e.g. after a key rotation) — the UI shows a generic
    /// "secrets unreadable" hint in that case.
    pub missing_keys: Vec<String>,
    /// Free-form reason — `missing_keys` for a key-by-key gap, or a
    /// short error message for decryption failures.
    pub reason: String,
}
