#[cfg(test)]
mod tests {
    use crate::core::registry::*;

    #[test]
    fn registry_not_empty() {
        let reg = builtin_registry();
        assert!(!reg.is_empty());
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
    fn search_by_name() {
        let results = search("github");
        assert!(results.iter().any(|m| m.id == "mcp-github"));
    }

    #[test]
    fn search_by_tag() {
        let results = search("database");
        assert!(!results.is_empty());
        // All results should have the database tag or mention it in description
        for r in &results {
            let matches = r.tags.iter().any(|t| t.contains("database"))
                || r.description.to_lowercase().contains("database");
            assert!(matches, "MCP {} doesn't match 'database'", r.id);
        }
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
            crate::models::McpTransport::Stdio { command, args } => {
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
    fn registry_count_at_least_30() {
        let reg = builtin_registry();
        assert!(reg.len() >= 30,
            "Expected at least 30 MCPs in registry, got {}", reg.len());
    }

    #[test]
    fn key_mcps_present() {
        let reg = builtin_registry();
        let required = [
            "mcp-github", "mcp-gitlab", "mcp-sentry", "mcp-slack",
            "mcp-postgres", "mcp-sqlite", "mcp-grafana", "mcp-chrome-devtools",
            "mcp-playwright", "mcp-memory", "mcp-sequential-thinking",
        ];
        for id in &required {
            assert!(reg.iter().any(|m| m.id == *id),
                "Required MCP {} not found in registry", id);
        }
    }
}
