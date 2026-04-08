use crate::models::{McpDefinition, McpTransport};

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
            token_help: Some("Requires Fastly CLI installed (brew install fastly/tap/fastly or https://developer.fastly.com/reference/cli/). Then run: fastly profile create <name> and enter your API token. No env var needed — auth is read from CLI profiles.".into()),
            publisher: "Fastly".into(),
            official: true,
            alt_packages: vec!["fastly-mcp-server".into()],
            default_context: Some(r#"# Fastly — Usage Context

> Instructions for AI agents using **Fastly** MCP in this project.

**Server:** Official Fastly MCP (Go binary wrapping Fastly CLI)

## Performance rules

- **Results are often very large** (100K+ chars for service listings). Always use `fastly_result_read` with small `limit` (5-10) or `fastly_result_query` with filters to reduce output.
- If a result exceeds token limits and gets saved to disk, parse the file with `jq` or `python3`:
  ```
  jq '.[0].text | fromjson | .data[] | {Name, ServiceID, ActiveVersion}' <file>
  ```
- The MCP result format is `[{"type": "text", "text": "<JSON_STRING>"}]` where the inner JSON has a `data` key containing the actual array.

## Common operations

- List services: `fastly_execute(command: "service", args: ["list"], flags: [{"name": "json"}])`
- Purge all: `fastly_execute(command: "purge", args: ["--all"], flags: [{"name": "service-id", "value": "<ID>"}])`
- Show domain: `fastly_execute(command: "domain", args: ["list"], flags: [{"name": "service-id", "value": "<ID>"}, {"name": "version", "value": "active"}])`

## Rules

- Always use `--json` flag when available to get structured output
- Never purge without explicit user confirmation
- Prefer `fastly_result_summary` to get an overview before reading full results
"#.into()),
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
