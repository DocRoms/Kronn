#[cfg(test)]
mod tests {
    use crate::core::scanner::*;

    #[test]
    fn resolve_host_path_no_env() {
        // Without KRONN_HOST_HOME, paths should pass through unchanged
        std::env::remove_var("KRONN_HOST_HOME");
        let result = resolve_host_path("/some/local/path");
        assert_eq!(result.to_string_lossy(), "/some/local/path");
    }

    #[test]
    fn detect_audit_status_no_ai_dir() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-no-dir");
        let _ = std::fs::create_dir_all(&tmp);
        // No ai/ dir → NoTemplate
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::NoTemplate));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_with_bootstrap() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-bootstrap");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\nKRONN:BOOTSTRAP:START\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::TemplateInstalled));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_bootstrapped() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-bootstrapped");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\n<!-- KRONN:BOOTSTRAPPED:2026-03-14 -->\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Bootstrapped));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_bootstrapped_and_validated() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-bootstrapped-validated");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\n<!-- KRONN:BOOTSTRAPPED:2026-03-14 -->\n<!-- KRONN:VALIDATED:2026-03-14 -->\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Validated));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_with_placeholder() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-placeholder");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# {{PROJECT_NAME}}\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::TemplateInstalled));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_validated() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-validated");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# Project\nKRONN:VALIDATED\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Validated));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_audit_status_audited() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-audited");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(ai_dir.join("index.md"), "# My Project\nFilled content\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::Audited));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_empty() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-empty");
        let _ = std::fs::create_dir_all(&tmp);
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_with_markers() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-markers");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("index.md"),
            "# Project\n<!-- TODO: verify -->\nSome text\n<!-- TODO: check -->\n",
        ).unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── scan_paths_with_depth: ignore list ────────────────────────────────────

    #[tokio::test]
    async fn scan_skips_ignored_directories() {
        let tmp = std::env::temp_dir().join("kronn-test-scan-ignore");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("Library/some-app/.git")).unwrap();
        std::fs::create_dir_all(tmp.join("real-project/.git")).unwrap();

        let ignore = vec!["Library".into()];
        let repos = scan_paths_with_depth(
            &[tmp.to_string_lossy().to_string()],
            &ignore,
            3,
        ).await.unwrap();

        // "Library" should be ignored, only "real-project" found
        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"real-project"), "Expected real-project, got: {:?}", names);
        assert!(!names.iter().any(|n| n.contains("Library")), "Library should be ignored");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn scan_ignore_is_case_insensitive() {
        let tmp = std::env::temp_dir().join("kronn-test-scan-case");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("NODE_MODULES/foo/.git")).unwrap();
        std::fs::create_dir_all(tmp.join("my-repo/.git")).unwrap();

        let ignore = vec!["node_modules".into()];
        let repos = scan_paths_with_depth(
            &[tmp.to_string_lossy().to_string()],
            &ignore,
            3,
        ).await.unwrap();

        let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        assert!(!names.iter().any(|n| n.contains("NODE_MODULES")),
            "NODE_MODULES should be ignored (case-insensitive)");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn scan_nonexistent_path_returns_empty() {
        let repos = scan_paths_with_depth(
            &["/nonexistent/path/that/does/not/exist".into()],
            &[],
            3,
        ).await.unwrap();
        assert!(repos.is_empty());
    }

    #[tokio::test]
    async fn scan_empty_paths_returns_empty() {
        let repos = scan_paths_with_depth(&[], &[], 3).await.unwrap();
        assert!(repos.is_empty());
    }

    // ─── default_config ignore list ────────────────────────────────────────────

    #[test]
    fn default_config_ignores_macos_dirs() {
        let config = crate::core::config::default_config();
        let ignore = &config.scan.ignore;
        assert!(ignore.contains(&"Library".to_string()), "Should ignore macOS Library");
        assert!(ignore.contains(&".Trash".to_string()), "Should ignore macOS .Trash");
        assert!(ignore.contains(&"node_modules".to_string()), "Should ignore node_modules");
    }

    #[test]
    fn default_config_ignores_cache_dirs() {
        let config = crate::core::config::default_config();
        let ignore = &config.scan.ignore;
        assert!(ignore.contains(&".cache".to_string()), "Should ignore .cache");
        assert!(ignore.contains(&".npm".to_string()), "Should ignore .npm");
        assert!(ignore.contains(&".cargo".to_string()), "Should ignore .cargo");
    }

    // ─── count_ai_todos: Phase 2 markers ─────────────────────────────────────

    #[test]
    fn count_ai_todos_with_ask_user_markers() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-ask-user");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        std::fs::write(
            ai_dir.join("glossary.md"),
            "# Glossary\n| Widget | some entity <!-- TODO: ask user --> | |\n| Known | definition | |\n",
        ).unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_ai_todos_in_tech_debt_subdir() {
        let tmp = std::env::temp_dir().join("kronn-test-todos-techdebt");
        let td_dir = tmp.join("ai/tech-debt");
        let _ = std::fs::create_dir_all(&td_dir);
        std::fs::write(
            td_dir.join("TD-20260313-old-php.md"),
            "# TD\n<!-- TODO: verify -->\nSome content\n",
        ).unwrap();
        std::fs::write(
            tmp.join("ai").join("index.md"),
            "# Project\nClean content\n",
        ).unwrap();
        assert_eq!(count_ai_todos(&tmp.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── detect_audit_status: ai/ dir exists but no index.md ─────────────────

    #[test]
    fn detect_audit_status_ai_dir_no_index() {
        let tmp = std::env::temp_dir().join("kronn-test-audit-no-index");
        let ai_dir = tmp.join("ai");
        let _ = std::fs::create_dir_all(&ai_dir);
        // ai/ dir exists but no index.md → TemplateInstalled
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::TemplateInstalled));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
