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
        },
        McpDefinition {
            id: "mcp-gitlab".into(),
            name: "GitLab".into(),
            description: "Issues, MRs, pipelines, repos — official GitLab server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-gitlab".into()],
            },
            env_keys: vec!["GITLAB_PERSONAL_ACCESS_TOKEN".into(), "GITLAB_API_URL".into()],
            tags: vec!["git".into(), "ci".into(), "code".into()],
            token_url: Some("https://gitlab.com/-/user_settings/personal_access_tokens".into()),
            token_help: Some("PAT with api scope".into()),
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
        },
        McpDefinition {
            id: "mcp-redis".into(),
            name: "Redis".into(),
            description: "Cache, pub/sub, streams — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-redis".into()],
            },
            env_keys: vec!["REDIS_URL".into()],
            tags: vec!["cache".into(), "database".into()],
            token_url: None,
            token_help: Some("Redis URL: redis://host:6379".into()),
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
        },
        McpDefinition {
            id: "mcp-grafana".into(),
            name: "Grafana".into(),
            description: "Dashboards, datasources, alerts, incidents — official Grafana server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@grafana/mcp-server-grafana".into()],
            },
            env_keys: vec!["GRAFANA_URL".into(), "GRAFANA_API_KEY".into()],
            tags: vec!["monitoring".into(), "dashboards".into(), "observability".into()],
            token_url: Some("https://grafana.com/docs/grafana/latest/administration/service-accounts/".into()),
            token_help: Some("Service account token with Viewer role minimum".into()),
        },
        McpDefinition {
            id: "mcp-google-analytics".into(),
            name: "Google Analytics".into(),
            description: "GA4 data, reports, realtime — community server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "mcp-server-google-analytics".into()],
            },
            env_keys: vec!["GA4_PROPERTY_ID".into(), "GA4_CREDENTIALS_PATH".into()],
            tags: vec!["analytics".into(), "google".into()],
            token_url: Some("https://console.cloud.google.com/apis/credentials".into()),
            token_help: Some("Service account JSON with GA4 read access".into()),
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
        },
        // ── Project Management ──────────────────────────────────────────────
        McpDefinition {
            id: "mcp-linear".into(),
            name: "Linear".into(),
            description: "Issues, projects, teams — official Linear SSE server".into(),
            transport: McpTransport::Sse {
                url: "https://mcp.linear.app/sse".into(),
            },
            env_keys: vec!["LINEAR_API_KEY".into()],
            tags: vec!["project-management".into(), "issues".into()],
            token_url: Some("https://linear.app/settings/api".into()),
            token_help: Some("Personal API key".into()),
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
            token_url: Some("https://www.figma.com/developers/api#access-tokens".into()),
            token_help: Some("Personal access token from Figma Settings > Security".into()),
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
        },
        McpDefinition {
            id: "mcp-puppeteer".into(),
            name: "Puppeteer".into(),
            description: "Browser automation and screenshots — official Anthropic server".into(),
            transport: McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-puppeteer".into()],
            },
            env_keys: vec![],
            tags: vec!["browser".into(), "scraping".into()],
            token_url: None,
            token_help: None,
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
