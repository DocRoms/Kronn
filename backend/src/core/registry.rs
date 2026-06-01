use crate::models::{
    ApiAuthKind, ApiConfigKey, ApiEndpoint, ApiSpec, McpDefinition, McpTransport,
    OAuth2ExtraHeader,
};

/// Sentinel id surfaced at the top of the registry. Picking it in the UI
/// drawer switches the right panel to the Custom API form. On submit, the
/// backend ignores the sentinel and materializes a fresh `McpServer` from
/// the user-provided `CustomApiPayload`. Prefixed `api-` to conform to the
/// registry-id naming convention (enforced by `registry_test`).
pub const CUSTOM_API_SERVER_ID: &str = "api-custom";

/// Return the built-in MCP registry — official servers only
pub fn builtin_registry() -> Vec<McpDefinition> {
    vec![
        // ── Custom API: sentinel for the "describe your own API" flow ──
        // Pinned first so users with an unsupported vendor find it immediately
        // in the drawer. On submit the backend swaps this id for a generated
        // `custom-{slug}-{nano}` server id sourced from the form payload.
        McpDefinition {
            id: CUSTOM_API_SERVER_ID.into(),
            name: "Custom API".into(),
            description: "Define your own REST API: name, base URL, free-form description, optional docs link, and any fields the agent needs (tokens, IDs, headers).".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec![],
            tags: vec!["custom".into(), "freeform".into(), "user-defined".into(), "api".into()],
            token_url: None,
            token_help: None,
            publisher: "You".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: Some(ApiSpec {
                base_url: "{BASE_URL}".into(),
                auth: ApiAuthKind::None,
                endpoints: vec![],
                docs_url: None,
                config_keys: vec![],
            }),
        },

        // ── Git & Code ──────────────────────────────────────────────────────
        // ── GitHub: hybrid MCP (Stdio agent calls) + REST API (ApiCall steps) ──
        // Both layers share the same `GITHUB_PERSONAL_ACCESS_TOKEN`, encrypted
        // once in the plugin config. The MCP transport keeps powering Quick
        // Prompts via `@modelcontextprotocol/server-github`; `api_spec`
        // declares the curated REST endpoints surfaced to the workflow
        // wizard's ApiCall step. Path placeholders (`{owner}`, `{repo}`,
        // `{issue_number}`, …) are filled by the user in the wizard's
        // editable endpoint combobox at step build time.
        McpDefinition {
            id: "mcp-github".into(),
            name: "GitHub".into(),
            description: "Issues, PRs, Actions, repos — MCP for agents (Quick Prompts) + REST API for désagentified ApiCall steps".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            },
            env_keys: vec!["GITHUB_PERSONAL_ACCESS_TOKEN".into()],
            tags: vec!["git".into(), "ci".into(), "code".into(), "api".into()],
            token_url: Some("https://github.com/settings/tokens?type=beta".into()),
            token_help: Some("Fine-grained PAT with `repo` (Issues + PRs + Contents) scopes for the repos you want to query. Classic PAT with `repo` scope also works. The same token powers both the MCP transport (used by agents in Quick Prompts) and the REST API (used by ApiCall workflow steps).".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: Some(ApiSpec {
                base_url: "https://api.github.com".into(),
                auth: ApiAuthKind::Bearer {
                    env_key: "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                },
                docs_url: Some("https://docs.github.com/en/rest".into()),
                config_keys: vec![],
                endpoints: vec![
                    // ── Sanity check ──
                    ApiEndpoint { path: "/user".into(),                                          method: "GET".into(),  description: "[USER · sanity] Authenticated user — quick way to verify the token works (200 = OK, 401 = revoked / wrong scope).".into() },
                    // ── Repos ──
                    ApiEndpoint { path: "/user/repos".into(),                                    method: "GET".into(),  description: "[REPOS] List repos the user has access to. Useful query params: `visibility=all|public|private`, `affiliation=owner,collaborator`, `sort=updated`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}".into(),                          method: "GET".into(),  description: "[REPOS] Single repo metadata (default branch, stars, language, …). Replace `{owner}/{repo}` in the path field.".into() },
                    // ── Issues ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues".into(),                   method: "GET".into(),  description: "[ISSUES · list] Issues for a repo. Query: `state=open|closed|all`, `labels=bug,security`, `assignee=login|*|none`, `since=ISO8601`, `per_page=100`. NB: also returns PRs (PRs are issues) — filter by absence of `pull_request` field if needed.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues/{issue_number}".into(),    method: "GET".into(),  description: "[ISSUES · single] One issue by number. Replace `{owner}/{repo}/{issue_number}`.".into() },
                    ApiEndpoint { path: "/search/issues".into(),                                  method: "GET".into(),  description: "[SEARCH · issues+PRs] Cross-repo search. Query: `q=is:issue+is:open+repo:owner/name+label:bug` (URL-encoded). Powerful + capped at 1000 results, paginated.".into() },
                    // ── Pull requests ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls".into(),                    method: "GET".into(),  description: "[PRs · list] Open / closed / all PRs. Query: `state`, `head=user:branch`, `base=main`, `sort=updated`, `direction=desc`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls/{pull_number}".into(),      method: "GET".into(),  description: "[PRs · single] One PR by number, with `mergeable`, `mergeable_state`, `additions`, `deletions`, `changed_files`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls/{pull_number}/files".into(),method: "GET".into(),  description: "[PRs · diff] Files changed in a PR — paths + patches (truncated > 3000 lines).".into() },
                    // ── Commits ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/commits".into(),                  method: "GET".into(),  description: "[COMMITS] Commit list. Query: `sha=branch|sha`, `path=file`, `author=login`, `since=ISO8601`, `until=ISO8601`.".into() },
                    // ── Actions ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/actions/runs".into(),             method: "GET".into(),  description: "[ACTIONS] Workflow runs. Query: `status=queued|in_progress|completed`, `conclusion=success|failure|cancelled`, `branch=main`, `event=push|pull_request`.".into() },
                    // ── Releases ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/releases".into(),                 method: "GET".into(),  description: "[RELEASES] Releases for a repo (paginated, latest first).".into() },
                    // ── Notifications ──
                    ApiEndpoint { path: "/notifications".into(),                                  method: "GET".into(),  description: "[NOTIFS] Authenticated user's notifications. Query: `all=true|false`, `participating=true|false`, `since=ISO8601`.".into() },
                    // ── Write endpoints (require a token with `repo` scope) ──
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls".into(),                     method: "POST".into(), description: "[PRs · create] Open a pull request. Body: `{title, head, base, body?, draft?}`. `head` = source branch (or `owner:branch` for forks), `base` = target branch. Returns the created PR incl. `number` and `head.sha`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls/{pull_number}".into(),       method: "PATCH".into(), description: "[PRs · update] Edit a PR. Body: `{title?, body?, state?, base?}`. `state: \"closed\"` closes it (no merge). Replace `{pull_number}`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/pulls/{pull_number}/comments".into(), method: "POST".into(), description: "[PRs · review comment] Comment on a specific line of a PR diff. Body: `{body, commit_id, path, line, side?}`. `commit_id` = head SHA, `path` = file, `line` = line in the diff. For a plain top-level PR comment use the issues/comments endpoint instead.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues".into(),                    method: "POST".into(), description: "[ISSUES · create] Create an issue. Body: `{title, body?, labels?, assignees?, milestone?}`. (PRs are issues, but create PRs via /pulls.)".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues/{issue_number}".into(),      method: "PATCH".into(), description: "[ISSUES · update] Edit an issue/PR. Body: `{title?, body?, state?, labels?, assignees?}`. `state: \"closed\"` closes. Works on PR numbers too (PRs are issues).".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues/{issue_number}/comments".into(), method: "POST".into(), description: "[ISSUES · comment] Add a comment to an issue or PR (top-level, not line-bound). Body: `{body}`.".into() },
                    ApiEndpoint { path: "/repos/{owner}/{repo}/issues/{issue_number}/labels".into(), method: "POST".into(), description: "[ISSUES · add labels] Add labels to an issue/PR (additive — does not remove existing). Body: `{labels: [\"ci-test\"]}`. Replace `{issue_number}`.".into() },
                ],
            }),
        },
        McpDefinition {
            id: "mcp-gitlab".into(),
            name: "GitLab".into(),
            description: "Issues, MRs, pipelines, projects — official GitLab CLI MCP server (experimental). Requires the `glab` CLI installed locally.".into(),
            transport: McpTransport::Stdio {
                command: "glab".into(),
                args: vec!["mcp".into(), "serve".into()],
            },
            env_keys: vec!["GITLAB_TOKEN".into(), "GITLAB_HOST".into()],
            // `cli` first so getCategory() picks it up before `git` →
            // surfaces this plugin under the "CLI wrappers" filter rather
            // than the regular Git/Code bucket. The MCP wraps a local
            // binary so it has the same prereq as a CLI agent (host
            // install required).
            tags: vec!["cli".into(), "git".into(), "ci".into(), "code".into()],
            token_url: Some("https://gitlab.com/-/user_settings/personal_access_tokens".into()),
            token_help: Some("Requires glab CLI (brew install glab / winget install glab). GITLAB_TOKEN: PAT with api scope. GITLAB_HOST: your GitLab hostname (e.g. gitlab.company.com). Leave GITLAB_HOST empty for gitlab.com.".into()),
            publisher: "GitLab".into(),
            official: true,
            alt_packages: vec!["@modelcontextprotocol/server-gitlab".into()],
            default_context: None,
            api_spec: None,
        },
        // ── Databases ───────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-postgres".into(),
            name: "PostgreSQL".into(),
            description: "SQL queries and schema management — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-postgres".into()],
            },
            env_keys: vec!["POSTGRES_CONNECTION_STRING".into()],
            tags: vec!["database".into(), "sql".into()],
            token_url: None,
            token_help: Some("Connection string: postgresql://user:pass@host:5432/db".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-sqlite".into(),
            name: "SQLite".into(),
            description: "Embedded database queries — official MCP server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-sqlite".into()],
            },
            env_keys: vec![],
            tags: vec!["database".into(), "embedded".into()],
            token_url: None,
            token_help: None,
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-redis".into(),
            name: "Redis".into(),
            description: "Cache, pub/sub, streams, JSON, search — official Redis server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["--from".into(), "redis-mcp-server@latest".into(), "redis-mcp-server".into()],
            },
            env_keys: vec!["REDIS_HOST".into(), "REDIS_PWD".into()],
            tags: vec!["cache".into(), "database".into()],
            token_url: None,
            token_help: Some("REDIS_HOST (default 127.0.0.1), REDIS_PORT (default 6379), REDIS_PWD. Optional: REDIS_SSL=true for TLS.".into()),
            publisher: "Redis Ltd".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-mongodb".into(),
            name: "MongoDB".into(),
            description: "Atlas clusters, collections, documents, aggregation pipelines — official MongoDB server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "mongodb-mcp-server".into()],
            },
            env_keys: vec!["MDB_MCP_CONNECTION_STRING".into(), "MDB_MCP_ATLAS_CLIENT_ID".into(), "MDB_MCP_ATLAS_CLIENT_SECRET".into()],
            tags: vec!["database".into(), "nosql".into()],
            token_url: Some("https://cloud.mongodb.com/v2#/org/settings/apiKeys".into()),
            token_help: Some("MDB_MCP_CONNECTION_STRING: mongodb+srv://user:pass@cluster.mongodb.net/db. For Atlas API: MDB_MCP_ATLAS_CLIENT_ID + MDB_MCP_ATLAS_CLIENT_SECRET (service account). Supports --readOnly flag for safe usage.".into()),
            publisher: "MongoDB".into(),
            official: true,
            alt_packages: vec!["mongodb/mongodb-mcp-server".into()],
            default_context: None,
            api_spec: None,
        },
        // ── Cloud & Infra ───────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-cloudflare".into(),
            name: "Cloudflare".into(),
            description: "Workers, KV, R2, D1, DNS — official Cloudflare server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@cloudflare/mcp-server-cloudflare".into()],
            },
            env_keys: vec!["CLOUDFLARE_API_TOKEN".into(), "CLOUDFLARE_ACCOUNT_ID".into()],
            tags: vec!["cloud".into(), "edge".into(), "deploy".into()],
            token_url: Some("https://dash.cloudflare.com/profile/api-tokens".into()),
            token_help: Some("API token with needed zone/account permissions".into()),
            publisher: "Cloudflare".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-aws-cloudwatch".into(),
            name: "AWS CloudWatch".into(),
            description: "Logs, metrics, alarms — official AWS Labs server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["awslabs.cloudwatch-mcp-server@latest".into()],
            },
            env_keys: vec!["AWS_ACCESS_KEY_ID".into(), "AWS_SECRET_ACCESS_KEY".into(), "AWS_REGION".into()],
            tags: vec!["cloud".into(), "monitoring".into(), "aws".into()],
            token_url: Some("https://console.aws.amazon.com/iam/home#/security_credentials".into()),
            token_help: Some("IAM access keys with CloudWatchLogsReadOnlyAccess".into()),
            publisher: "AWS Labs".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-aws-api".into(),
            name: "AWS API".into(),
            description: "Unified access to all AWS services via CLI commands (EC2, S3, IAM, Lambda, RDS...) — official AWS Labs server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["awslabs.aws-api-mcp-server@latest".into()],
            },
            env_keys: vec!["AWS_ACCESS_KEY_ID".into(), "AWS_SECRET_ACCESS_KEY".into(), "AWS_REGION".into()],
            tags: vec!["cloud".into(), "aws".into(), "infrastructure".into(), "devops".into()],
            token_url: Some("https://console.aws.amazon.com/iam/home#/security_credentials".into()),
            token_help: Some("IAM access keys, or set AWS_API_MCP_PROFILE_NAME to use a named profile. Single-user only.".into()),
            publisher: "AWS Labs".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-docker".into(),
            name: "Docker".into(),
            description: "Container and image management — official MCP server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-docker".into()],
            },
            env_keys: vec![],
            tags: vec!["containers".into(), "devops".into()],
            token_url: None,
            token_help: None,
            publisher: "Docker".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-kubernetes".into(),
            name: "Kubernetes".into(),
            description: "Pods, deployments, services, Helm, multi-cluster, OpenShift — Red Hat containers server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["kubernetes-mcp-server".into()],
            },
            env_keys: vec![],
            tags: vec!["containers".into(), "cloud".into(), "devops".into(), "infrastructure".into()],
            token_url: Some("https://github.com/containers/kubernetes-mcp-server".into()),
            token_help: Some("Uses your local kubeconfig (~/.kube/config). No API key needed. Supports --read-only flag for safe usage. Also available as Go binary and Docker image.".into()),
            publisher: "Red Hat".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Search & Web ────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-brave-search".into(),
            name: "Brave Search".into(),
            description: "Web search and local search — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-brave-search".into()],
            },
            env_keys: vec!["BRAVE_API_KEY".into()],
            tags: vec!["search".into(), "web".into()],
            token_url: Some("https://brave.com/search/api/".into()),
            token_help: Some("Brave Search API key (free tier available)".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-fetch".into(),
            name: "Fetch".into(),
            description: "HTTP requests and web content extraction — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-fetch".into()],
            },
            env_keys: vec![],
            tags: vec!["web".into(), "http".into()],
            token_url: None,
            token_help: None,
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Analytics & Monitoring ──────────────────────────────────────────
        McpDefinition {
            id: "mcp-sentry".into(),
            name: "Sentry".into(),
            description: "Error monitoring, crash reporting — official Sentry server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@sentry/mcp-server".into()],
            },
            env_keys: vec!["SENTRY_AUTH_TOKEN".into()],
            tags: vec!["monitoring".into(), "errors".into()],
            token_url: Some("https://sentry.io/settings/auth-tokens/".into()),
            token_help: Some("Auth token with project:read scope".into()),
            publisher: "Sentry".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-grafana".into(),
            name: "Grafana".into(),
            description: "Dashboards, datasources, alerts, incidents, Prometheus, Loki — official Grafana server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-grafana".into()],
            },
            env_keys: vec!["GRAFANA_URL".into(), "GRAFANA_SERVICE_ACCOUNT_TOKEN".into()],
            tags: vec!["monitoring".into(), "dashboards".into(), "observability".into(), "prometheus".into(), "loki".into()],
            token_url: Some("https://grafana.com/docs/grafana/latest/administration/service-accounts/".into()),
            token_help: Some("Service account token with Viewer role minimum. Optional: GRAFANA_ORG_ID for multi-org".into()),
            publisher: "Grafana Labs".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Communication ───────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-slack".into(),
            name: "Slack".into(),
            description: "Messages, channels, users — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-slack".into()],
            },
            env_keys: vec!["SLACK_BOT_TOKEN".into(), "SLACK_TEAM_ID".into()],
            tags: vec!["communication".into(), "chat".into()],
            token_url: Some("https://api.slack.com/apps".into()),
            token_help: Some("Bot User OAuth Token (xoxb-...)".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-microsoft-365".into(),
            name: "Microsoft 365".into(),
            description: "Outlook (mail, calendar), Teams (chat), OneDrive, OneNote — Microsoft Graph via Softeria server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@softeria/ms-365-mcp-server".into(), "--org-mode".into()],
            },
            env_keys: vec![
                "MS365_MCP_TENANT_ID".into(),
                "MS365_MCP_CLIENT_ID".into(),
            ],
            tags: vec!["communication".into(), "email".into(), "calendar".into(), "teams".into(), "microsoft".into()],
            token_url: Some("https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade".into()),
            token_help: Some("Community server (Softeria). Uses device code flow — at first launch, visit https://microsoft.com/devicelogin and enter the code shown. Leave env vars empty for the built-in app, or use your own Azure app (recommended for organizations).".into()),
            publisher: "Softeria (community)".into(),
            official: false,
            alt_packages: vec!["@merill/lokka".into(), "@pnp/cli-microsoft365-mcp-server".into()],
            default_context: None,
            api_spec: None,
        },
        // ── Project Management ──────────────────────────────────────────────
        McpDefinition {
            id: "mcp-linear".into(),
            name: "Linear".into(),
            description: "Issues, projects, teams — official Linear SSE server".into(),
            transport: McpTransport::Sse {
                url: "https://mcp.linear.app/sse".into(),
            },
            env_keys: vec![],
            tags: vec!["project-management".into(), "issues".into()],
            token_url: Some("https://linear.app/settings/api".into()),
            token_help: Some("OAuth via browser on first connection — no API key needed".into()),
            publisher: "Linear".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // Atlassian — hybrid MCP (Stdio for agents) + REST API for ApiCall
        // steps. Auth Cloud = Basic `email:api_token`; the same token also
        // works against `mcp-atlassian` via JIRA_API_TOKEN, so the user
        // configures one set of credentials and both surfaces light up.
        // base_url is templated (`{JIRA_URL}`) so each user/workspace can
        // point Kronn at their own `https://acme.atlassian.net` without a
        // separate plugin per workspace.
        McpDefinition {
            id: "mcp-atlassian".into(),
            name: "Atlassian (Jira + Confluence)".into(),
            description: "Jira issues / search / projects / Confluence — MCP for agents (Quick Prompts) + REST API for désagentified ApiCall steps".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-atlassian".into()],
            },
            env_keys: vec![
                "JIRA_URL".into(), "JIRA_USERNAME".into(), "JIRA_API_TOKEN".into(),
                "CONFLUENCE_URL".into(), "CONFLUENCE_USERNAME".into(), "CONFLUENCE_API_TOKEN".into(),
            ],
            tags: vec!["project-management".into(), "jira".into(), "confluence".into(), "api".into()],
            token_url: Some("https://id.atlassian.com/manage-profile/security/api-tokens".into()),
            token_help: Some("Cloud only (REST API): create an API token at id.atlassian.com → Security → API tokens. JIRA_USERNAME = your Atlassian email; JIRA_API_TOKEN = the token. JIRA_URL = your workspace, e.g. https://acme.atlassian.net (no trailing slash). The same credentials power the MCP server (used by agents in Quick Prompts) and REST API (used by ApiCall workflow steps).".into()),
            publisher: "Atlassian".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: Some(ApiSpec {
                // Templated — interpolated against the encrypted env at
                // request time. Without `JIRA_URL` set, the executor
                // surfaces an explicit "unresolved env placeholder" error.
                base_url: "{JIRA_URL}".into(),
                auth: ApiAuthKind::Basic {
                    user_env: "JIRA_USERNAME".into(),
                    password_env: "JIRA_API_TOKEN".into(),
                },
                docs_url: Some("https://developer.atlassian.com/cloud/jira/platform/rest/v3/intro/".into()),
                config_keys: vec![
                    ApiConfigKey {
                        env_key: "JIRA_URL".into(),
                        label: "Workspace URL".into(),
                        placeholder: "https://acme.atlassian.net".into(),
                        description: "Your Atlassian Cloud workspace URL — no trailing slash. Each Kronn project can wire one workspace; duplicate the plugin to track several.".into(),
                    },
                ],
                endpoints: vec![
                    // ── Sanity check ────────────────────────────────────
                    ApiEndpoint { path: "/rest/api/3/myself".into(),                       method: "GET".into(),  description: "[USER · sanity] Authenticated user — confirms credentials work. 401 = wrong email or expired token; 403 = no API access on the account.".into() },
                    // ── Search (the killer endpoint for backlog ops) ────
                    // Atlassian removed `/rest/api/3/search` in April 2025 (CHANGE-2046)
                    // — it now returns 410 Gone. The replacement is `/rest/api/3/search/jql`
                    // with cursor pagination (`nextPageToken`) instead of offset.
                    ApiEndpoint { path: "/rest/api/3/search/jql".into(),                   method: "GET".into(),  description: "[SEARCH · JQL] The headline endpoint (replaces the deprecated /rest/api/3/search, 410 since 2025-04). Query: `jql=project = KR AND status = Open ORDER BY priority DESC` (URL-encoded). `fields=summary,status,priority,assignee` to limit response size. Pagination via `nextPageToken` (cursor — pass it back as `nextPageToken=…` for the next page). Returns `{issues: [...], nextPageToken, isLast}`.".into() },
                    ApiEndpoint { path: "/rest/api/3/search/approximate-count".into(),    method: "POST".into(), description: "[SEARCH · count] Approximate total result count for a JQL — Atlassian split the count out of /search/jql to keep that endpoint cheap. Body: `{\"jql\": \"project = KR\"}`. Returns `{count: 173}`. Use sparingly, the count can be off-by-a-few on heavy projects.".into() },
                    // ── Issues ──────────────────────────────────────────
                    ApiEndpoint { path: "/rest/api/3/issue/{issueIdOrKey}".into(),         method: "GET".into(),  description: "[ISSUES · single] Full payload of one issue. Path placeholder `{issueIdOrKey}` = key (`KR-123`) or numeric id. `fields=...` query param to slice fields. `expand=changelog,renderedFields` for transitions + ADF rendered HTML.".into() },
                    ApiEndpoint { path: "/rest/api/3/issue/{issueIdOrKey}/comment".into(), method: "GET".into(),  description: "[ISSUES · comments] Comments on an issue (paginated, `startAt`+`maxResults`).".into() },
                    ApiEndpoint { path: "/rest/api/3/issue/{issueIdOrKey}/transitions".into(), method: "GET".into(), description: "[ISSUES · transitions] Available workflow transitions for an issue (id + name + target status). Use this before POSTing a transition by id.".into() },
                    // ── Projects ────────────────────────────────────────
                    ApiEndpoint { path: "/rest/api/3/project/search".into(),               method: "GET".into(),  description: "[PROJECTS · list] Paginated project list. Query: `query=ACME` for name/key match, `expand=lead,description`. Replaces the deprecated `GET /project`.".into() },
                    ApiEndpoint { path: "/rest/api/3/project/{projectIdOrKey}".into(),     method: "GET".into(),  description: "[PROJECTS · single] One project — components, lead, issue types. `expand=description,lead,issueTypes`.".into() },
                    ApiEndpoint { path: "/rest/api/3/project/{projectIdOrKey}/components".into(), method: "GET".into(), description: "[PROJECTS · components] Components defined for a project — useful for assignee resolution.".into() },
                    ApiEndpoint { path: "/rest/api/3/project/{projectIdOrKey}/versions".into(),   method: "GET".into(), description: "[PROJECTS · versions] Released / unreleased versions for a project.".into() },
                    // ── Schema introspection ────────────────────────────
                    ApiEndpoint { path: "/rest/api/3/field".into(),                        method: "GET".into(),  description: "[SCHEMA · fields] All fields visible to the user, including custom fields with their `customfield_NNNNN` ids. Use this to map `Story Points` → `customfield_10016` before searching.".into() },
                    // ── Filters ─────────────────────────────────────────
                    ApiEndpoint { path: "/rest/api/3/filter/search".into(),                method: "GET".into(),  description: "[FILTERS] User-saved JQL filters. Query: `accountId=...&filterName=Backlog`. Surfaces the JQL of each filter in `jql`.".into() },
                ],
            }),
        },
        // ── Design ──────────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-figma".into(),
            name: "Figma".into(),
            description: "Read Figma files, components, styles and variables — official Figma Dev Mode server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "figma-developer-mcp".into(), "--stdio".into()],
            },
            env_keys: vec!["FIGMA_API_KEY".into()],
            tags: vec!["design".into(), "ui".into()],
            token_url: Some("https://www.figma.com/settings".into()),
            token_help: Some("Personal access token from Figma Settings > Personal access tokens".into()),
            publisher: "Figma".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-drawio".into(),
            name: "draw.io".into(),
            description: "Create and edit diagrams (flowcharts, UML, architecture) — official jgraph/draw.io server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "drawio-mcp".into()],
            },
            env_keys: vec![],
            tags: vec!["design".into(), "diagrams".into(), "architecture".into()],
            token_url: None,
            token_help: None,
            publisher: "jgraph".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Files & Utilities ───────────────────────────────────────────────
        McpDefinition {
            id: "mcp-filesystem".into(),
            name: "Filesystem".into(),
            description: "Read/write local files — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
            },
            env_keys: vec![],
            tags: vec!["core".into(), "filesystem".into()],
            token_url: None,
            token_help: None,
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // Puppeteer removed — use Playwright (mcp-playwright) instead.
        McpDefinition {
            id: "mcp-chrome-devtools".into(),
            name: "Chrome DevTools".into(),
            description: "Debug, inspect DOM/CSS, network, performance traces — official Google server. Requires Chrome installed on the host machine.".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "chrome-devtools-mcp@latest".into()],
            },
            env_keys: vec![],
            tags: vec!["browser".into(), "debug".into(), "devtools".into(), "testing".into()],
            token_url: None,
            token_help: Some("Requires Google Chrome (stable) installed. Use --headless for servers without display.".into()),
            publisher: "Google".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-playwright".into(),
            name: "Playwright".into(),
            description: "Cross-browser automation and E2E testing — official Microsoft server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@playwright/mcp@latest".into()],
            },
            env_keys: vec![],
            tags: vec!["browser".into(), "testing".into(), "e2e".into()],
            token_url: None,
            token_help: Some("Run 'npx playwright install' first to download browser binaries".into()),
            publisher: "Microsoft".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-context7".into(),
            name: "Context7".into(),
            description: "Up-to-date docs for any library — official Upstash server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@upstash/context7-mcp@latest".into()],
            },
            env_keys: vec![],
            tags: vec!["docs".into(), "libraries".into()],
            token_url: None,
            token_help: None,
            publisher: "Upstash".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Payments ───────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-stripe".into(),
            name: "Stripe".into(),
            description: "Payments, subscriptions, customers — official Stripe server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@stripe/mcp".into(), "--tools=all".into()],
            },
            env_keys: vec!["STRIPE_SECRET_KEY".into()],
            tags: vec!["payments".into(), "billing".into(), "ecommerce".into()],
            token_url: Some("https://dashboard.stripe.com/apikeys".into()),
            token_help: Some("Secret key (sk_live_... or sk_test_...)".into()),
            publisher: "Stripe".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Knowledge & Docs ──────────────────────────────────────────────
        McpDefinition {
            id: "mcp-notion".into(),
            name: "Notion".into(),
            description: "Pages, databases, docs — official Notion server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@notionhq/notion-mcp-server".into()],
            },
            env_keys: vec!["NOTION_TOKEN".into()],
            tags: vec!["docs".into(), "knowledge".into(), "wiki".into()],
            token_url: Some("https://www.notion.so/profile/integrations".into()),
            token_help: Some("Internal integration token (secret_...)".into()),
            publisher: "Notion".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── BaaS ──────────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-supabase".into(),
            name: "Supabase".into(),
            description: "Managed Postgres, auth, storage — official Supabase server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@supabase/mcp-server-supabase@latest".into()],
            },
            env_keys: vec!["SUPABASE_ACCESS_TOKEN".into()],
            tags: vec!["database".into(), "auth".into(), "cloud".into()],
            token_url: Some("https://supabase.com/dashboard/account/tokens".into()),
            token_help: Some("Personal access token from Supabase dashboard".into()),
            publisher: "Supabase".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── SEO ───────────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-ahrefs".into(),
            name: "Ahrefs".into(),
            description: "SEO analysis, keywords, backlinks, rank tracking — official Ahrefs server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@ahrefs/mcp".into()],
            },
            env_keys: vec!["API_KEY".into()],
            tags: vec!["seo".into(), "analytics".into(), "marketing".into()],
            token_url: Some("https://app.ahrefs.com/api".into()),
            token_help: Some("API v3 key (requires Ahrefs subscription)".into()),
            publisher: "Ahrefs".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Git (local) ──────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-git".into(),
            name: "Git".into(),
            description: "Local git repository operations — official MCP server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-git".into()],
            },
            env_keys: vec![],
            tags: vec!["git".into(), "code".into(), "vcs".into()],
            token_url: None,
            token_help: None,
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Email ─────────────────────────────────────────────────────────
        // Resend — hybrid plugin (MCP for agents in Quick Prompts +
        // REST API for zero-token ApiCall steps). Same pattern as
        // `mcp-github` / `mcp-atlassian`: one entry, one credential
        // (`RESEND_API_KEY`), two surfaces. Pick MCP for exploratory
        // chat; pick API for deterministic CSM/lifecycle pipelines
        // where the send step must stay at ~0 tokens.
        McpDefinition {
            id: "mcp-resend".into(),
            name: "Resend".into(),
            description: "Transactional + lifecycle email — MCP for agents (Quick Prompts) + REST API for désagentified ApiCall steps (CSM email pipelines at ~0 tokens/send)".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "resend-mcp".into()],
            },
            env_keys: vec!["RESEND_API_KEY".into()],
            tags: vec!["email".into(), "mailing".into(), "communication".into(), "transactional".into(), "api".into(), "csm".into()],
            token_url: Some("https://resend.com/api-keys".into()),
            token_help: Some("API key from Resend dashboard (https://resend.com/api-keys). Prefix `re_`. The same token powers both the MCP transport (used by agents in Quick Prompts) and the REST API (used by ApiCall workflow steps) — configure once. ⚠ Before sending, register and verify the sending domain at https://resend.com/domains — the `from` address MUST be on a verified domain (or use `onboarding@resend.dev` for tests).".into()),
            publisher: "Resend".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# Resend — Usage Context

> Instructions for agents calling the **Resend REST API** via curl.

Resend is a developer-first email API. Two send patterns matter:
single (`POST /emails`) and batch (`POST /emails/batch`, up to 100).
For lifecycle/CSM flows, batch + the `Idempotency-Key` header is the
right combo: cheap and replay-safe.

## 1. Auth — Bearer token (already injected by Kronn)

```
Authorization: Bearer re_xxxxxxxx
Content-Type: application/json
```

Do NOT suggest the key in `headers` — Kronn injects it. Just hit the
endpoint with the JSON body.

## 2. Send one email — `POST /emails`

```bash
curl -X POST "https://api.resend.com/emails" \
  -H "Authorization: Bearer $RESEND_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: csm-followup-{user_id}-{date}" \
  -d '{
    "from": "Acme <hello@acme.dev>",
    "to": ["user@example.com"],
    "subject": "Quick check-in",
    "html": "<p>Hi there — saw you logged in 3 times last week…</p>",
    "tags": [
      {"name": "category", "value": "csm_followup"},
      {"name": "user_id", "value": "{user_id}"}
    ]
  }'
```

Response: `{"id": "re_xxx"}` — store it for tracking + webhook
correlation.

### Required fields
- `from` — `"Display Name <addr@verified-domain.tld>"` OR `"addr@…"`.
  **The domain MUST be verified** in `https://resend.com/domains`,
  otherwise you get a 422 `"The from address is not valid"` even when
  the address itself is well-formed. For tests, use the open sandbox
  domain `onboarding@resend.dev`.
- `to` — array of strings (max 50 in a single request).
- `subject` — string.
- Either `html` **or** `text` (one of the two required; both allowed).

### Optional
- `cc`, `bcc` — arrays of strings.
- `reply_to` — STRING (singular), not array.
- `headers` — `{"X-Entity-Ref-ID": "…", "List-Unsubscribe": "<…>", "X-Tag": "…"}`.
  Custom headers passthrough. Useful for List-Unsubscribe on marketing.
- `attachments` — `[{filename, content (base64), content_type?}]`. Max
  total payload 40MB. **Not supported in batch.**
- `scheduled_at` — ISO 8601 (`"2026-05-20T14:00:00Z"`) or natural
  language (`"in 1 hour"`). **Not supported in batch.**
- `tags` — `[{name, value}]` for analytics. Keys/values ASCII letters,
  digits, `_`, `-` (no spaces, no `@`, no `.`). **Hard rule** — Resend
  silently drops a tag whose key has a space.

## 3. Batch send — `POST /emails/batch`

Body is a JSON **array** (not an envelope object). One Resend call,
up to 100 messages, charged as 100 sends. Perfect for CSM fan-out.

```bash
curl -X POST "https://api.resend.com/emails/batch" \
  -H "Authorization: Bearer $RESEND_API_KEY" \
  -H "Content-Type: application/json" \
  -d '[
    {"from":"Acme <hello@acme.dev>","to":["a@x.com"],"subject":"…","html":"…"},
    {"from":"Acme <hello@acme.dev>","to":["b@x.com"],"subject":"…","html":"…"}
  ]'
```

Response: `{"data":[{"id":"…"},{"id":"…"}, …]}` — index-aligned with
the request.

**Restrictions vs single send:**
- No `attachments`.
- No `scheduled_at`.
- ALL messages in the array must validate; a single bad `from` rejects
  the whole batch with `422`. Validate the payload locally first.

## 4. Idempotency — `Idempotency-Key` header

Pass `Idempotency-Key: <stable-string>` on `POST /emails` and
`POST /emails/batch`. Resend returns the original response for repeated
calls within 24h. **Always set it on CSM workflows** — a retry must
not double-send.

Recommended shape: `{workflow_run_id}-{user_id}` so re-runs of the same
workflow on the same user are idempotent but DIFFERENT users still
go through.

## 5. Retrieve email status — `GET /emails/{id}`

```bash
curl "https://api.resend.com/emails/re_xxx" \
  -H "Authorization: Bearer $RESEND_API_KEY"
```

Returns `last_event`: `delivered | bounced | complained | opened |
clicked | sent | …` + timestamps. Useful in a Notify/Gate followup
step to verify delivery before marking the user as "contacted" in your
DB. **Note**: opens/clicks require tracking pixels — disabled by default
on some Resend plans.

## 6. Contacts / Audiences / Broadcasts (lifecycle / marketing)

For CSM lists rather than 1-to-1 transactional:

- `GET  /audiences` — list audiences (your "lists").
- `POST /audiences` — `{name}` — create an audience.
- `POST /audiences/{audience_id}/contacts` — `{email, first_name?, last_name?, unsubscribed?}` — add or update a contact (idempotent on email).
- `GET  /audiences/{audience_id}/contacts` — paginated.
- `DELETE /audiences/{audience_id}/contacts/{id_or_email}` — remove contact.
- `POST /broadcasts` — `{audience_id, from, subject, html, name?, reply_to?, preview_text?}` — DRAFT a broadcast (not sent yet).
- `POST /broadcasts/{broadcast_id}/send` — `{scheduled_at?}` — fire it.
- `GET  /broadcasts/{id}` — status (`draft | queued | sending | sent`).

Pattern for a CSM nudge campaign:
1. `POST /audiences/{id}/contacts` to push the at-risk users.
2. `POST /broadcasts` to draft the email (templated body).
3. **Human Gate** in Kronn — operator reviews the audience + preview.
4. `POST /broadcasts/{id}/send` once approved.

## 7. Sanity check — `GET /domains`

```bash
curl "https://api.resend.com/domains" \
  -H "Authorization: Bearer $RESEND_API_KEY"
```

Returns the list of verified domains. `200` + non-empty `data` → auth
works AND at least one sending domain is ready. `401` → wrong key.
Cheaper than triggering a send to test credentials.

## 8. Error code matrix (the ones you'll actually see)

- `401 unauthorized` — `RESEND_API_KEY` revoked or wrong (does NOT
  start with `re_`).
- `403 forbidden` — domain blocked (compliance) or rate-limit ceiling
  per account.
- `422 validation_error` — most common in practice:
  - `"The from address is not valid"` → domain not in `/domains` OR
    not yet verified (DNS records pending). Check there first.
  - `"to must contain valid email addresses"` → typo, or testing with
    `example.com` (Resend rejects RFC-2606 reserved TLDs in prod).
  - `"missing_required_field"` → one of `from`, `to`, `subject`,
    `html`/`text`.
- `429 rate_limit_exceeded` — default 2 req/s, 10 req/s on Pro.
  Response includes `Retry-After` seconds. Solution: switch to
  `/emails/batch` (1 call = up to 100 messages, single rate-limit hit).
- `400 invalid_idempotency_key` — must be ≤ 256 chars, ASCII only.

## 9. Common gotchas (sorted by how much time they cost)

- **Domain not verified** — you'll waste an hour debugging "valid"
  addresses that 422. Always verify the domain in the Resend dashboard
  before launching a CSM flow. `onboarding@resend.dev` is fine for
  dev/staging but rate-limited.
- **`to` is an array, even for one recipient** — `"to": "a@x.com"`
  silently 422s.
- **`reply_to` is a STRING, not an array.** Counter-intuitive given
  `to/cc/bcc` are arrays.
- **Tags with spaces in `name` are dropped silently.** No error, no tag.
  Use `csm_followup`, not `csm followup`.
- **No `scheduled_at` in batch** — split sends into single calls if you
  need scheduling per row.
- **Webhooks vs polling** — for high-volume CSM, set up webhooks at
  `https://resend.com/webhooks` rather than polling `GET /emails/{id}`
  for every send.

Official docs: https://resend.com/docs/api-reference/introduction
"#.into()),
            api_spec: Some(ApiSpec {
                base_url: "https://api.resend.com".into(),
                auth: ApiAuthKind::Bearer {
                    env_key: "RESEND_API_KEY".into(),
                },
                docs_url: Some("https://resend.com/docs/api-reference/introduction".into()),
                config_keys: vec![],
                endpoints: vec![
                    // ── Sanity check ────────────────────────────────────────
                    ApiEndpoint { path: "/domains".into(),                              method: "GET".into(),    description: "[SANITY] List verified sending domains. Use this as the first call after wiring the key — 200 + non-empty data = auth OK + at least one usable `from` domain. 401 = wrong/revoked key.".into() },
                    // ── Send ────────────────────────────────────────────────
                    ApiEndpoint { path: "/emails".into(),                                method: "POST".into(),   description: "[SEND · single] Send one transactional email. Body: `{from, to[], subject, html|text, cc?, bcc?, reply_to?, tags?, headers?, attachments?, scheduled_at?}`. ALWAYS pass `Idempotency-Key` header for CSM/replay-safe flows. `from` MUST be on a verified domain (see /domains).".into() },
                    ApiEndpoint { path: "/emails/batch".into(),                          method: "POST".into(),   description: "[SEND · batch ≤100] Body is a JSON ARRAY (not an envelope). One call = up to 100 messages, billed accordingly, 1 rate-limit hit. NO `attachments`, NO `scheduled_at`. One bad message = whole batch 422 → validate locally first. Returns `{data: [{id}, …]}` index-aligned.".into() },
                    ApiEndpoint { path: "/emails/{email_id}".into(),                     method: "GET".into(),    description: "[SEND · status] Retrieve an email by id. Returns `last_event` (`delivered|bounced|complained|opened|clicked|sent`) + timestamps. Replace `{email_id}` with the id returned by /emails.".into() },
                    ApiEndpoint { path: "/emails/{email_id}".into(),                     method: "PATCH".into(),  description: "[SEND · reschedule] Reschedule a scheduled-but-not-yet-sent email. Body: `{scheduled_at: \"ISO8601\"}`.".into() },
                    ApiEndpoint { path: "/emails/{email_id}".into(),                     method: "DELETE".into(), description: "[SEND · cancel] Cancel a scheduled email that hasn't fired yet. Returns 200 if cancelled, 422 if already sent.".into() },
                    // ── Audiences (lifecycle / marketing lists) ─────────────
                    ApiEndpoint { path: "/audiences".into(),                             method: "GET".into(),    description: "[AUDIENCES] List audiences (Resend's term for contact lists). Paginated.".into() },
                    ApiEndpoint { path: "/audiences".into(),                             method: "POST".into(),   description: "[AUDIENCES] Create an audience. Body: `{name}`. Returns `{id, name}`.".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}".into(),                method: "GET".into(),    description: "[AUDIENCES] Single audience metadata.".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}".into(),                method: "DELETE".into(), description: "[AUDIENCES] Delete an audience. Contacts inside are also removed.".into() },
                    // ── Contacts (members of audiences) ─────────────────────
                    ApiEndpoint { path: "/audiences/{audience_id}/contacts".into(),       method: "POST".into(),   description: "[CONTACTS] Add or upsert a contact in an audience. Body: `{email, first_name?, last_name?, unsubscribed?}`. Idempotent on `email`. Useful in a CSM batch: push the at-risk user list, then create a broadcast.".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}/contacts".into(),       method: "GET".into(),    description: "[CONTACTS] List contacts in an audience. Paginated.".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}/contacts/{id}".into(),  method: "GET".into(),    description: "[CONTACTS] Single contact by id or email.".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}/contacts/{id}".into(),  method: "PATCH".into(),  description: "[CONTACTS] Update a contact (e.g. `unsubscribed: true` to suppress).".into() },
                    ApiEndpoint { path: "/audiences/{audience_id}/contacts/{id}".into(),  method: "DELETE".into(), description: "[CONTACTS] Remove a contact from the audience.".into() },
                    // ── Broadcasts (audience-wide sends) ────────────────────
                    ApiEndpoint { path: "/broadcasts".into(),                            method: "GET".into(),    description: "[BROADCASTS] List broadcasts (drafts + sent).".into() },
                    ApiEndpoint { path: "/broadcasts".into(),                            method: "POST".into(),   description: "[BROADCASTS · draft] Create a broadcast (NOT yet sent). Body: `{audience_id, from, subject, html|text, name?, reply_to?, preview_text?}`. Use this then human-Gate in Kronn before firing.".into() },
                    ApiEndpoint { path: "/broadcasts/{broadcast_id}".into(),              method: "GET".into(),    description: "[BROADCASTS] Status: `draft|queued|sending|sent`.".into() },
                    ApiEndpoint { path: "/broadcasts/{broadcast_id}/send".into(),         method: "POST".into(),   description: "[BROADCASTS · fire] Send the broadcast NOW or schedule it. Body: `{scheduled_at?: \"ISO8601\"}`. Returns 200 + status `queued`.".into() },
                    ApiEndpoint { path: "/broadcasts/{broadcast_id}".into(),              method: "DELETE".into(), description: "[BROADCASTS] Delete a draft broadcast. Already-sent broadcasts cannot be deleted.".into() },
                    // ── API keys (read-only — managing keys via API is rare) ──
                    ApiEndpoint { path: "/api-keys".into(),                              method: "GET".into(),    description: "[KEYS · introspection] List API keys (metadata only — secrets never returned).".into() },
                ],
            }),
        },
        // Mailjet — EU-friendly transactional + marketing email API. The
        // counterweight to Resend for users with RGPD / data-residency
        // constraints (media, banking, public sector). Auth = HTTP Basic
        // `api_key:api_secret`, the wire format Mailjet has shipped since
        // v3. Send API v3.1 is the modern endpoint — body is a
        // `{Messages: [...]}` envelope. **Sender validation** is the
        // single biggest pitfall: `From.Email` must exist in
        // `/v3/REST/sender` and be validated, otherwise 400.
        McpDefinition {
            id: "api-mailjet".into(),
            name: "Mailjet".into(),
            description: "Mailjet REST API — EU-hosted transactional + marketing email (Send API v3.1, contacts, lists, templates, stats). Pair with a Kronn Gate for human-approved CSM flows. Sister provider to Resend (`mcp-resend`) for EU/RGPD use cases.".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec!["MAILJET_API_KEY".into(), "MAILJET_API_SECRET".into()],
            tags: vec!["email".into(), "mailing".into(), "communication".into(), "transactional".into(), "marketing".into(), "api".into(), "eu".into(), "csm".into()],
            token_url: Some("https://app.mailjet.com/account/apikeys".into()),
            token_help: Some("API key + Secret key pair from Mailjet → Account Settings → API Key Management. Both halves required. ⚠ Before sending, register a sender at https://app.mailjet.com/account/sender (or validate a whole domain) — `From.Email` MUST match a validated sender or you get 400. Sandbox mode (`SandboxMode: true` in body) lets you dry-run without actually sending.".into()),
            publisher: "Mailjet".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# Mailjet — Usage Context

> Instructions for agents calling the **Mailjet REST API** via curl.

Mailjet is an EU-hosted email API (data residency in Paris/Brussels)
covering transactional + marketing in one product. The send surface
moved from a flat body (v3) to an envelope-array (v3.1) — **always
target v3.1 for new code**; v3 is kept only for legacy callers.

## 1. Auth — HTTP Basic with api_key:api_secret (already injected by Kronn)

```
Authorization: Basic <base64(MAILJET_API_KEY:MAILJET_API_SECRET)>
Content-Type: application/json
```

Both halves come from Mailjet → Account Settings → API Key Management.
Kronn injects both — never put the credentials in `headers` or the URL.

## 2. Send one email — `POST /v3.1/send` (the only send you should use)

```bash
curl -X POST "https://api.mailjet.com/v3.1/send" \
  -u "$MAILJET_API_KEY:$MAILJET_API_SECRET" \
  -H "Content-Type: application/json" \
  -d '{
    "Messages": [
      {
        "From":    { "Email": "hello@acme.eu", "Name": "Acme" },
        "To":      [{ "Email": "user@example.com", "Name": "User" }],
        "Subject": "Quick check-in",
        "TextPart": "Hi there…",
        "HTMLPart": "<p>Hi there — saw you logged in 3 times last week…</p>",
        "CustomID": "csm-followup-{user_id}-{date}",
        "EventPayload": "{\"workflow_run\":\"…\",\"user_id\":\"…\"}"
      }
    ]
  }'
```

Response shape:

```json
{
  "Messages": [
    {
      "Status": "success",
      "CustomID": "csm-followup-…",
      "To": [{ "Email": "user@example.com", "MessageUUID": "…", "MessageID": 12345, "MessageHref": "https://api.mailjet.com/v3/REST/message/12345" }]
    }
  ]
}
```

**Status values:** `success`, `error`. Partial failures are surfaced
per-message — the HTTP code can be 200 even with one message in error.
**Always iterate `Messages[].Status`**, do not just check `response.ok`.

## 3. Batch send — same endpoint, multiple `Messages`

There is no separate batch URL. You just push N messages into the
`Messages` array (limit ~50 per request, ~500 KB total).

```json
{
  "Messages": [
    { "From": {…}, "To": [{…}], "Subject": "…", "HTMLPart": "…" },
    { "From": {…}, "To": [{…}], "Subject": "…", "HTMLPart": "…" }
  ]
}
```

Each message is processed independently. Mix and match templates,
recipients, languages in the same call.

## 4. Required fields per message

- `From.Email` — **must be a validated sender** (see §7). 400 if not.
- `To` — array of `{Email, Name?}` objects. **Always an array**, even
  for one recipient.
- `Subject` — string.
- Either `TextPart` or `HTMLPart` (one minimum, both allowed).

## 5. Useful optional fields

- `Cc`, `Bcc` — same shape as `To`.
- `ReplyTo` — `{Email, Name?}` object (NOT a string, unlike Resend).
- `TemplateID` — integer (id from `/v3/REST/template`). Combine with
  `TemplateLanguage: true` to enable MJML / variable interpolation.
- `Variables` — `{key: value, …}` injected into the template.
- `TemplateErrorReporting` — `{Email, Name?}` — where to send template
  rendering errors.
- `TemplateErrorDeliver` — boolean. `true` = deliver the email even if
  template rendering errors out (useful when the fallback is "send a
  raw email rather than fail silently").
- `Headers` — `{...}` for custom headers (List-Unsubscribe, X-Tag, etc.).
- `Attachments`, `InlinedAttachments` — `[{ContentType, Filename, Base64Content}]`.
- `Priority` — `0..3` (0 = lowest, 2 = normal default, 3 = highest).
- `CustomCampaign`, `DeduplicateCampaign` — group sends by campaign name + dedupe.
- `TrackOpens`, `TrackClicks` — `"account_default" | "enabled" | "disabled"`.
- `CustomID` — your free-form trace id, echoed in response + webhooks.
  **Use this to correlate sends with workflow runs**.
- `EventPayload` — opaque string returned in webhooks (max 1KB). Pack
  JSON metadata here for stateless event handlers.
- `SandboxMode` — `true` = validate without sending (no charge, no
  delivery). Perfect for CSM dry-runs / Gate previews.

## 6. Sandbox / dry-run

Set `SandboxMode: true` at the message level OR top-level. Mailjet
validates the payload (sender, template, recipients) and returns the
same response shape — **without sending**. Use this in a Gate-preview
step to surface "would-be sent" to the human reviewer.

```json
{
  "SandboxMode": true,
  "Messages": [ … ]
}
```

## 7. Senders — the #1 pitfall

`From.Email` must be a validated sender registered for THIS api_key.
You manage them via `/v3/REST/sender`.

- `GET /v3/REST/sender` — list (filter by `IsDomain=false` for emails,
  `=true` for whole domains). Returns `{Data: [{Email, Status, …}]}`.
- `POST /v3/REST/sender` — `{Email}` — register. Mailjet emails the
  address with a verification link.
- `POST /v3/REST/sender/{id}/validate` — re-trigger validation.

**Status** values:
- `Active` → usable.
- `Inactive` → registered but not yet validated.
- `Deleted` → don't try, will 400.

If you 400 with `Sender not allowed for this account`, run
`GET /v3/REST/sender` first and pick from the `Active` rows.

## 8. Contacts + lists (CSM / lifecycle)

- `GET  /v3/REST/contact` — list, paginated (`Limit`, `Offset`).
- `POST /v3/REST/contact` — `{Email, Name?}` — register a contact.
- `GET  /v3/REST/contact/{id_or_email}` — single contact (id or email).
- `PUT  /v3/REST/contactdata/{contact_id}` — `{Data: [{Name, Value}, …]}`
  set custom properties for a contact.
- `GET  /v3/REST/contactslist` — list contact lists.
- `POST /v3/REST/contactslist` — `{Name}` — create a list.
- `POST /v3/REST/contactslist/{list_id}/managecontact` — **the killer
  endpoint** — `{Email, Action, Properties?}` where `Action` is
  `addnoforce | addforce | remove | unsub`. Use this to push CSM
  signals: at-risk users into "at-risk", churned into "churned", etc.
- `POST /v3/REST/contactslist/{list_id}/managemanycontacts` — bulk
  version (up to 1000 contacts in one async job).

## 9. Templates

- `GET /v3/REST/template` — list templates (filter `OwnerType=user`).
- `GET /v3/REST/template/{id}/detailcontent` — full HTML/MJML + variables.

Reference a template in a send via `TemplateID` (integer). Use
`TemplateLanguage: true` to enable Mailjet's variable syntax
(`{{var:firstname:""}}`) and conditional blocks.

## 10. Stats

- `GET /v3/REST/statcounters` — aggregated counters. Query:
  `CounterSource=APIKey`, `CounterResolution=Day`, `CounterTiming=Message`,
  `FromTS=<epoch>`, `ToTS=<epoch>`.
- `GET /v3/REST/messagesentstatistics` — per-message stats (opens,
  clicks per message-id).

## 11. Error matrix

- `401 unauthorized` — wrong api_key:api_secret pair (one half wrong
  is enough). Re-check both env vars.
- `400 Bad Request`:
  - `"Invalid email format"` → typo in `From.Email` or `To[].Email`.
  - `"Sender not allowed for this account"` → `From.Email` not in
    `/v3/REST/sender` or `Status != Active`. **The most common 400.**
  - `"Either TextPart or HTMLPart is required"` → both omitted.
  - `"Argument value is invalid for property: …"` → check field types
    (e.g. `Priority` is int, `TrackOpens` is string).
- `403 forbidden` — sub-account permissions / IP allowlist mismatch.
- `429 Too Many Requests` — Mailjet rate-limits per account tier
  (default 500/hour for transactional, more on paid plans). Header
  `Retry-After: <seconds>`.
- `200` with `Messages[].Status == "error"` — partial failure.
  Always inspect each message's status, do not stop at HTTP-200.

## 12. Common gotchas (sorted by pain)

- **`From.Email` not validated** — accounts for ~70% of integration
  failures. Run `GET /v3/REST/sender` and copy the exact `Email` value.
- **Status check on partial failures** — a 200 OK does NOT mean all
  messages were sent. Loop `Messages[].Status`.
- **`v3` legacy URL** — `POST /v3/send` (flat body, `FromEmail`,
  `Recipients`) is still alive but property names changed in v3.1.
  Don't mix the two flavors in one workflow.
- **CustomID is for YOU, MessageID is the Mailjet id** — webhooks
  return both. Persist `CustomID` to correlate with your DB.
- **EU region** — `https://api.mailjet.com` is the canonical URL for
  ALL accounts (EU/US). There is no separate EU subdomain — data
  residency is set per account at signup, not at the URL.

Official docs: https://dev.mailjet.com/email/reference/
"#.into()),
            api_spec: Some(ApiSpec {
                base_url: "https://api.mailjet.com".into(),
                auth: ApiAuthKind::Basic {
                    user_env: "MAILJET_API_KEY".into(),
                    password_env: "MAILJET_API_SECRET".into(),
                },
                docs_url: Some("https://dev.mailjet.com/email/reference/".into()),
                config_keys: vec![],
                endpoints: vec![
                    // ── Sanity check / sender validation ───────────────────
                    ApiEndpoint { path: "/v3/REST/sender".into(),                                  method: "GET".into(),  description: "[SENDER · sanity] List validated senders. Run this FIRST when wiring the key — 200 + at least one row with `Status: Active` = auth OK and at least one usable `From.Email`. 401 = wrong key/secret pair.".into() },
                    ApiEndpoint { path: "/v3/REST/sender".into(),                                  method: "POST".into(), description: "[SENDER] Register a new sender. Body: `{Email}` (single address) or `{Email: \"*@yourdomain.eu\"}` (whole domain). Mailjet sends a verification email.".into() },
                    ApiEndpoint { path: "/v3/REST/sender/{id}/validate".into(),                    method: "POST".into(), description: "[SENDER] Re-trigger the validation email for an `Inactive` sender.".into() },
                    // ── Send (the headline endpoint) ───────────────────────
                    ApiEndpoint { path: "/v3.1/send".into(),                                       method: "POST".into(), description: "[SEND · v3.1] Modern send. Body is `{Messages: [{From, To[], Subject, HTMLPart|TextPart, TemplateID?, Variables?, CustomID?, EventPayload?, SandboxMode?}, …]}`. Up to ~50 messages per call. **Always check `Messages[].Status`** — a 200 can hide per-message errors. `SandboxMode: true` = validate without sending.".into() },
                    ApiEndpoint { path: "/v3/send".into(),                                         method: "POST".into(), description: "[SEND · v3 legacy] Old flat body (`FromEmail`, `Recipients`, `Text-part`, `Html-part`). Kept only for legacy callers — prefer /v3.1/send for new code.".into() },
                    // ── Sent message introspection ─────────────────────────
                    ApiEndpoint { path: "/v3/REST/message".into(),                                 method: "GET".into(),  description: "[MESSAGES] List sent messages. Filter by `Contact_ID`, `CustomID`, `FromTS`, `ToTS`. Returns delivery status + open/click counts.".into() },
                    ApiEndpoint { path: "/v3/REST/message/{message_id}".into(),                   method: "GET".into(),  description: "[MESSAGES] Single message status by Mailjet `MessageID` (returned in send response).".into() },
                    ApiEndpoint { path: "/v3/REST/messagehistory/{message_id}".into(),            method: "GET".into(),  description: "[MESSAGES] Event timeline for a message (queued → sent → opened → clicked).".into() },
                    // ── Contacts ───────────────────────────────────────────
                    ApiEndpoint { path: "/v3/REST/contact".into(),                                 method: "GET".into(),  description: "[CONTACTS] List contacts. Paginated (`Limit`, `Offset` — max `Limit=1000`).".into() },
                    ApiEndpoint { path: "/v3/REST/contact".into(),                                 method: "POST".into(), description: "[CONTACTS] Create. Body: `{Email, Name?, IsExcludedFromCampaigns?}`.".into() },
                    ApiEndpoint { path: "/v3/REST/contact/{id_or_email}".into(),                  method: "GET".into(),  description: "[CONTACTS] Single contact by id or email.".into() },
                    ApiEndpoint { path: "/v3/REST/contact/{id}".into(),                            method: "PUT".into(),  description: "[CONTACTS] Update. Body: `{Name?, IsExcludedFromCampaigns?}`.".into() },
                    ApiEndpoint { path: "/v3/REST/contactdata/{contact_id}".into(),               method: "PUT".into(),  description: "[CONTACTS · props] Set custom properties. Body: `{Data: [{Name, Value}, …]}`. Properties must exist first via /v3/REST/contactmetadata.".into() },
                    // ── Contact lists (CSM segmentation) ───────────────────
                    ApiEndpoint { path: "/v3/REST/contactslist".into(),                            method: "GET".into(),  description: "[LISTS] List contact lists (paginated).".into() },
                    ApiEndpoint { path: "/v3/REST/contactslist".into(),                            method: "POST".into(), description: "[LISTS] Create a list. Body: `{Name}`.".into() },
                    ApiEndpoint { path: "/v3/REST/contactslist/{list_id}".into(),                  method: "GET".into(),  description: "[LISTS] Single list metadata.".into() },
                    ApiEndpoint { path: "/v3/REST/contactslist/{list_id}/managecontact".into(),    method: "POST".into(), description: "[LISTS · killer] Add/remove/unsub a single contact in a list. Body: `{Email, Action: \"addnoforce\"|\"addforce\"|\"remove\"|\"unsub\", Name?, Properties?}`. Idempotent on email — perfect for CSM segmentation (`at-risk`, `churned`, `power-user`).".into() },
                    ApiEndpoint { path: "/v3/REST/contactslist/{list_id}/managemanycontacts".into(), method: "POST".into(), description: "[LISTS · bulk] Async bulk variant — up to 1000 contacts. Body: `{Contacts: [{Email, Name?, Properties?}, …], Action}`. Returns a `JobID` to poll via /v3/REST/contactslist/{list_id}/managemanycontacts/{job_id}.".into() },
                    // ── Templates (transactional) ──────────────────────────
                    ApiEndpoint { path: "/v3/REST/template".into(),                                method: "GET".into(),  description: "[TEMPLATES] List templates. Filter `OwnerType=user` to exclude system templates.".into() },
                    ApiEndpoint { path: "/v3/REST/template/{template_id}/detailcontent".into(),    method: "GET".into(),  description: "[TEMPLATES] HTML/MJML body + declared variables for a template. Use to pre-fill `Variables` in /v3.1/send.".into() },
                    // ── Stats ──────────────────────────────────────────────
                    ApiEndpoint { path: "/v3/REST/statcounters".into(),                            method: "GET".into(),  description: "[STATS] Aggregated counters. Query: `CounterSource=APIKey&CounterResolution=Day&CounterTiming=Message&FromTS=<epoch>&ToTS=<epoch>`.".into() },
                    ApiEndpoint { path: "/v3/REST/messagesentstatistics".into(),                   method: "GET".into(),  description: "[STATS] Per-message statistics (opens, clicks). Joinable on MessageID.".into() },
                ],
            }),
        },
        // ── AI & Reasoning ───────────────────────────────────────────────
        McpDefinition {
            id: "mcp-memory".into(),
            name: "Memory".into(),
            description: "Persistent knowledge graph for agents — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-memory".into()],
            },
            env_keys: vec![],
            tags: vec!["core".into(), "memory".into(), "knowledge".into()],
            token_url: None,
            token_help: Some("Stores data in memory.jsonl. Set MEMORY_FILE_PATH to customize location.".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-qdrant".into(),
            name: "Qdrant".into(),
            description: "Vector database for semantic search, RAG, and agent memory — official Qdrant server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-server-qdrant".into()],
            },
            env_keys: vec!["QDRANT_URL".into(), "COLLECTION_NAME".into(), "EMBEDDING_MODEL".into()],
            tags: vec!["database".into(), "memory".into(), "rag".into(), "search".into()],
            token_url: Some("https://cloud.qdrant.io".into()),
            token_help: Some("QDRANT_URL: http://localhost:6333 (local) or https://xxx.cloud.qdrant.io (cloud + QDRANT_API_KEY). COLLECTION_NAME: your collection. EMBEDDING_MODEL: e.g. sentence-transformers/all-MiniLM-L6-v2.".into()),
            publisher: "Qdrant".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-sequential-thinking".into(),
            name: "Sequential Thinking".into(),
            description: "Structured step-by-step reasoning for complex problems — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-sequential-thinking".into()],
            },
            env_keys: vec![],
            tags: vec!["core".into(), "reasoning".into(), "thinking".into()],
            token_url: None,
            token_help: None,
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Browser (cloud) ─────────────────────────────────────────────
        McpDefinition {
            id: "mcp-browserbase".into(),
            name: "Browserbase".into(),
            description: "Cloud browser automation — no local Chrome needed. Official Browserbase server.".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@browserbasehq/mcp-server-browserbase".into()],
            },
            env_keys: vec!["BROWSERBASE_API_KEY".into(), "BROWSERBASE_PROJECT_ID".into()],
            tags: vec!["browser".into(), "cloud".into(), "scraping".into(), "testing".into()],
            token_url: Some("https://www.browserbase.com/dashboard".into()),
            token_help: Some("API key + project ID from Browserbase dashboard (paid service)".into()),
            publisher: "Browserbase".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Cloud — Azure ───────────────────────────────────────────────
        McpDefinition {
            id: "mcp-azure".into(),
            name: "Azure".into(),
            description: "Storage, Cosmos DB, Azure CLI, Resource Manager — official Microsoft server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@azure/mcp@latest".into()],
            },
            env_keys: vec![],
            tags: vec!["cloud".into(), "azure".into(), "microsoft".into(), "devops".into()],
            token_url: Some("https://portal.azure.com".into()),
            token_help: Some("Uses Azure CLI auth (az login). No API key needed if already authenticated.".into()),
            publisher: "Microsoft".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Search ──────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-exa".into(),
            name: "Exa".into(),
            description: "AI-native search engine — official Exa server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "exa-mcp-server".into()],
            },
            env_keys: vec!["EXA_API_KEY".into()],
            tags: vec!["search".into(), "web".into(), "ai".into()],
            token_url: Some("https://dashboard.exa.ai/api-keys".into()),
            token_help: Some("API key from Exa dashboard (free tier available)".into()),
            publisher: "Exa".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-perplexity".into(),
            name: "Perplexity".into(),
            description: "AI-powered web search with citations and source verification — official Perplexity server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@perplexity-ai/mcp-server".into()],
            },
            env_keys: vec!["PERPLEXITY_API_KEY".into()],
            tags: vec!["search".into(), "web".into(), "ai".into(), "research".into()],
            token_url: Some("https://www.perplexity.ai/settings/api".into()),
            token_help: Some("API key from Perplexity Settings → API. Supports sonar models for search-grounded responses.".into()),
            publisher: "Perplexity".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-datagouv".into(),
            name: "data.gouv.fr".into(),
            description: "Search and explore French Open Data datasets — official data.gouv.fr server".into(),
            transport: McpTransport::Sse {
                url: "http://localhost:8000/sse".into(),
            },
            env_keys: vec![],
            tags: vec!["search".into(), "opendata".into(), "france".into()],
            token_url: Some("https://github.com/datagouv/datagouv-mcp".into()),
            token_help: Some("No API key needed. Run: docker compose up -d (from cloned repo)".into()),
            publisher: "data.gouv.fr".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Scraping ────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-firecrawl".into(),
            name: "Firecrawl".into(),
            description: "Web scraping, crawling, and content extraction — official Firecrawl server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "firecrawl-mcp".into()],
            },
            env_keys: vec!["FIRECRAWL_API_KEY".into()],
            tags: vec!["scraping".into(), "web".into(), "crawling".into()],
            token_url: Some("https://www.firecrawl.dev/app/api-keys".into()),
            token_help: Some("API key from Firecrawl dashboard".into()),
            publisher: "Firecrawl".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Sandbox ─────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-e2b".into(),
            name: "E2B".into(),
            description: "Execute code in secure cloud sandboxes — official E2B server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@e2b/mcp-server".into()],
            },
            env_keys: vec!["E2B_API_KEY".into()],
            tags: vec!["sandbox".into(), "code-execution".into(), "cloud".into()],
            token_url: Some("https://e2b.dev/dashboard".into()),
            token_help: Some("API key from E2B dashboard".into()),
            publisher: "E2B".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Cloud — Google ────────────────────────────────────────────
        McpDefinition {
            id: "mcp-gcloud".into(),
            name: "Google Cloud (gcloud)".into(),
            description: "GCP resource management via gcloud CLI — official Google server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@anthropic-ai/gcloud-mcp@latest".into()],
            },
            env_keys: vec![],
            tags: vec!["cloud".into(), "gcp".into(), "google".into(), "devops".into()],
            token_url: Some("https://console.cloud.google.com".into()),
            token_help: Some("Uses gcloud CLI auth (gcloud auth login). No API key needed if already authenticated.".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-bigquery".into(),
            name: "BigQuery".into(),
            description: "SQL analytics on large datasets — official Google server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@anthropic-ai/bigquery-mcp@latest".into()],
            },
            env_keys: vec!["GOOGLE_PROJECT_ID".into()],
            tags: vec!["database".into(), "analytics".into(), "gcp".into(), "sql".into()],
            token_url: Some("https://console.cloud.google.com/bigquery".into()),
            token_help: Some("Requires gcloud auth + GOOGLE_PROJECT_ID env var".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-google-analytics".into(),
            name: "Google Analytics 4".into(),
            description: "GA4 reports, realtime data, account summaries — official Google server (read-only)".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["analytics-mcp".into()],
            },
            env_keys: vec!["GOOGLE_APPLICATION_CREDENTIALS".into()],
            tags: vec!["analytics".into(), "google".into(), "seo".into(), "marketing".into()],
            token_url: Some("https://console.cloud.google.com/apis/credentials".into()),
            token_help: Some("Requires gcloud auth application-default login with analytics.readonly scope, or a service account JSON (GOOGLE_APPLICATION_CREDENTIALS). Enable Analytics Admin API + Analytics Data API in GCP console.".into()),
            publisher: "Community".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Database (serverless) ─────────────────────────────────────
        McpDefinition {
            id: "mcp-neon".into(),
            name: "Neon".into(),
            description: "Serverless Postgres — branching, schema management — official Neon server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@neondatabase/mcp-server-neon@latest".into()],
            },
            env_keys: vec!["NEON_API_KEY".into()],
            tags: vec!["database".into(), "postgres".into(), "serverless".into()],
            token_url: Some("https://console.neon.tech/app/settings/api-keys".into()),
            token_help: Some("API key from Neon console".into()),
            publisher: "Neon".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Observability ─────────────────────────────────────────────
        McpDefinition {
            id: "mcp-datadog".into(),
            name: "Datadog".into(),
            description: "Logs, metrics, APM, incidents — official Datadog server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@datadog/mcp-server-datadog".into()],
            },
            env_keys: vec!["DD_API_KEY".into(), "DD_APP_KEY".into()],
            tags: vec!["monitoring".into(), "observability".into(), "logs".into(), "apm".into()],
            token_url: Some("https://app.datadoghq.com/organization-settings/api-keys".into()),
            token_help: Some("API key + Application key from Datadog settings".into()),
            publisher: "Datadog".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-fastly".into(),
            name: "Fastly".into(),
            description: "CDN management, cache purge, VCL, WAF, backends, domains, stats — official Fastly Go server (wraps Fastly CLI). Requires the `fastly` CLI installed locally.".into(),
            transport: McpTransport::Stdio {
                command: "fastly-mcp".into(),
                args: vec![],
            },
            env_keys: vec![],
            // `cli` first so the filter UI groups this plugin under the
            // "CLI wrappers" category — distinct from the pure-MCP
            // bucket (Anthropic-shipped servers, third-party MCP-only).
            tags: vec!["cli".into(), "cdn".into(), "cache".into(), "infrastructure".into(), "edge".into(), "waf".into()],
            token_url: Some("https://manage.fastly.com/account/personal/tokens".into()),
            token_help: Some("Requires the Fastly CLI installed on the host (the MCP shells out to it). Install: `brew install fastly/tap/fastly` (macOS) or the tarball from https://github.com/fastly/cli/releases (Linux/WSL — prefer this over `npm i -g @fastly/cli` which ships a JS wrapper that breaks inside Docker). Then `fastly profile create <name>` and paste your API token. No env var needed — auth is read from CLI profiles.".into()),
            publisher: "Fastly".into(),
            official: true,
            alt_packages: vec!["fastly-mcp-server".into()],
            default_context: Some(r#"# Fastly — Usage Context

> Instructions for AI agents using **Fastly** MCP in this project.

**Server:** Official Fastly MCP (Go binary wrapping Fastly CLI)

## 0. If `fastly CLI not found in PATH` — READ FIRST

The MCP shells out to the `fastly` CLI under the hood. Inside Kronn's Docker
container, three symptoms point to the same root cause:

- `fastly_execute` returns *"fastly CLI not found in PATH"*
- `which fastly` inside the container: not found
- But on the host, `fastly version` works fine

**Root cause**: on Linux/WSL, `npm i -g @fastly/cli` installs a JS wrapper
(`/usr/local/bin/fastly` → `../lib/node_modules/@fastly/cli/fastly.js`).
Kronn mounts `/usr/local/bin` but, until v0.5.0, did NOT mount
`/usr/local/lib`, so the relative symlink resolved to a non-existent
path inside the container. v0.5.0+ adds the `/host-bin/lib` mount which
fixes this transparently — if the problem persists, verify you're on
an up-to-date Kronn image (`./kronn version` / `make start`).

**Alternative fix that works on any Kronn version**: replace the JS
wrapper with the standalone Go binary from
[fastly/cli releases](https://github.com/fastly/cli/releases). The Go
binary is self-contained → no symlink gymnastics → works from any
mount layout.

Verify auth after install:
```bash
fastly profile list          # shows configured profiles
fastly auth list             # shows active tokens
fastly service list --json   # smoke test against the API
```

## 1. Performance rules (result size)

Service listings return 100K+ chars easily. Mitigations, in order of
effectiveness:

- `fastly_result_summary` first — get a digest before reading anything
- `fastly_result_query` with filters (see tool spec)
- `fastly_result_read` with small `limit` (5-10) for pagination

If a result overflows to disk, parse with `jq` or `python3`:
```bash
jq '.[0].text | fromjson | .data[] | {Name, ServiceID, ActiveVersion}' <file>
```

The MCP result format is `[{"type": "text", "text": "<JSON_STRING>"}]`
— the inner JSON has a `data` key containing the actual array.

## 2. Common operations

```
# List services
fastly_execute(command: "service", args: ["list"], flags: [{"name": "json"}])

# Stats — historical traffic for a service (by service-id, minute granularity)
fastly_execute(
  command: "stats",
  args: ["historical"],
  flags: [
    {"name": "service", "value": "<SERVICE_ID>"},
    {"name": "from",    "value": "2026-04-20 14:00:00"},
    {"name": "to",      "value": "2026-04-20 18:00:00"},
    {"name": "by",      "value": "minute"},
    {"name": "json"}
  ]
)

# Real-time stats (rolling window) — useful to correlate live traffic anomalies
fastly_execute(command: "stats", args: ["realtime"], flags: [{"name": "service", "value": "<SERVICE_ID>"}, {"name": "json"}])

# Purge by surrogate key
fastly_execute(command: "purge", args: ["--key", "<KEY>"], flags: [{"name": "service-id", "value": "<ID>"}])

# Domain listing
fastly_execute(command: "domain", args: ["list"], flags: [{"name": "service-id", "value": "<ID>"}, {"name": "version", "value": "active"}])
```

## 3. Traffic-correlation playbook

When the user reports a traffic anomaly in an external analytics tool
(Chartbeat, GA, etc.) and asks "is it the site or a Discover-style
referrer chute?", Fastly stats are the tie-breaker:

1. Find the service whose domain matches — `service list --json`, grep on
   domain name. Sub-domains often have their own service ID.
2. Pull `stats historical` at minute granularity over the suspect window.
3. Compare *hits* (edge requests served) vs *cache_miss* (backend hits):
   - Stable hits, normal cache ratio → the site was healthy; the dip
     is upstream (referrer algorithm, editorial, etc.).
   - Hit drop mirroring the analytics drop, cache ratio stable → traffic
     really fell at the edge — not a measurement artefact.
   - Hit drop + cache miss spike → origin slow / 5xx → site issue.

Surface both the Chartbeat-style number AND the Fastly hit number in
the final report so the user can judge for themselves.

## 4. Rules

- Always use `--json` flag when available to get structured output
- Never purge without explicit user confirmation
- Prefer `fastly_result_summary` to get an overview before reading full results
- If the CLI reports "no profile selected" → the token is missing;
  stop and ask the user to run `fastly profile create` rather than
  guessing a service id
"#.into()),
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-tavily".into(),
            name: "Tavily".into(),
            description: "AI-optimized web search and content extraction — factual results for RAG and research".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "tavily-mcp@latest".into()],
            },
            env_keys: vec!["TAVILY_API_KEY".into()],
            tags: vec!["search".into(), "web".into(), "rag".into(), "research".into()],
            token_url: Some("https://app.tavily.com/home".into()),
            token_help: Some("API key from Tavily dashboard".into()),
            publisher: "Tavily".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-google-colab".into(),
            name: "Google Colab".into(),
            description: "Execute code on Google Colab runtimes with GPU/TPU — official Google server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["--from".into(), "git+https://github.com/googlecolab/colab-mcp".into(), "colab_mcp".into()],
            },
            env_keys: vec![],
            tags: vec!["compute".into(), "gpu".into(), "python".into(), "data-science".into(), "notebook".into()],
            token_url: None,
            token_help: Some("No API key needed — authenticates via Google account in browser".into()),
            publisher: "Google".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Code Quality & Security ──────────────────────────────────
        McpDefinition {
            id: "mcp-sonarqube".into(),
            name: "SonarQube".into(),
            description: "Code quality analysis, security vulnerabilities, quality gates — official SonarSource server".into(),
            transport: McpTransport::Stdio {
                command: "docker".into(),
                args: vec!["run".into(), "--init".into(), "-i".into(), "--rm".into(),
                    "-e".into(), "SONARQUBE_TOKEN".into(),
                    "-e".into(), "SONARQUBE_ORG".into(),
                    "-e".into(), "SONARQUBE_URL".into(),
                    "mcp/sonarqube".into()],
            },
            env_keys: vec!["SONARQUBE_TOKEN".into(), "SONARQUBE_ORG".into()],
            tags: vec!["quality".into(), "security".into(), "ci".into(), "code".into()],
            token_url: Some("https://sonarcloud.io/account/security".into()),
            token_help: Some("Requires Docker. User token from SonarCloud or SonarQube. Set SONARQUBE_ORG for Cloud, or SONARQUBE_URL for self-hosted.".into()),
            publisher: "SonarSource".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Infrastructure as Code ──────────────────────────────────
        McpDefinition {
            id: "mcp-terraform".into(),
            name: "Terraform".into(),
            description: "Registry search, workspace management, runs, providers — official HashiCorp server".into(),
            transport: McpTransport::Stdio {
                command: "docker".into(),
                args: vec!["run".into(), "--init".into(), "-i".into(), "--rm".into(),
                    "-e".into(), "TFE_TOKEN".into(),
                    "hashicorp/terraform-mcp-server".into()],
            },
            env_keys: vec!["TFE_TOKEN".into()],
            tags: vec!["infrastructure".into(), "iac".into(), "devops".into(), "cloud".into()],
            token_url: Some("https://app.terraform.io/app/settings/tokens".into()),
            token_help: Some("Requires Docker. HCP Terraform API token. Optional: TFE_ADDRESS for self-hosted Terraform Enterprise.".into()),
            publisher: "HashiCorp".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Hosting & Deployment ────────────────────────────────────
        McpDefinition {
            id: "mcp-vercel".into(),
            name: "Vercel".into(),
            description: "Projects, deployments, logs, docs — official Vercel server (OAuth)".into(),
            transport: McpTransport::Streamable {
                url: "https://mcp.vercel.com".into(),
            },
            env_keys: vec![],
            tags: vec!["deploy".into(), "hosting".into(), "cloud".into(), "frontend".into()],
            token_url: Some("https://vercel.com/account/tokens".into()),
            token_help: Some("OAuth via browser on first connection — no API key needed. Supports project-specific URLs: https://mcp.vercel.com/<team>/<project>".into()),
            publisher: "Vercel".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        // ── Data Federation ─────────────────────────────────────────
        McpDefinition {
            id: "mcp-mindsdb".into(),
            name: "MindsDB".into(),
            description: "Unified query layer over 200+ data sources — databases, warehouses, and applications".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "minds-mcp".into()],
            },
            env_keys: vec!["MINDS_API_KEY".into()],
            tags: vec!["database".into(), "data".into(), "ai".into(), "federation".into()],
            token_url: Some("https://mdb.ai/account/api-keys".into()),
            token_help: Some("API key from MindsDB Cloud dashboard".into()),
            publisher: "MindsDB".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },

        // ── API-only plugins (no MCP server exists — agent uses curl) ────
        //
        // Chartbeat exposes a REST-only API (live dashboard, historical
        // traffic, engagement, authors…). No official MCP, so we register
        // it as API-only: transport = ApiOnly, api_spec carries the
        // endpoints + auth + docs link. `env_keys` lists BOTH the secret
        // (CHARTBEAT_API_KEY) and non-secret config (CHARTBEAT_HOST),
        // stored together in the encrypted env blob — the UI renders the
        // latter as a plain input via `api_spec.config_keys`.
        McpDefinition {
            id: "api-chartbeat".into(),
            name: "Chartbeat".into(),
            description: "Real-time & historical analytics for editorial sites — Top pages live, engagement, referrers, authors. Live API is synchronous; historical API is async (submit → status → fetch).".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec!["CHARTBEAT_API_KEY".into(), "CHARTBEAT_HOST".into()],
            tags: vec!["analytics".into(), "api".into(), "editorial".into(), "traffic".into()],
            token_url: Some("https://chartbeat.com/publishing/my-account/".into()),
            token_help: Some("API key from Chartbeat account settings (select 'all' scope for full access). HOST = site tracked in Chartbeat (e.g. example.com). One plugin instance per site — duplicate the plugin to track multiple hosts. Sub-domains (de.example.com) are queried by setting `host=de.example.com`, provided the API key has access to them.".into()),
            publisher: "Chartbeat".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# Chartbeat — Usage Context

> Instructions for agents calling the **Chartbeat** API via curl.

Chartbeat has **two API families with DIFFERENT auth mechanisms and
call patterns**. Using the wrong auth on the wrong endpoint is the #1
source of 401/404 confusion — read §1 and §2 before composing anything.

## 1. Live Publishing API — `apikey=` query param, synchronous

All `/live/...` endpoints. Direct GET, JSON response.

```bash
curl -s "https://api.chartbeat.com/live/quickstats/v4?apikey=<KEY>&host=example.com"
```

`host` selects the tracked site. For multi-locale sites pass the
concrete sub-domain (`host=de.example.com` for a German edition). The
plugin config stores a default `host` but agents SHOULD override per
question when the user mentions another edition.

## 2. Historical API — `X-CB-AK` HEADER, synchronous

**Critical**: `/historical/...` endpoints do NOT accept `apikey` as a
query parameter. They require the header `X-CB-AK: <KEY>`. Passing the
key in the URL returns 401/404 that LOOK like access errors — it's
just the wrong auth channel. The Kronn plugin auth setting is
`apikey=` (Live API), so for Historical calls Kronn surfaces the
same key as the `X-CB-AK` header in the agent's context — copy it
verbatim.

```bash
# Time series — returns JSON directly (synchronous).
curl -s "https://api.chartbeat.com/historical/traffic/series/?host=example.com&start=2026-04-19&end=2026-04-20&frequency=hour" \
     -H "X-CB-AK: <KEY>"
```

Available historical endpoints (all `GET`, all header auth):

- `/historical/traffic/{series,stats}/`
- `/historical/engagement/{series,stats}/`
- `/historical/social/{series,stats}/`

There is **no** `/query/v2/...` async family on this Chartbeat
account — those paths return 404. If you have an older code sample
referencing them, ignore it and use the `/historical/...` paths above
or the `/live/...` family for real-time data.

## 3. Granularity for analysing dips

Live Publishing exposes traffic at **5-minute granularity** through
`/live/recent/v3` and friends. When the user says *"I see a dip
between 16h and 17h20"*, don't stop at hourly historical data — pull
minute-level live series, otherwise you miss the shape of the dip
(gradual vs brutal) and the rebound. 0-values at isolated timestamps
are almost always **API data gaps**, not real zero-traffic moments.

## 4. Host vs sub-domain — common pitfall

- `host=example.com` only sees the root-domain traffic. It does NOT
  aggregate sub-domains automatically.
- Regional editions use the full host: `host=de.example.com`. Check
  the site's client-side Chartbeat config to learn the exact host
  string (often `${locale}.${base}` built from a `Locale` helper).
- A 404 on `/historical/...` is almost always a wrong-auth issue
  (you sent `apikey=` instead of `X-CB-AK:`), NOT an API-key scope
  limit. Verify by switching to header auth.

## 5. Params that work on both families

- `host=...` — tracked site (required)
- `limit=N` — cap rows
- `sections=news,sport` — filter
- `path=/article/xxx` — single URL filter
- `start=YYYY-MM-DD` / `end=YYYY-MM-DD` — date range
- `frequency=minute|hour|day` — time-series granularity

Official docs: https://docs.chartbeat.com/cbp/api/historical-api/getting-started-with-our-historical-api
"#.into()),
            api_spec: Some(ApiSpec {
                base_url: "https://api.chartbeat.com".into(),
                auth: ApiAuthKind::ApiKeyQuery {
                    param_name: "apikey".into(),
                    env_key: "CHARTBEAT_API_KEY".into(),
                },
                docs_url: Some("https://help.chartbeat.com/hc/en-us/articles/360045337214-Guide-to-Chartbeat-APIs".into()),
                config_keys: vec![
                    ApiConfigKey {
                        env_key: "CHARTBEAT_HOST".into(),
                        label: "Host (default)".into(),
                        placeholder: "domain.tld".into(),
                        description: "Default site tracked in Chartbeat (e.g. example.com). Passed as `host=<value>` on each request. Agents can override per-call when the user asks about a regional edition (e.g. `host=de.example.com`). Duplicate the plugin to track unrelated sites.".into(),
                    },
                ],
                endpoints: vec![
                    // List sourced from the official Chartbeat API explorer:
                    // https://chartbeat.com/docs/api/explore/. Keep in sync —
                    // /live/dashapi/v4, /live/toppages/v4, /live/top_geo/v3,
                    // /live/social/v3, /live/top_devices/v3, /live/video/*,
                    // /query/v2/* and /historical/topreferrers,authors are
                    // NOT real endpoints (or have been deprecated). They
                    // returned 404 in production and were removed.
                    //
                    // ── Live Publishing API (synchronous GET, apikey= query param) ──
                    ApiEndpoint { path: "/live/quickstats/v4/".into(),       method: "GET".into(), description: "[LIVE · apikey= · sync] Aggregate stats (visits, engaged time, referrer types) — the closest thing to a dashboard snapshot.".into() },
                    ApiEndpoint { path: "/live/toppages/v3/".into(),         method: "GET".into(), description: "[LIVE · apikey= · sync] Top pages right now, ranked by concurrents.".into() },
                    ApiEndpoint { path: "/live/recent/v3/".into(),           method: "GET".into(), description: "[LIVE · apikey= · sync] Recent visitor activity (5-min granularity — ideal for short-window dip analysis).".into() },
                    ApiEndpoint { path: "/live/referrers/v3/".into(),        method: "GET".into(), description: "[LIVE · apikey= · sync] Top referrers bringing traffic now.".into() },
                    ApiEndpoint { path: "/live/summary/v3/".into(),          method: "GET".into(), description: "[LIVE · apikey= · sync] Concise traffic summary for a section. Pass `section=<path>` to scope.".into() },
                    ApiEndpoint { path: "/live/top_geo/v1/".into(),          method: "GET".into(), description: "[LIVE · apikey= · sync] Top countries / regions.".into() },
                    // ── Historical API (synchronous GET, X-CB-AK header — NOT apikey= param) ──
                    ApiEndpoint { path: "/historical/traffic/series/".into(),method: "GET".into(), description: "[HIST · X-CB-AK · sync] Traffic time series. Query: `host=`, `start=YYYY-MM-DD`, `end=YYYY-MM-DD`, `frequency=hour|minute|day`. Header auth required — `apikey=` query param will 401/404.".into() },
                    ApiEndpoint { path: "/historical/traffic/stats/".into(), method: "GET".into(), description: "[HIST · X-CB-AK · sync] Aggregate traffic stats over a date range.".into() },
                    ApiEndpoint { path: "/historical/engagement/series/".into(), method: "GET".into(), description: "[HIST · X-CB-AK · sync] Engagement time series (engaged time, scroll depth).".into() },
                    ApiEndpoint { path: "/historical/engagement/stats/".into(), method: "GET".into(), description: "[HIST · X-CB-AK · sync] Aggregate engagement stats over a date range.".into() },
                    ApiEndpoint { path: "/historical/social/series/".into(), method: "GET".into(), description: "[HIST · X-CB-AK · sync] Social-referrer time series (Facebook, Twitter/X, etc).".into() },
                    ApiEndpoint { path: "/historical/social/stats/".into(),  method: "GET".into(), description: "[HIST · X-CB-AK · sync] Aggregate social-referrer stats over a date range.".into() },
                ],
            }),
        },

        // ─────────────────────────────────────────────────────────────────
        // Adobe Analytics Reporting 2.0 — second API plugin. Demonstrates
        // the OAuth2 client-credentials auth path (Kronn mints + caches
        // the bearer token automatically before injection).
        // ─────────────────────────────────────────────────────────────────
        McpDefinition {
            id: "api-adobe-analytics".into(),
            name: "Adobe Analytics".into(),
            description: "Adobe Analytics Reporting API 2.0 — page views, visitors, segments, realtime. OAuth2 S2S with Kronn-managed bearer token refresh.".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec![
                "ADOBE_CLIENT_ID".into(),
                "ADOBE_CLIENT_SECRET".into(),
                "ADOBE_ORG_ID".into(),
                "ADOBE_COMPANY_ID".into(),
                "ADOBE_RSID".into(),
            ],
            tags: vec!["analytics".into(), "api".into(), "editorial".into(), "traffic".into(), "oauth2".into()],
            token_url: Some("https://developer.adobe.com/console/projects".into()),
            token_help: Some("1) Create a project in Adobe Developer Console → add Analytics API → configure OAuth2 Server-to-Server (replaces the deprecated JWT flow). 2) Copy Client ID, Client Secret, and Organization ID. 3) Find your Company ID in Analytics (URL after /company/ on analytics.adobe.com). 4) Pick a Report Suite ID (RSID) for this instance. Kronn handles the Bearer token refresh automatically — no manual renewal.".into()),
            publisher: "Adobe".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# Adobe Analytics Reporting 2.0 — Usage Context

> Instructions for agents calling the **Adobe Analytics Reporting API 2.0**
> via curl. Kronn mints + caches the bearer token automatically.

## 1. How Kronn handles auth (so you don't have to)

Kronn exchanges `ADOBE_CLIENT_ID` + `ADOBE_CLIENT_SECRET` against
Adobe IMS (`https://ims-na1.adobelogin.com/ims/token/v3`) on every
discussion start (and on refresh when the 24h token is close to
expiring). The fresh bearer is injected into this context block as
`Authorization: Bearer <token>`. You just copy-paste it.

If the context says **"TOKEN UNAVAILABLE"**, stop and tell the user
— their `ADOBE_CLIENT_ID` / `SECRET` / `ORG_ID` are wrong or the
Adobe project isn't authorized for the Analytics API.

Three headers on EVERY Adobe call (Kronn surfaces them above; copy them
verbatim):
- `Authorization: Bearer <access_token>` (Kronn-managed)
- `x-api-key: <client_id>` (Adobe requires both)
- `x-proxy-global-company-id: <company_id>` (the analytics tenant)

Also:
- `Content-Type: application/json` on POST bodies

## 2. Base URL

```
https://analytics.adobe.io/api/<ADOBE_COMPANY_ID>/
```

Kronn interpolates `<ADOBE_COMPANY_ID>` automatically in the Base URL
field above. Endpoints are relative to that: `/reports`, `/dimensions`,
`/metrics`, etc.

## 3. The main endpoint — `POST /reports`

This is where 90 % of analyses go. It accepts a JSON body describing:
- `rsid`: the report suite (set `ADOBE_RSID` in the plugin config)
- `globalFilters`: at minimum a `dateRange` (ISO 8601 interval)
- `metrics`: what to count (`metrics/pageviews`, `metrics/visits`, etc.)
- `dimension`: ONE dimension to break down by (page, section, device…)
- `settings`: `{ "limit": 50 }` typical

### Pageviews by page over yesterday

```bash
curl -s -X POST "https://analytics.adobe.io/api/<COMPANY_ID>/reports" \
  -H "Authorization: Bearer <TOKEN>" \
  -H "x-api-key: <CLIENT_ID>" \
  -H "x-proxy-global-company-id: <COMPANY_ID>" \
  -H "Content-Type: application/json" \
  -d '{
    "rsid": "<RSID>",
    "globalFilters": [
      { "type": "dateRange", "dateRange": "2026-04-20T00:00:00/2026-04-20T23:59:59" }
    ],
    "metricContainer": {
      "metrics": [ { "id": "metrics/pageviews", "columnId": "0" } ]
    },
    "dimension": "variables/page",
    "settings": { "limit": 50, "page": 0 }
  }'
```

### Trended (time series) — minute granularity

Replace `dimension` with a time dimension:
- `variables/daterangeminute`  — per-minute
- `variables/daterangehour`    — per-hour
- `variables/daterangeday`     — per-day

```bash
# Pageviews per minute on a tight window (dip analysis)
-d '{
  "rsid": "<RSID>",
  "globalFilters": [
    { "type": "dateRange", "dateRange": "2026-04-20T14:00:00/2026-04-20T17:00:00" }
  ],
  "metricContainer": {
    "metrics": [ { "id": "metrics/pageviews", "columnId": "0" } ]
  },
  "dimension": "variables/daterangeminute",
  "settings": { "limit": 180 }
}'
```

### Segmenting

Apply a segment UUID (see `/segments` to list) as a `globalFilter`:
```json
{ "type": "segment", "segmentId": "s300000000_63abcdef1234" }
```

## 4. Other useful endpoints

- `GET /dimensions?rsid=<RSID>` — available dimensions for the RSID
- `GET /metrics?rsid=<RSID>` — available metrics
- `GET /segments?rsid=<RSID>` — saved segments
- `POST /reports/realtime` — realtime reports (last 15-30 min, different body shape)
- `GET /calculatedmetrics?rsids=<RSID>` — user-defined calculated metrics
- `GET /users/me` — smoke test: should return your Adobe user object

Full schema: [Adobe Analytics 2.0 API docs](https://developer.adobe.com/analytics-apis/docs/2.0/)

## 5. Common pitfalls

- **`401` on a freshly-started Kronn**: the token was already requested
  but the Adobe project isn't linked to the Analytics product profile.
  Fix it in Adobe Admin Console → Products → Analytics → Permissions.
- **`403 "Forbidden"`**: the user doesn't have access to the RSID you
  requested. Try a different RSID or escalate to Adobe admin.
- **Rate limits**: 120 requests/minute per client_id per tenant on
  Reporting 2.0. Don't fan out parallel queries for minute-by-minute —
  one trended query with 60-180 rows is cheaper.
- **Huge responses**: a `/reports` with `limit=200` on a wide
  dimension can return 1 MB+. Prefer `limit=25-50` + pagination via
  `settings.page` when digging deep.

## 6. What NOT to do

- Do NOT call `/reports/realtime` for historical data — it's capped at
  the last ~30 min.
- Do NOT fan out one call per day to build a week's trend — one call
  with `variables/daterangeday` + 7-day range returns all 7 points.
- Do NOT leak the bearer token back to the user. It's valid for 24h
  but still a live credential.
"#.into()),
            api_spec: Some(ApiSpec {
                // Company ID is path-interpolated — Kronn substitutes
                // `{ADOBE_COMPANY_ID}` at prompt-build time so the agent
                // sees the actual URL it must call.
                base_url: "https://analytics.adobe.io/api/{ADOBE_COMPANY_ID}".into(),
                auth: ApiAuthKind::OAuth2ClientCredentials {
                    token_url: "https://ims-na1.adobelogin.com/ims/token/v3".into(),
                    client_id_env: "ADOBE_CLIENT_ID".into(),
                    client_secret_env: "ADOBE_CLIENT_SECRET".into(),
                    // Adobe IMS uses COMMA separators (not spaces like RFC).
                    scope: "openid,AdobeID,additional_info.projectedProductContext,session,read_organizations,additional_info.roles".into(),
                    extra_headers: vec![
                        OAuth2ExtraHeader {
                            name: "x-api-key".into(),
                            value_template: "{ADOBE_CLIENT_ID}".into(),
                        },
                        OAuth2ExtraHeader {
                            name: "x-proxy-global-company-id".into(),
                            value_template: "{ADOBE_COMPANY_ID}".into(),
                        },
                    ],
                },
                docs_url: Some("https://developer.adobe.com/analytics-apis/docs/2.0/".into()),
                config_keys: vec![
                    ApiConfigKey {
                        env_key: "ADOBE_COMPANY_ID".into(),
                        label: "Company ID".into(),
                        placeholder: "mycompany".into(),
                        description: "Adobe Analytics tenant identifier (the segment after `/company/` in the Analytics dashboard URL). Interpolated into the Base URL + the x-proxy-global-company-id header.".into(),
                    },
                    ApiConfigKey {
                        env_key: "ADOBE_ORG_ID".into(),
                        label: "Organization ID (IMS)".into(),
                        placeholder: "ABCDEF1234567890@AdobeOrg".into(),
                        description: "Adobe IMS organization identifier — found in the Developer Console project overview. Used during OAuth2 exchange.".into(),
                    },
                    ApiConfigKey {
                        env_key: "ADOBE_RSID".into(),
                        label: "Report Suite ID (default)".into(),
                        placeholder: "examplecompanyprod".into(),
                        description: "Default Analytics report suite queried by `/reports` calls. Agents can override per-call when the user asks about another property.".into(),
                    },
                ],
                endpoints: vec![
                    ApiEndpoint { path: "/reports".into(),                method: "POST".into(), description: "[REPORT · OAuth2 · sync] Main reporting endpoint — JSON body declares rsid, metrics, dimension, dateRange, segments. 90% of analyses go here.".into() },
                    ApiEndpoint { path: "/reports/realtime".into(),       method: "POST".into(), description: "[REALTIME · OAuth2 · sync] Real-time reports (last 15-30 min only). Different body shape from /reports.".into() },
                    ApiEndpoint { path: "/dimensions".into(),             method: "GET".into(),  description: "[META · OAuth2 · sync] ?rsid=<RSID> → list of available dimensions for the report suite".into() },
                    ApiEndpoint { path: "/metrics".into(),                method: "GET".into(),  description: "[META · OAuth2 · sync] ?rsid=<RSID> → list of available metrics".into() },
                    ApiEndpoint { path: "/segments".into(),               method: "GET".into(),  description: "[META · OAuth2 · sync] ?rsid=<RSID> → saved segments (copy segmentId into /reports' globalFilters).".into() },
                    ApiEndpoint { path: "/calculatedmetrics".into(),      method: "GET".into(),  description: "[META · OAuth2 · sync] ?rsids=<RSID> → user-defined calculated metrics".into() },
                    ApiEndpoint { path: "/users/me".into(),               method: "GET".into(),  description: "[SMOKE · OAuth2 · sync] Current user profile — use as a health check".into() },
                ],
            }),
        },

        // ─────────────────────────────────────────────────────────────────
        // Google Programmable Search (Custom Search JSON API) — third API
        // plugin. No MCP equivalent. Single-endpoint search over a curated
        // set of sites (or the whole web) defined by a Programmable Search
        // Engine ID (CX). Useful for SEO monitoring, competitive SERP
        // research, or site-scoped search from workflows.
        // ─────────────────────────────────────────────────────────────────
        McpDefinition {
            id: "api-google-search".into(),
            name: "Google Search".into(),
            description: "Google Programmable Search (Custom Search JSON API) — site-scoped or whole-web SERP. 100 queries/day free (paid beyond). Single endpoint; rich filters (dateRestrict, siteSearch, lr, gl, searchType=image).".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec![
                "GOOGLE_SEARCH_API_KEY".into(),
                "GOOGLE_SEARCH_CX".into(),
            ],
            tags: vec!["search".into(), "api".into(), "seo".into(), "editorial".into()],
            token_url: Some("https://console.cloud.google.com/apis/credentials".into()),
            token_help: Some("1) Create an API key at Google Cloud Console (Credentials page) and enable the 'Custom Search API'. 2) Create a Programmable Search Engine at https://programmablesearchengine.google.com/, configure which sites it searches (or 'Search the entire web'), copy the Search Engine ID (cx). 3) Free tier: 100 queries/day. Beyond that, $5 per 1000 queries, capped at 10 000/day. Quota billed per project.".into()),
            publisher: "Google".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# Google Programmable Search — Usage Context

> Instructions for agents calling the Google Custom Search JSON API via curl.

## 1. One endpoint, many modes

```
GET https://www.googleapis.com/customsearch/v1
  ?key=<API_KEY>
  &cx=<SEARCH_ENGINE_ID>
  &q=<query>
  [&num=10 &start=1 &lr=lang_fr &gl=fr &safe=off]
```

The API has ONE path. Everything else is query parameters. Kronn
pre-fills `key=` and `cx=` above — you just add `q=...` and whatever
filters the question requires.

## 2. Quota — read carefully before scripting

- **100 queries/day free.** After that the project must have billing
  enabled; $5 per 1000 queries, max 10 000/day.
- Quota is **per project**, not per user, not per cx. If you batch a
  workflow over 100 pages, you burn the free tier in one run.
- Every call counts, including failed ones (except HTTP 429).
- There is NO "preview" mode — no way to check quota remaining before
  a call. Budget pessimistically; prefer `num=10` (max) over multiple
  paginated calls.

When the user asks for "top 100 SERPs", the honest answer is: 10 calls
× `num=10` with `start=1, 11, 21, …, 91` = 10 units of quota, one day
of the free tier. Confirm before going deep.

## 3. Key parameters

| Param | What | Example |
|-------|------|---------|
| `q` | Query (required) | `q=euronews+mid-article+video` |
| `num` | Results per call (1-10) | `num=10` |
| `start` | 1-based offset for pagination | `start=11` (page 2) |
| `lr` | Language restrict | `lr=lang_fr` |
| `gl` | Geo (country code boost, 2 letters) | `gl=fr` |
| `dateRestrict` | Relative time | `dateRestrict=d7` (7 days) · `w2` · `m6` · `y1` |
| `siteSearch` | Scope to one domain | `siteSearch=example.com` |
| `siteSearchFilter` | `i` include / `e` exclude (needs siteSearch) | `siteSearchFilter=e` |
| `searchType=image` | Image search mode | add `searchType=image` |
| `safe` | `active` / `off` | `safe=off` |
| `sort` | `date` or similar | `sort=date` |

The `exactTerms`, `excludeTerms`, `orTerms` params also exist when the
natural `q` becomes too clunky.

## 4. Response shape

```json
{
  "kind": "customsearch#search",
  "searchInformation": { "totalResults": "12300", "searchTime": 0.34 },
  "items": [
    {
      "title": "…",
      "link": "https://example.com/…",
      "displayLink": "example.com",
      "snippet": "…",
      "pagemap": { "metatags": [ { "og:title": "…" } ] }
    },
    …
  ]
}
```

`pagemap` carries OpenGraph / structured-data enrichment when the page
exposes it — great for SEO competitive analysis without fetching each
URL yourself.

## 5. Common use-cases (pre-composed curls)

### Where does our site rank for query X?

```bash
curl -s "https://www.googleapis.com/customsearch/v1\
?key=<KEY>&cx=<CX>&q=euronews+vertical+video&num=10&gl=fr" \
| jq '.items[] | {rank: .cacheId, link, title}'
```

### Recent news on a topic (7-day window)

```bash
curl -s "https://www.googleapis.com/customsearch/v1\
?key=<KEY>&cx=<CX>&q=climate+policy&num=10&dateRestrict=d7&sort=date" \
| jq '.items[] | {link, title, published: .pagemap.metatags[0]["article:published_time"]}'
```

### Site-scoped search (only your domain)

```bash
curl -s "https://www.googleapis.com/customsearch/v1\
?key=<KEY>&cx=<CX>&q=EV+batteries&siteSearch=example.com&num=10"
```

## 6. `cx` variants — when the agent hits a wall

- A CX configured with "Search the entire web" works like the real
  Google SERP but with the 10-result limit.
- A CX scoped to specific sites returns 0 results if the query doesn't
  match them — an empty `items` is correct, not a bug.
- The `/siterestrict/customsearch/v1` path exists for CX engines whose
  config locks them to specific sites; performance difference is nil.
  Only use it if the Programmable Search Engine UI tells you to.

## 7. Pitfalls

- **HTTP 403 "dailyLimitExceeded"**: you hit the 100/day free cap. Tell
  the user, stop. Don't retry.
- **Empty `items` array**: NOT an error — the query just had no
  matches. Surface that plainly.
- **429**: rate-limiting burst. Back off 1-2s and retry once, no more.
- **Never leak the API key** back to the user. It's billable.

Official docs: https://developers.google.com/custom-search/v1/reference/rest
"#.into()),
            api_spec: Some(ApiSpec {
                base_url: "https://www.googleapis.com/customsearch/v1".into(),
                auth: ApiAuthKind::ApiKeyQuery {
                    param_name: "key".into(),
                    env_key: "GOOGLE_SEARCH_API_KEY".into(),
                },
                docs_url: Some("https://developers.google.com/custom-search/v1/reference/rest".into()),
                config_keys: vec![
                    ApiConfigKey {
                        env_key: "GOOGLE_SEARCH_CX".into(),
                        label: "Search Engine ID (cx)".into(),
                        placeholder: "a0b1c2d3e4f5g6h7i".into(),
                        description: "Programmable Search Engine ID (cx) — create or manage at https://programmablesearchengine.google.com/. Non-secret. Passed as `cx=<value>` on every call. Duplicate the plugin to use several engines (e.g. one scoped to your site, one whole-web).".into(),
                    },
                ],
                endpoints: vec![
                    ApiEndpoint {
                        path: "".into(),
                        method: "GET".into(),
                        description: "[SEARCH · apikey= · sync] The one and only endpoint. Pass q=<query>, num=1-10, start=<offset>, plus any filter (dateRestrict, siteSearch, lr, gl, searchType=image). See default_context for the complete param matrix.".into(),
                    },
                ],
            }),
        },

        // ─────────────────────────────────────────────────────────────────
        // SpeedCurve — synthetic & real-user web performance monitoring.
        // API-only plugin, HTTP Basic with the API key as username and an
        // empty password (the `BasicApiKey` auth variant). Two data planes:
        //   - Synthetic (WPT-style): /v1/sites, /v1/tests, /v1/deploys
        //   - LUX (Real User Monitoring): /v1/lux/...
        // Both share the same API key.
        // ─────────────────────────────────────────────────────────────────
        McpDefinition {
            id: "api-speedcurve".into(),
            name: "SpeedCurve".into(),
            description: "Synthetic + RUM web performance — Core Web Vitals, deploys, LUX. Tracks performance regressions across releases.".into(),
            transport: McpTransport::ApiOnly,
            env_keys: vec!["SPEEDCURVE_API_KEY".into()],
            tags: vec!["performance".into(), "api".into(), "rum".into(), "synthetic".into(), "core-web-vitals".into()],
            token_url: Some("https://app.speedcurve.com/account/api/".into()),
            token_help: Some("Read-only API key from SpeedCurve account settings (Account → API). The same key works for synthetic + LUX endpoints. Auth is HTTP Basic with the key as username and an empty password — Kronn handles the encoding automatically.".into()),
            publisher: "SpeedCurve".into(),
            official: true,
            alt_packages: vec![],
            default_context: Some(r#"# SpeedCurve — Usage Context

> Instructions for agents calling the **SpeedCurve** API via curl.

## 1. Auth — HTTP Basic with API key as user, empty password

```bash
curl -u "$SPEEDCURVE_API_KEY:" "https://api.speedcurve.com/v1/sites"
```

Note the trailing colon in `$KEY:` — the password half is intentionally
empty. Kronn already injects `Authorization: Basic <base64>` for you,
this is just for reference if the agent debugs raw auth.

## 2. Two data planes

- **Synthetic** (WebPageTest-style scheduled tests): `/v1/sites`,
  `/v1/tests`, `/v1/deploys`. Tracks Core Web Vitals + custom metrics
  on a schedule (every 30min, hourly, daily…) from chosen regions.
- **LUX** (Real User Monitoring — JS beacon on real user pages):
  `/v1/lux/...`. Aggregated CWV from real visitors, segmented by
  device/country/page-label/etc.

A single API key works for both.

## 3. Common workflows

- **Did this deploy break perf?** → POST `/v1/deploys` to mark a
  release timeline → next test run carries the deploy_id → compare
  `/v1/tests?since=<deploy_id>` against pre-deploy baseline.
- **CWV regression on prod?** → GET `/v1/lux/sites/<id>/metrics`
  with `metric=lcp&granularity=hour&start=<ts>&end=<ts>`.
- **Top slow URLs?** → GET `/v1/lux/sites/<id>/url_metrics`
  with `metric=lcp&order=desc&limit=20`.

## 4. Pagination

`limit` + `offset` (most endpoints, default limit 100, max 1000).
Some endpoints use `since` / `until` ISO-8601 timestamps for windowing.

## 5. Rate limit

300 req/min per key. Kronn's BatchApiCall default `concurrent_limit=5`
is well within bounds for typical fan-outs.
"#.into()),
            api_spec: Some(ApiSpec {
                base_url: "https://api.speedcurve.com".into(),
                auth: ApiAuthKind::BasicApiKey {
                    env_key: "SPEEDCURVE_API_KEY".into(),
                },
                docs_url: Some("https://support.speedcurve.com/docs/api".into()),
                config_keys: vec![],
                endpoints: vec![
                    // ── Account / sanity ──
                    ApiEndpoint { path: "/v1/account".into(),                                method: "GET".into(),  description: "[ACCOUNT · sanity] Account info + plan tier — quickest way to verify the key works (200 = OK, 401 = bad key).".into() },
                    ApiEndpoint { path: "/v1/teams".into(),                                  method: "GET".into(),  description: "[TEAMS] List teams in the account — useful to scope subsequent calls.".into() },
                    // ── Synthetic — Sites ──
                    ApiEndpoint { path: "/v1/sites".into(),                                  method: "GET".into(),  description: "[SYNTHETIC · sites] List monitored sites with their site_id (used as path param everywhere else).".into() },
                    ApiEndpoint { path: "/v1/sites/{site_id}".into(),                        method: "GET".into(),  description: "[SYNTHETIC · sites] One site's config (URL, regions, browsers, schedule).".into() },
                    // ── Synthetic — Tests ──
                    ApiEndpoint { path: "/v1/tests".into(),                                  method: "GET".into(),  description: "[SYNTHETIC · tests] List test runs across all sites. Query: `site=<id>`, `since=<unix_ts>`, `until=<unix_ts>`, `limit=N`, `browser=chrome|firefox`, `region=<id>`. One row per (URL × browser × region × timestamp).".into() },
                    ApiEndpoint { path: "/v1/tests/{test_id}".into(),                        method: "GET".into(),  description: "[SYNTHETIC · tests] Single test run with full waterfall, every CWV metric, screenshots, traces.".into() },
                    // ── Synthetic — Deploys (release timeline markers) ──
                    ApiEndpoint { path: "/v1/deploys".into(),                                method: "GET".into(),  description: "[SYNTHETIC · deploys · list] List release markers. Query: `site=<id>`, `since=<ts>`, `limit=N`. Use to correlate perf changes with releases.".into() },
                    ApiEndpoint { path: "/v1/deploys".into(),                                method: "POST".into(), description: "[SYNTHETIC · deploys · create] Mark a release on the timeline. Body: `{\"site_id\":<id>,\"note\":\"v0.6.0\",\"details\":\"<changelog excerpt>\"}`. Returns the new deploy_id.".into() },
                    ApiEndpoint { path: "/v1/deploys/{deploy_id}".into(),                    method: "GET".into(),  description: "[SYNTHETIC · deploys] One deploy + its associated tests (auto-fired post-deploy).".into() },
                    // ── LUX (RUM) ──
                    ApiEndpoint { path: "/v1/lux/sites".into(),                              method: "GET".into(),  description: "[LUX · sites] List LUX-enabled sites (subset of synthetic sites that have the JS beacon installed).".into() },
                    ApiEndpoint { path: "/v1/lux/sites/{site_id}/metrics".into(),            method: "GET".into(),  description: "[LUX · metrics] Aggregated CWV time series. Query: `metric=lcp|cls|inp|fcp|ttfb`, `granularity=hour|day`, `start=<ISO>`, `end=<ISO>`, `dimension=device|country|page_label`.".into() },
                    ApiEndpoint { path: "/v1/lux/sites/{site_id}/url_metrics".into(),        method: "GET".into(),  description: "[LUX · per-URL] Top URLs ranked by a metric. Query: `metric=lcp|cls|inp`, `order=asc|desc`, `limit=N`, `start=<ISO>`, `end=<ISO>`. Use to find slowest pages.".into() },
                    ApiEndpoint { path: "/v1/lux/sites/{site_id}/page_groups".into(),        method: "GET".into(),  description: "[LUX · page groups] Performance segmented by page label (homepage, article, search…). Same query params as url_metrics.".into() },
                ],
            }),
        },
    ]
}

/// Search the registry by name or tag
pub fn search(query: &str) -> Vec<McpDefinition> {
    let q = query.to_lowercase();
    builtin_registry()
        .into_iter()
        .filter(|m| {
            m.name.to_lowercase().contains(&q)
                || m.description.to_lowercase().contains(&q)
                || m.tags.iter().any(|t| t.contains(&q))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Packages whose upstream switched runtime (e.g. to bun) and MUST stay pinned to a Node-compatible version.
    const PINNED_PACKAGES: &[(&str, &str)] = &[];

    #[test]
    fn registry_ids_are_unique() {
        let reg = builtin_registry();
        let mut seen = HashSet::new();
        for def in &reg {
            assert!(
                seen.insert(&def.id),
                "Duplicate MCP registry id: {}",
                def.id
            );
        }
    }

    #[test]
    fn pinned_packages_are_respected() {
        let reg = builtin_registry();
        for (pkg, expected_version) in PINNED_PACKAGES {
            let expected_arg = format!("{pkg}@{expected_version}");
            let found = reg.iter().any(|def| {
                if let McpTransport::Stdio { args, .. } = &def.transport {
                    args.iter().any(|a| a == &expected_arg)
                } else {
                    false
                }
            });
            assert!(
                found,
                "Pinned package {pkg} must use version @{expected_version} in the registry. \
                 See PINNED_PACKAGES comment for why (upstream broke Node.js compat)."
            );
        }
    }

    #[test]
    fn search_finds_fastly() {
        let results = search("fastly");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "mcp-fastly");
    }

    #[test]
    fn all_stdio_entries_have_nonempty_command() {
        for def in builtin_registry() {
            if let McpTransport::Stdio { command, .. } = &def.transport {
                assert!(!command.is_empty(), "MCP {} has empty command", def.id);
            }
        }
    }

    /// `mcp-resend` is a hybrid plugin (Stdio MCP transport + REST API
    /// spec) — same convention as `mcp-github` and `mcp-atlassian`. One
    /// entry, one credential (`RESEND_API_KEY`), two surfaces: MCP for
    /// agent-rich Quick Prompts, ApiCall for zero-token workflow steps.
    /// Guards against accidental removal of the `api_spec` (which would
    /// silently regress the plugin back to MCP-only — exactly the
    /// pre-0.8.3 state).
    #[test]
    fn resend_is_a_hybrid_mcp_plus_api_plugin() {
        let reg = builtin_registry();
        let def = reg
            .iter()
            .find(|d| d.id == "mcp-resend")
            .expect("mcp-resend missing from registry");

        // ── MCP transport (for Quick Prompts) ──
        match &def.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert!(args.iter().any(|a| a == "resend-mcp"),
                    "mcp-resend args must reference the resend-mcp package, got {args:?}");
            }
            other => panic!("mcp-resend must keep Stdio transport (MCP capability), got {other:?}"),
        }

        // ── API capability (for zero-token ApiCall steps) ──
        let spec = def.api_spec.as_ref()
            .expect("mcp-resend MUST declare api_spec — the hybrid is the whole point of the 0.8.3 work, do NOT regress to MCP-only");
        assert_eq!(spec.base_url, "https://api.resend.com");
        match &spec.auth {
            ApiAuthKind::Bearer { env_key } => assert_eq!(env_key, "RESEND_API_KEY"),
            other => panic!("Resend auth must be Bearer, got {other:?}"),
        }
        assert!(spec.docs_url.is_some());
        assert!(!spec.endpoints.is_empty());
        // Headline endpoints — if a future rename drops them, CSM/lifecycle
        // flows break. Lock them by name.
        assert!(spec.endpoints.iter().any(|e| e.path == "/emails" && e.method == "POST"),
            "Resend api_spec must keep POST /emails (single send)");
        assert!(spec.endpoints.iter().any(|e| e.path == "/emails/batch" && e.method == "POST"),
            "Resend api_spec must keep POST /emails/batch (fan-out send)");

        // ── Shared shape (credential + metadata) ──
        assert_eq!(def.env_keys, vec!["RESEND_API_KEY"],
            "one credential drives both surfaces — never split into two env keys");
        assert!(def.tags.contains(&"email".into()));
        assert!(def.tags.contains(&"api".into()),
            "the `api` tag flags this plugin as ApiCall-callable, even when the entry is `mcp-` prefixed");
        assert!(def.tags.contains(&"csm".into()),
            "Resend should carry the `csm` tag — CSM/lifecycle is the use case the API spec exists for");
        assert!(def.default_context.is_some(),
            "mcp-resend must ship a default_context covering the API pitfalls (verified domain, idempotency, batch constraints)");
        assert_eq!(def.publisher, "Resend");
        assert!(def.official);
        assert!(def.token_url.is_some());
        assert!(def.token_help.is_some());
    }

    /// `api-mailjet` — Mailjet Send API v3.1. EU/RGPD counterpart of Resend.
    /// Auth = HTTP Basic with `api_key:api_secret`; both env keys required.
    /// Guards `/v3.1/send` (modern body shape) and sender introspection.
    #[test]
    fn api_mailjet_is_registered_and_shaped_correctly() {
        let reg = builtin_registry();
        let def = reg
            .iter()
            .find(|d| d.id == "api-mailjet")
            .expect("api-mailjet missing from registry");

        assert!(matches!(def.transport, McpTransport::ApiOnly));
        assert!(def.env_keys.contains(&"MAILJET_API_KEY".into()));
        assert!(def.env_keys.contains(&"MAILJET_API_SECRET".into()));
        assert_eq!(def.env_keys.len(), 2,
            "api-mailjet must declare EXACTLY the two halves of Basic auth — no more, no less");
        assert!(def.tags.contains(&"email".into()));
        assert!(def.tags.contains(&"eu".into()),
            "api-mailjet should carry the `eu` tag — RGPD positioning is the reason it exists alongside Resend");
        assert!(def.tags.contains(&"csm".into()));
        assert!(def.default_context.is_some(), "api-mailjet must ship a default_context");
        assert_eq!(def.publisher, "Mailjet");
        assert!(def.official, "api-mailjet is by Mailjet, must be `official: true`");
        assert!(def.token_url.is_some());
        assert!(def.token_help.is_some());

        let spec = def.api_spec.as_ref().expect("api-mailjet must declare api_spec");
        assert_eq!(spec.base_url, "https://api.mailjet.com");
        match &spec.auth {
            ApiAuthKind::Basic { user_env, password_env } => {
                assert_eq!(user_env, "MAILJET_API_KEY");
                assert_eq!(password_env, "MAILJET_API_SECRET");
            }
            other => panic!("api-mailjet auth must be Basic, got {other:?}"),
        }
        assert!(!spec.endpoints.is_empty());
        assert!(spec.endpoints.iter().any(|e| e.path == "/v3.1/send" && e.method == "POST"),
            "api-mailjet must keep POST /v3.1/send (modern send envelope)");
        assert!(spec.endpoints.iter().any(|e| e.path == "/v3/REST/sender" && e.method == "GET"),
            "api-mailjet must keep GET /v3/REST/sender (sanity + sender lookup — #1 pitfall guard)");
        assert!(spec.endpoints.iter().any(|e| e.path.contains("/managecontact") && e.method == "POST"),
            "api-mailjet must keep /managecontact (the killer CSM segmentation endpoint)");
    }

    /// `search()` must surface the new email plugins by vendor name + by
    /// the `email` / `csm` tags so they're discoverable from the McpPage
    /// drawer for users not typing the exact slug.
    #[test]
    fn search_surfaces_resend_and_mailjet_email_plugins() {
        let resend_hits = search("resend");
        assert!(resend_hits.iter().any(|d| d.id == "mcp-resend"),
            "`search(\"resend\")` must return mcp-resend; got: {:?}",
            resend_hits.iter().map(|d| &d.id).collect::<Vec<_>>());

        let mailjet_hits = search("mailjet");
        assert!(mailjet_hits.iter().any(|d| d.id == "api-mailjet"),
            "`search(\"mailjet\")` must return api-mailjet; got: {:?}",
            mailjet_hits.iter().map(|d| &d.id).collect::<Vec<_>>());

        // Both should also be tag-discoverable via the `email` family.
        let email_hits = search("email");
        let ids: Vec<&String> = email_hits.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"mcp-resend".to_string()));
        assert!(ids.contains(&&"api-mailjet".to_string()));

        // CSM filter should surface both — the use case the plugins exist for.
        let csm_hits = search("csm");
        let csm_ids: Vec<&String> = csm_hits.iter().map(|d| &d.id).collect();
        assert!(csm_ids.contains(&&"mcp-resend".to_string()));
        assert!(csm_ids.contains(&&"api-mailjet".to_string()));
    }

    // ── 0.8.6 phase 4 — CLI category (audit feedback 2026-05-22) ──
    //
    // Fastly and GitLab are MCP servers but they SHELL OUT to a local
    // CLI binary (`fastly`, `glab`). From the user's install standpoint
    // they have the same prereq as a CLI agent (binary on the host),
    // so they get their own category in the McpPage filter.
    //
    // These tests pin the `cli` tag at the start of the tags array so
    // `getCategory()` (which walks tags in order) routes them to the
    // CLI bucket rather than the generic Git/Code or Cloud buckets.

    #[test]
    fn cli_wrapper_plugins_carry_the_cli_tag_first() {
        let reg = builtin_registry();
        let cli_wrappers = ["mcp-gitlab", "mcp-fastly"];
        for slug in cli_wrappers {
            let def = reg.iter().find(|d| d.id == slug)
                .unwrap_or_else(|| panic!("missing registry entry for {}", slug));
            assert_eq!(
                def.tags.first().map(String::as_str),
                Some("cli"),
                "{} must have `cli` as its FIRST tag — McpPage.getCategory() \
                 routes by first-matching tag, putting `cli` first ensures the \
                 plugin lands in the CLI-wrappers bucket. Got tags: {:?}",
                slug, def.tags,
            );
        }
    }

    #[test]
    fn cli_tag_is_unique_to_cli_wrappers() {
        // Defensive : if a non-CLI-wrapper plugin accidentally inherits
        // the `cli` tag (e.g. through copy-paste), it would silently
        // land in the wrong bucket. Pin the inverse contract — ONLY
        // mcp-gitlab + mcp-fastly carry `cli`.
        let reg = builtin_registry();
        let cli_tagged: Vec<&String> = reg.iter()
            .filter(|d| d.tags.iter().any(|t| t == "cli"))
            .map(|d| &d.id)
            .collect();
        let expected: Vec<&String> = vec![
            &"mcp-gitlab".to_string(),
            &"mcp-fastly".to_string(),
        ].into_iter().map(|s| {
            // Find the equivalent &String in the actual collection.
            cli_tagged.iter().find(|c| ***c == *s).copied()
                .unwrap_or_else(|| panic!("expected `cli` tag on {}", s))
        }).collect();
        assert_eq!(
            cli_tagged.len(),
            expected.len(),
            "ONLY mcp-gitlab + mcp-fastly should carry the `cli` tag — found: {:?}",
            cli_tagged,
        );
    }
}
