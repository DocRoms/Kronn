#[cfg(test)]
mod tests {
    use crate::core::registry::*;
    use crate::models::McpTransport;

    // ─── Registry integrity ─────────────────────────────────────────────────────

    #[test]
    fn registry_not_empty() {
        let reg = builtin_registry();
        assert!(!reg.is_empty());
    }

    #[test]
    fn registry_count_at_least_35() {
        let reg = builtin_registry();
        assert!(reg.len() >= 49,
            "Expected at least 49 MCPs in registry, got {}", reg.len());
    }

    #[test]
    fn registry_ids_unique() {
        let reg = builtin_registry();
        let mut ids: Vec<&str> = reg.iter().map(|m| m.id.as_str()).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "Duplicate MCP IDs found in registry");
    }

    #[test]
    fn registry_names_unique() {
        let reg = builtin_registry();
        let mut names: Vec<&str> = reg.iter().map(|m| m.name.as_str()).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "Duplicate MCP names found in registry");
    }

    #[test]
    fn registry_ids_follow_naming_convention() {
        // Prefixes signal the plugin kind to anyone reading the DB or logs:
        //   `mcp-*` → plugin has an MCP transport (may also have api_spec)
        //   `api-*` → plugin is API-only (transport == ApiOnly)
        let reg = builtin_registry();
        for m in &reg {
            assert!(m.id.starts_with("mcp-") || m.id.starts_with("api-"),
                "Plugin id '{}' must start with 'mcp-' or 'api-' prefix", m.id);
            assert!(!m.id.contains(' '),
                "Plugin id '{}' must not contain spaces", m.id);
            assert_eq!(m.id, m.id.to_lowercase(),
                "Plugin id '{}' must be lowercase", m.id);
            // Consistency: api-* plugins must have api_spec, and their
            // transport must be ApiOnly.
            if m.id.starts_with("api-") {
                assert!(m.api_spec.is_some(),
                    "Plugin '{}' uses `api-` prefix but has no api_spec", m.id);
                assert!(matches!(m.transport, McpTransport::ApiOnly),
                    "Plugin '{}' uses `api-` prefix but transport is not ApiOnly", m.id);
            }
        }
    }

    #[test]
    fn registry_all_have_descriptions() {
        let reg = builtin_registry();
        for m in &reg {
            assert!(!m.description.is_empty(), "MCP {} has empty description", m.id);
        }
    }

    #[test]
    fn registry_all_have_tags() {
        let reg = builtin_registry();
        for m in &reg {
            assert!(!m.tags.is_empty(), "MCP {} has no tags", m.id);
        }
    }

    #[test]
    fn registry_all_stdio_have_valid_command() {
        let reg = builtin_registry();
        let valid_commands = ["npx", "uvx", "node", "python", "docker", "fastly-mcp", "glab"];
        for m in &reg {
            if let McpTransport::Stdio { command, .. } = &m.transport {
                assert!(valid_commands.contains(&command.as_str()),
                    "MCP {} uses unknown command '{}' (expected one of {:?})",
                    m.id, command, valid_commands);
            }
        }
    }

    #[test]
    fn registry_all_sse_have_valid_url() {
        let reg = builtin_registry();
        for m in &reg {
            if let McpTransport::Sse { url } = &m.transport {
                assert!(url.starts_with("http://") || url.starts_with("https://"),
                    "MCP {} SSE url '{}' must start with http(s)://", m.id, url);
            }
        }
    }

    #[test]
    fn registry_env_keys_with_token_servers_have_token_url_or_help() {
        let reg = builtin_registry();
        for m in &reg {
            if !m.env_keys.is_empty() {
                let has_guidance = m.token_url.is_some() || m.token_help.is_some();
                assert!(has_guidance,
                    "MCP {} requires env keys {:?} but has no token_url or token_help",
                    m.id, m.env_keys);
            }
        }
    }

    #[test]
    fn all_registry_mcps_have_complete_data() {
        let reg = builtin_registry();
        for m in &reg {
            assert!(!m.id.is_empty(), "MCP has empty id");
            assert!(!m.name.is_empty(), "MCP {} has empty name", m.id);
            assert!(!m.description.is_empty(), "MCP {} has empty description", m.id);
            assert!(!m.tags.is_empty(), "MCP {} has no tags", m.id);
        }
    }

    // ─── Key MCPs presence ──────────────────────────────────────────────────────

    #[test]
    fn key_mcps_present() {
        let reg = builtin_registry();
        let required = [
            // Core / AI
            "mcp-memory", "mcp-sequential-thinking", "mcp-filesystem",
            // Git & Code
            "mcp-github", "mcp-gitlab", "mcp-git",
            // Databases
            "mcp-postgres", "mcp-sqlite", "mcp-redis", "mcp-neon", "mcp-mongodb", "mcp-qdrant",
            // Monitoring
            "mcp-sentry", "mcp-grafana", "mcp-datadog",
            // Cloud & Analytics
            "mcp-cloudflare", "mcp-aws-cloudwatch", "mcp-aws-api", "mcp-azure", "mcp-gcloud", "mcp-bigquery",
            "mcp-google-analytics",
            // Browser & Testing
            "mcp-playwright", "mcp-chrome-devtools",
            // CDN & Edge
            "mcp-fastly",
            // Code Quality & IaC
            "mcp-sonarqube", "mcp-terraform",
            // Hosting
            "mcp-vercel",
            // Search
            "mcp-tavily",
            // Compute
            "mcp-google-colab",
            // Data Federation
            "mcp-mindsdb",
            // Cloud — containers
            "mcp-kubernetes",
            // Search
            "mcp-perplexity",
            // Communication & PM
            "mcp-slack", "mcp-linear", "mcp-atlassian", "mcp-microsoft-365",
            // Design
            "mcp-figma", "mcp-drawio",
            // Knowledge & Docs
            "mcp-notion", "mcp-context7",
        ];
        for id in &required {
            assert!(reg.iter().any(|m| m.id == *id),
                "Required MCP {} not found in registry", id);
        }
    }

    // ─── Individual MCP validation ──────────────────────────────────────────────

    #[test]
    fn memory_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-memory").unwrap();
        assert!(m.description.contains("knowledge graph"), "Memory MCP should mention knowledge graph");
        assert!(m.env_keys.is_empty(), "Memory MCP needs no API keys");
        assert!(m.tags.contains(&"memory".to_string()));
        assert!(m.tags.contains(&"core".to_string()));
        match &m.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert!(args.iter().any(|a| a.contains("server-memory")),
                    "Memory MCP args should reference server-memory package");
            }
            _ => panic!("Memory MCP should use Stdio transport"),
        }
    }

    #[test]
    fn gcloud_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-gcloud").unwrap();
        assert!(m.tags.contains(&"gcp".to_string()));
        assert!(m.tags.contains(&"cloud".to_string()));
        assert!(m.env_keys.is_empty(), "gcloud MCP uses CLI auth, not API keys");
        match &m.transport {
            McpTransport::Stdio { command, .. } => {
                assert_eq!(command, "npx");
            }
            _ => panic!("gcloud MCP should use Stdio transport"),
        }
    }

    #[test]
    fn bigquery_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-bigquery").unwrap();
        assert!(m.tags.contains(&"sql".to_string()));
        assert!(m.tags.contains(&"gcp".to_string()));
        assert!(m.env_keys.contains(&"GOOGLE_PROJECT_ID".to_string()),
            "BigQuery requires GOOGLE_PROJECT_ID");
    }

    #[test]
    fn neon_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-neon").unwrap();
        assert!(m.tags.contains(&"postgres".to_string()));
        assert!(m.tags.contains(&"serverless".to_string()));
        assert!(m.env_keys.contains(&"NEON_API_KEY".to_string()));
    }

    #[test]
    fn datadog_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-datadog").unwrap();
        assert!(m.tags.contains(&"monitoring".to_string()));
        assert!(m.env_keys.contains(&"DD_API_KEY".to_string()));
        assert!(m.env_keys.contains(&"DD_APP_KEY".to_string()));
    }

    #[test]
    fn grafana_in_registry() {
        let reg = builtin_registry();
        let grafana = reg.iter().find(|m| m.id == "mcp-grafana");
        assert!(grafana.is_some(), "Grafana MCP should be in registry");
        let g = grafana.unwrap();
        assert!(g.env_keys.contains(&"GRAFANA_URL".to_string()));
        assert!(g.env_keys.contains(&"GRAFANA_SERVICE_ACCOUNT_TOKEN".to_string()));
    }

    #[test]
    fn grafana_uses_uvx_transport() {
        let reg = builtin_registry();
        let g = reg.iter().find(|m| m.id == "mcp-grafana").unwrap();
        match &g.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "uvx", "Grafana should use uvx, not npx");
                assert!(args.contains(&"mcp-grafana".to_string()));
            }
            _ => panic!("Grafana should use Stdio transport"),
        }
    }

    #[test]
    fn chrome_devtools_in_registry() {
        let reg = builtin_registry();
        let chrome = reg.iter().find(|m| m.id == "mcp-chrome-devtools");
        assert!(chrome.is_some(), "Chrome DevTools MCP should be in registry");
        let c = chrome.unwrap();
        assert!(!c.description.is_empty());
        assert!(c.tags.iter().any(|t| t.contains("browser") || t.contains("debug")));
    }

    #[test]
    fn playwright_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-playwright").unwrap();
        assert!(m.tags.contains(&"testing".to_string()));
        assert!(m.tags.contains(&"e2e".to_string()));
        assert!(m.env_keys.is_empty(), "Playwright MCP needs no API keys");
    }

    #[test]
    fn figma_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-figma").unwrap();
        assert!(m.description.contains("Dev Mode"), "Figma MCP should mention Dev Mode");
        assert!(m.tags.contains(&"design".to_string()));
        assert!(m.tags.contains(&"ui".to_string()));
        assert!(m.env_keys.contains(&"FIGMA_API_KEY".to_string()));
        assert!(m.token_url.is_some());
    }

    #[test]
    fn sentry_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-sentry").unwrap();
        assert!(m.tags.contains(&"monitoring".to_string()));
        assert!(m.env_keys.contains(&"SENTRY_AUTH_TOKEN".to_string()));
        assert!(m.token_url.is_some());
    }

    #[test]
    fn linear_uses_sse_transport() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-linear").unwrap();
        match &m.transport {
            McpTransport::Sse { url } => {
                assert!(url.contains("linear.app"), "Linear SSE URL should point to linear.app");
            }
            _ => panic!("Linear should use SSE transport"),
        }
    }

    // ─── Search function ────────────────────────────────────────────────────────

    #[test]
    fn search_by_name() {
        let results = search("github");
        assert!(results.iter().any(|m| m.id == "mcp-github"));
    }

    #[test]
    fn search_by_tag() {
        let results = search("database");
        assert!(!results.is_empty());
        for r in &results {
            let matches = r.tags.iter().any(|t| t.contains("database"))
                || r.description.to_lowercase().contains("database");
            assert!(matches, "MCP {} doesn't match 'database'", r.id);
        }
    }

    #[test]
    fn search_by_tag_monitoring_finds_multiple() {
        let results = search("monitoring");
        assert!(results.len() >= 3,
            "Should find at least Sentry, Grafana, Datadog for 'monitoring', got {}",
            results.len());
    }

    #[test]
    fn search_by_tag_cloud_finds_multiple() {
        let results = search("cloud");
        assert!(results.len() >= 3,
            "Should find multiple cloud providers for 'cloud', got {}",
            results.len());
    }

    #[test]
    fn search_case_insensitive() {
        let r1 = search("GitHub");
        let r2 = search("github");
        assert_eq!(r1.len(), r2.len());
    }

    #[test]
    fn search_no_results() {
        let results = search("zzz_nonexistent_xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn search_by_description() {
        let results = search("knowledge graph");
        assert!(results.iter().any(|m| m.id == "mcp-memory"),
            "Searching 'knowledge graph' should find Memory MCP");
    }

    #[test]
    fn fastly_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-fastly").unwrap();
        assert_eq!(m.name, "Fastly");
        assert!(m.description.contains("CDN"), "Fastly MCP should mention CDN");
        assert!(m.env_keys.is_empty(), "Official Fastly MCP uses CLI profiles, not env vars");
        assert!(m.tags.contains(&"cdn".to_string()));
        assert!(m.token_url.is_some());
        assert!(m.token_help.as_ref().unwrap().contains("fastly profile create"),
            "token_help should guide users to create a Fastly CLI profile");
    }

    #[test]
    fn tavily_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-tavily").unwrap();
        assert_eq!(m.name, "Tavily");
        assert!(m.description.contains("search"), "Tavily MCP should mention search");
        assert!(m.env_keys.contains(&"TAVILY_API_KEY".to_string()));
        assert!(m.tags.contains(&"search".to_string()));
    }

    #[test]
    fn google_colab_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-google-colab").unwrap();
        assert_eq!(m.name, "Google Colab");
        assert!(m.description.contains("GPU"), "Colab MCP should mention GPU");
        assert!(m.tags.contains(&"compute".to_string()));
        assert!(m.env_keys.is_empty(), "Colab uses browser auth, no API key");
    }

    #[test]
    fn aws_api_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-aws-api").unwrap();
        assert_eq!(m.name, "AWS API");
        assert!(m.description.contains("Unified"), "AWS API MCP should mention unified access");
        assert!(m.env_keys.contains(&"AWS_ACCESS_KEY_ID".to_string()));
        assert!(m.env_keys.contains(&"AWS_SECRET_ACCESS_KEY".to_string()));
        assert!(m.tags.contains(&"aws".to_string()));
        assert!(m.token_url.is_some());
    }

    #[test]
    fn google_analytics_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-google-analytics").unwrap();
        assert_eq!(m.name, "Google Analytics 4");
        assert!(m.description.contains("GA4"), "GA4 MCP should mention GA4");
        assert!(m.env_keys.contains(&"GOOGLE_APPLICATION_CREDENTIALS".to_string()));
        assert!(m.tags.contains(&"analytics".to_string()));
        assert!(m.token_url.is_some());
        match &m.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "uvx");
                assert!(args.contains(&"analytics-mcp".to_string()));
            }
            _ => panic!("GA4 MCP should use Stdio transport"),
        }
    }

    #[test]
    fn sonarqube_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-sonarqube").unwrap();
        assert_eq!(m.name, "SonarQube");
        assert!(m.env_keys.contains(&"SONARQUBE_TOKEN".to_string()));
        assert!(m.tags.contains(&"quality".to_string()));
        assert!(m.token_url.is_some());
        match &m.transport {
            McpTransport::Stdio { command, .. } => assert_eq!(command, "docker"),
            _ => panic!("SonarQube MCP should use Stdio transport"),
        }
    }

    #[test]
    fn terraform_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-terraform").unwrap();
        assert_eq!(m.name, "Terraform");
        assert!(m.env_keys.contains(&"TFE_TOKEN".to_string()));
        assert!(m.tags.contains(&"iac".to_string()));
        assert!(m.token_url.is_some());
    }

    #[test]
    fn vercel_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-vercel").unwrap();
        assert_eq!(m.name, "Vercel");
        assert!(m.env_keys.is_empty(), "Vercel uses OAuth, no API key");
        assert!(m.tags.contains(&"deploy".to_string()));
        match &m.transport {
            McpTransport::Streamable { url } => {
                assert!(url.contains("mcp.vercel.com"));
            }
            _ => panic!("Vercel MCP should use Streamable transport"),
        }
    }

    #[test]
    fn redis_uses_official_server() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-redis").unwrap();
        assert!(m.description.contains("official Redis"), "Redis should use official Redis server, not Anthropic's");
        match &m.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "uvx");
                assert!(args.iter().any(|a| a.contains("redis-mcp-server")));
            }
            _ => panic!("Redis MCP should use Stdio transport"),
        }
    }

    #[test]
    fn drawio_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-drawio").unwrap();
        assert_eq!(m.name, "draw.io");
        assert!(m.description.contains("diagrams"), "draw.io MCP should mention diagrams");
        assert!(m.tags.contains(&"design".to_string()));
        assert!(m.tags.contains(&"diagrams".to_string()));
        assert!(m.env_keys.is_empty(), "draw.io MCP needs no API keys");
        match &m.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert!(args.iter().any(|a| a.contains("drawio-mcp")),
                    "draw.io MCP args should reference drawio-mcp package");
            }
            _ => panic!("draw.io MCP should use Stdio transport"),
        }
    }

    #[test]
    fn mindsdb_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-mindsdb").unwrap();
        assert_eq!(m.name, "MindsDB");
        assert!(m.env_keys.contains(&"MINDS_API_KEY".to_string()));
        assert!(m.tags.contains(&"database".to_string()));
    }

    // ── Publisher / official tests ──

    #[test]
    fn all_entries_have_publisher() {
        for m in builtin_registry() {
            assert!(!m.publisher.is_empty(), "MCP {} has empty publisher", m.id);
        }
    }

    #[test]
    fn official_mcps_are_by_vendor() {
        let reg = builtin_registry();
        // Fastly MCP should be official by Fastly
        let fastly = reg.iter().find(|m| m.id == "mcp-fastly").unwrap();
        assert!(fastly.official);
        assert_eq!(fastly.publisher, "Fastly");

        // GitLab MCP should be official by GitLab
        let gitlab = reg.iter().find(|m| m.id == "mcp-gitlab").unwrap();
        assert!(gitlab.official);
        assert_eq!(gitlab.publisher, "GitLab");

        // GitHub MCP is by Anthropic, not official by vendor
        let github = reg.iter().find(|m| m.id == "mcp-github").unwrap();
        assert!(!github.official);
        assert_eq!(github.publisher, "Anthropic");
    }

    #[test]
    fn vendor_official_mcps_exist() {
        let reg = builtin_registry();
        let official_count = reg.iter().filter(|m| m.official).count();
        let community_count = reg.iter().filter(|m| !m.official).count();
        // Most MCPs should be vendor-official
        assert!(official_count > community_count,
            "Expected more official ({}) than community ({}) MCPs",
            official_count, community_count);
    }

    // ─── New MCPs (0.3.3+) ───────────────────────────────────────────────────

    #[test]
    fn puppeteer_removed_from_registry() {
        let reg = builtin_registry();
        assert!(reg.iter().all(|m| m.id != "mcp-puppeteer"),
            "Puppeteer should be removed — use Playwright instead");
    }

    #[test]
    fn mongodb_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-mongodb").expect("mcp-mongodb missing");
        assert_eq!(m.publisher, "MongoDB");
        assert!(m.official);
        assert!(m.env_keys.contains(&"MDB_MCP_CONNECTION_STRING".into()));
        match &m.transport { McpTransport::Stdio { command, .. } => assert_eq!(command, "npx"), _ => panic!("expected Stdio") }
    }

    #[test]
    fn kubernetes_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-kubernetes").expect("mcp-kubernetes missing");
        assert_eq!(m.publisher, "Red Hat");
        assert!(m.official);
        assert!(m.tags.contains(&"containers".into()));
    }

    #[test]
    fn qdrant_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-qdrant").expect("mcp-qdrant missing");
        assert_eq!(m.publisher, "Qdrant");
        assert!(m.official);
        assert!(m.env_keys.contains(&"QDRANT_URL".into()));
        match &m.transport { McpTransport::Stdio { command, .. } => assert_eq!(command, "uvx"), _ => panic!("expected Stdio") }
    }

    #[test]
    fn perplexity_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-perplexity").expect("mcp-perplexity missing");
        assert_eq!(m.publisher, "Perplexity");
        assert!(m.official);
        assert!(m.env_keys.contains(&"PERPLEXITY_API_KEY".into()));
    }

    #[test]
    fn microsoft_365_mcp_configuration() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-microsoft-365").expect("mcp-microsoft-365 missing");
        assert_eq!(m.publisher, "Softeria (community)");
        assert!(!m.official);
        assert!(m.tags.contains(&"email".into()));
        assert!(m.tags.contains(&"teams".into()));
    }

    #[test]
    fn google_analytics_publisher_is_community() {
        let reg = builtin_registry();
        let m = reg.iter().find(|m| m.id == "mcp-google-analytics").unwrap();
        assert_eq!(m.publisher, "Community", "GA4 MCP is not by Google — should be Community");
        assert!(!m.official);
    }

    #[test]
    fn docker_required_mcps_mention_docker_in_help() {
        let reg = builtin_registry();
        for id in &["mcp-sonarqube", "mcp-terraform"] {
            let m = reg.iter().find(|m| m.id == *id).unwrap();
            match &m.transport {
                McpTransport::Stdio { command, .. } => assert_eq!(command, "docker", "{} should use docker", id),
                _ => panic!("{} should be Stdio", id),
            }
            assert!(m.token_help.as_ref().map(|h| h.contains("Docker")).unwrap_or(false),
                "{} help should mention Docker requirement", id);
        }
    }

    #[test]
    fn anthropic_mcps_are_not_vendor_official() {
        // Anthropic-built MCPs for third-party services should NOT be marked official
        let reg = builtin_registry();
        let anthropic = reg.iter().filter(|m| m.publisher == "Anthropic").collect::<Vec<_>>();
        assert!(!anthropic.is_empty(), "Should have Anthropic-published MCPs");
        for m in &anthropic {
            assert!(!m.official,
                "MCP {} is by Anthropic but marked official — only the service vendor should be official",
                m.id);
        }
    }
}
