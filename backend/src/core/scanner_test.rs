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
        std::fs::write(ai_dir.join("index.md"), "# Project\nKRONN:BOOTSTRAP\n").unwrap();
        let status = detect_audit_status(&tmp.to_string_lossy());
        assert!(matches!(status, crate::models::AiAuditStatus::TemplateInstalled));
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
}
