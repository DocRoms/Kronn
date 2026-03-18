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
    fn registry_count_at_least_34() {
        let reg = builtin_registry();
        assert!(reg.len() >= 34,
            "Expected at least 34 MCPs in registry, got {}", reg.len());
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
        let reg = builtin_registry();
        for m in &reg {
            assert!(m.id.starts_with("mcp-"),
                "MCP id '{}' must start with 'mcp-' prefix", m.id);
            assert!(!m.id.contains(' '),
                "MCP id '{}' must not contain spaces", m.id);
            assert_eq!(m.id, m.id.to_lowercase(),
                "MCP id '{}' must be lowercase", m.id);
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
        let valid_commands = ["npx", "uvx", "node", "python", "docker"];
        for m in &reg {
            if let McpTransport::Stdio { command, args } = &m.transport {
                assert!(valid_commands.contains(&command.as_str()),
                    "MCP {} uses unknown command '{}' (expected one of {:?})",
                    m.id, command, valid_commands);
                assert!(!args.is_empty(),
                    "MCP {} has Stdio transport but no args", m.id);
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
            "mcp-postgres", "mcp-sqlite", "mcp-redis", "mcp-neon",
            // Monitoring
            "mcp-sentry", "mcp-grafana", "mcp-datadog",
            // Cloud
            "mcp-cloudflare", "mcp-aws-cloudwatch", "mcp-azure", "mcp-gcloud", "mcp-bigquery",
            // Browser & Testing
            "mcp-playwright", "mcp-chrome-devtools", "mcp-puppeteer",
            // Communication & PM
            "mcp-slack", "mcp-linear", "mcp-atlassian",
            // Design
            "mcp-figma",
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
}
