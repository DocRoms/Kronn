use crate::models::{
    ApiAuthKind, ApiConfigKey, ApiEndpoint, ApiSpec, McpDefinition, McpTransport,
    OAuth2ExtraHeader,
};

/// Return the built-in MCP registry — official servers only
pub fn builtin_registry() -> Vec<McpDefinition> {
    vec![
        // ── Git & Code ──────────────────────────────────────────────────────
        McpDefinition {
            id: "mcp-github".into(),
            name: "GitHub".into(),
            description: "Issues, PRs, Actions, repos — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            },
            env_keys: vec!["GITHUB_PERSONAL_ACCESS_TOKEN".into()],
            tags: vec!["git".into(), "ci".into(), "code".into()],
            token_url: Some("https://github.com/settings/tokens?type=beta".into()),
            token_help: Some("Fine-grained PAT with repo access".into()),
            publisher: "Anthropic".into(),
            official: false,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
        },
        McpDefinition {
            id: "mcp-gitlab".into(),
            name: "GitLab".into(),
            description: "Issues, MRs, pipelines, projects — official GitLab CLI MCP server (experimental)".into(),
            transport: McpTransport::Stdio {
                command: "glab".into(),
                args: vec!["mcp".into(), "serve".into()],
            },
            env_keys: vec!["GITLAB_TOKEN".into(), "GITLAB_HOST".into()],
            tags: vec!["git".into(), "ci".into(), "code".into()],
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
        McpDefinition {
            id: "mcp-atlassian".into(),
            name: "Atlassian".into(),
            description: "Jira + Confluence — official MCP server".into(),
            transport: McpTransport::Stdio {
                command: "uvx".into(),
                args: vec!["mcp-atlassian".into()],
            },
            env_keys: vec![
                "JIRA_URL".into(), "JIRA_USERNAME".into(), "JIRA_API_TOKEN".into(),
                "CONFLUENCE_URL".into(), "CONFLUENCE_USERNAME".into(), "CONFLUENCE_API_TOKEN".into(),
            ],
            tags: vec!["project-management".into(), "jira".into(), "confluence".into()],
            token_url: Some("https://id.atlassian.com/manage-profile/security/api-tokens".into()),
            token_help: Some("API token for Jira & Confluence (same token for both)".into()),
            publisher: "Atlassian".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
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
        McpDefinition {
            id: "mcp-resend".into(),
            name: "Resend".into(),
            description: "Send transactional emails — official Resend MCP server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "resend-mcp".into()],
            },
            env_keys: vec!["RESEND_API_KEY".into()],
            tags: vec!["email".into(), "mailing".into(), "communication".into()],
            token_url: Some("https://resend.com/api-keys".into()),
            token_help: Some("API key from Resend dashboard".into()),
            publisher: "Resend".into(),
            official: true,
            alt_packages: vec![],
            default_context: None,
            api_spec: None,
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
            description: "CDN management, cache purge, VCL, WAF, backends, domains, stats — official Fastly Go server (wraps Fastly CLI)".into(),
            transport: McpTransport::Stdio {
                command: "fastly-mcp".into(),
                args: vec![],
            },
            env_keys: vec![],
            tags: vec!["cdn".into(), "cache".into(), "infrastructure".into(), "edge".into(), "waf".into()],
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

## 2. Historical / Query API — `X-CB-AK` HEADER, ASYNCHRONOUS

**Critical**: historical endpoints do NOT accept `apikey` as a query
parameter. They require the header `X-CB-AK: <KEY>`. Passing the key
in the URL on `/historical/...` or `/query/...` returns 401 or 404
that LOOK like access errors — it's just the wrong auth channel.

Two endpoint shapes observed in the wild, both asynchronous:

- **Modern**: `/query/v2/submit/page/` then `/query/v2/status/?query_id=<id>` then `/query/v2/fetch/?query_id=<id>`
- **Legacy**: `/historical/traffic/series/` (accepts the header directly, returns data synchronously in most cases — but still OK to retry with the modern flow if it doesn't)

### The 3-step async flow (modern, `/query/v2/...`)

```bash
# 1. Submit — returns { "query_id": "..." }
curl -s "https://api.chartbeat.com/query/v2/submit/page/?host=example.com&start=2026-04-13&end=2026-04-20" \
     -H "X-CB-AK: <KEY>"

# 2. Poll status — { "status": "running" | "completed" | "failed" }
curl -s "https://api.chartbeat.com/query/v2/status/?query_id=<QID>" -H "X-CB-AK: <KEY>"

# 3. Fetch the actual data once status=completed
curl -s "https://api.chartbeat.com/query/v2/fetch/?query_id=<QID>" -H "X-CB-AK: <KEY>"
```

### Polling loop template

```bash
qid=$(curl -s "https://api.chartbeat.com/query/v2/submit/page/?host=example.com&start=2026-04-13&end=2026-04-20" \
  -H "X-CB-AK: <KEY>" | jq -r .query_id)
deadline=$(($(date +%s) + 30))
while :; do
  st=$(curl -s "https://api.chartbeat.com/query/v2/status/?query_id=$qid" -H "X-CB-AK: <KEY>" | jq -r .status)
  [ "$st" = "completed" ] && break
  [ "$st" = "failed" ] && { echo "query failed"; exit 1; }
  [ $(date +%s) -ge $deadline ] && { echo "timeout"; exit 1; }
  sleep 1
done
curl -s "https://api.chartbeat.com/query/v2/fetch/?query_id=$qid" -H "X-CB-AK: <KEY>"
```

### Legacy historical — still works with the header

```bash
# /historical/traffic/series/ accepts the header, returns series JSON directly.
curl -s "https://api.chartbeat.com/historical/traffic/series/?host=example.com&start=2026-04-19&end=2026-04-20&frequency=hour" \
     -H "X-CB-AK: <KEY>"
```

Prefer `/query/v2/...` for new queries; fall back to `/historical/...`
only if the modern path returns 404 for the specific metric.

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
- A 404 on `/historical/...` is almost always a wrong-auth or
  wrong-endpoint issue, NOT an API-key scope limit. Verify by:
  (a) switching to the header-based auth, (b) trying `/query/v2/...`
  variant, (c) checking the user's API key scope page shows `all`.

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
                    // ── Live Publishing API (synchronous GET, apikey= query param) ──
                    ApiEndpoint { path: "/live/dashapi/v4".into(),           method: "GET".into(), description: "[LIVE · apikey= · sync] Full live dashboard snapshot (visits, engagement, top pages)".into() },
                    ApiEndpoint { path: "/live/toppages/v4".into(),          method: "GET".into(), description: "[LIVE · apikey= · sync] Top pages right now, ranked by concurrents".into() },
                    ApiEndpoint { path: "/live/quickstats/v4".into(),        method: "GET".into(), description: "[LIVE · apikey= · sync] Aggregate stats (visits, engaged time, referrer types)".into() },
                    ApiEndpoint { path: "/live/recent/v3".into(),            method: "GET".into(), description: "[LIVE · apikey= · sync] Recent visitor activity (5-min granularity — ideal for short-window dip analysis)".into() },
                    ApiEndpoint { path: "/live/referrers/v3".into(),         method: "GET".into(), description: "[LIVE · apikey= · sync] Top referrers bringing traffic now".into() },
                    ApiEndpoint { path: "/live/top_geo/v3".into(),           method: "GET".into(), description: "[LIVE · apikey= · sync] Top countries / regions".into() },
                    ApiEndpoint { path: "/live/summary/v3".into(),           method: "GET".into(), description: "[LIVE · apikey= · sync] Concise traffic summary for a section".into() },
                    ApiEndpoint { path: "/live/social/v3".into(),            method: "GET".into(), description: "[LIVE · apikey= · sync] Social-network referrers breakdown".into() },
                    ApiEndpoint { path: "/live/top_devices/v3".into(),       method: "GET".into(), description: "[LIVE · apikey= · sync] Top device types".into() },
                    ApiEndpoint { path: "/live/video/sources/v1".into(),     method: "GET".into(), description: "[LIVE · apikey= · sync] Live video sources".into() },
                    ApiEndpoint { path: "/live/video/top_videos/v1".into(),  method: "GET".into(), description: "[LIVE · apikey= · sync] Live top videos".into() },
                    // ── Historical / Query — `X-CB-AK` HEADER, async submit/status/fetch ──
                    ApiEndpoint { path: "/query/v2/submit/page/".into(),     method: "GET".into(), description: "[QUERY · X-CB-AK · async-submit] Modern historical query — returns {query_id}. Use header auth, NOT apikey= param.".into() },
                    ApiEndpoint { path: "/query/v2/status/".into(),          method: "GET".into(), description: "[QUERY · X-CB-AK · async-status] ?query_id=<id> → {status: running|completed|failed}".into() },
                    ApiEndpoint { path: "/query/v2/fetch/".into(),           method: "GET".into(), description: "[QUERY · X-CB-AK · async-fetch] ?query_id=<id> → final data (poll /status/ until completed first)".into() },
                    ApiEndpoint { path: "/historical/traffic/series/".into(),method: "GET".into(), description: "[HIST-legacy · X-CB-AK · often sync] Traffic time series with frequency=hour|minute|day. Accepts the header and usually returns data without the submit/fetch dance.".into() },
                    ApiEndpoint { path: "/historical/traffic/stats/".into(), method: "GET".into(), description: "[HIST-legacy · X-CB-AK] Aggregate historical traffic stats (legacy; prefer /query/v2/submit/page/ for new code)".into() },
                    ApiEndpoint { path: "/historical/topreferrers/".into(),  method: "GET".into(), description: "[HIST-legacy · X-CB-AK] Top referrers over a date range".into() },
                    ApiEndpoint { path: "/historical/authors/".into(),       method: "GET".into(), description: "[HIST-legacy · X-CB-AK] Author performance over a date range".into() },
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
}
