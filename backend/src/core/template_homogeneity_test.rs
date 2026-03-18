//! Regression tests verifying that all instruction/redirector template files
//! remain structurally homogeneous. These tests must pass after every change
//! to the templates/ directory.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// Returns the absolute path to the templates/ directory at the repo root.
    fn templates_dir() -> PathBuf {
        // CARGO_MANIFEST_DIR is the backend/ directory during tests.
        let manifest = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during tests");
        PathBuf::from(manifest).join("../templates")
    }

    /// All 10 instruction/redirector template files, relative to templates/.
    const INSTRUCTION_FILES: &[&str] = &[
        "CLAUDE.md",
        "GEMINI.md",
        "AGENTS.md",
        ".kiro/steering/instructions.md",
        ".vibe/instructions.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".github/copilot-instructions.md",
        ".cursor/rules/repo-instructions.mdc",
    ];

    /// Strips YAML frontmatter (the `--- ... ---` block) from a file's content.
    /// Only the `.mdc` file has frontmatter; for all other files this is a no-op.
    fn strip_frontmatter(content: &str) -> &str {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return content;
        }
        // Find the closing `---` after the opening one.
        let after_open = &trimmed[3..];
        if let Some(close_pos) = after_open.find("\n---") {
            // Skip past the closing `---\n`
            let after_close = &after_open[close_pos + 4..];
            // Trim the leading newline that follows the closing `---`
            after_close.trim_start_matches('\n')
        } else {
            content
        }
    }

    // ─── Test 1: All instruction files exist ─────────────────────────────────

    #[test]
    fn all_instruction_files_exist() {
        let base = templates_dir();
        let mut missing: Vec<&str> = Vec::new();
        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            if !full_path.exists() {
                missing.push(relative_path);
            }
        }
        assert!(
            missing.is_empty(),
            "Missing instruction template files: {:#?}\n\
             (looked in: {})",
            missing,
            base.display()
        );
    }

    // ─── Test 2: All instruction files contain required content ──────────────

    #[test]
    fn all_instruction_files_contain_required_sections() {
        let base = templates_dir();

        /// Strings that every instruction file must contain (after stripping frontmatter).
        const REQUIRED: &[&str] = &[
            "{{PROJECT_NAME}}",
            "{{STACK_SUMMARY}}",
            "{{PROJECT_LANGUAGE}}",
            "## Critical rules",
            "{{DO_NOT_1}}",
            "{{DO_NOT_2}}",
            "DO NOT guess",
            "DO NOT edit auto-generated",
            "DO NOT skip tests",
            "ai/repo-map.md",
            "ai/coding-rules.md",
            "ai/index.md",
        ];

        let mut failures: Vec<String> = Vec::new();

        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            let raw = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{}: could not read file — {}", relative_path, e));
                    continue;
                }
            };
            let content = strip_frontmatter(&raw);

            for &required in REQUIRED {
                if !content.contains(required) {
                    failures.push(format!(
                        "{}: missing required string {:?}",
                        relative_path, required
                    ));
                }
            }
        }

        assert!(
            failures.is_empty(),
            "Instruction template files are missing required content:\n{}",
            failures.join("\n")
        );
    }

    // ─── Test 3: DO NOT rules order — project-specific rules come first ───────

    #[test]
    fn do_not_project_rules_appear_before_generic_rules() {
        let base = templates_dir();

        /// Generic DO NOT patterns that must appear AFTER `{{DO_NOT_1}}` and `{{DO_NOT_2}}`.
        const GENERIC_DO_NOT: &[&str] = &[
            "DO NOT guess",
            "DO NOT edit auto-generated",
            "DO NOT skip tests",
        ];

        let mut failures: Vec<String> = Vec::new();

        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            let raw = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{}: could not read file — {}", relative_path, e));
                    continue;
                }
            };
            let content = strip_frontmatter(&raw);

            // Find the position of the last project-specific placeholder.
            let pos_do_not_1 = content.find("{{DO_NOT_1}}");
            let pos_do_not_2 = content.find("{{DO_NOT_2}}");

            let project_rules_end = match (pos_do_not_1, pos_do_not_2) {
                (Some(p1), Some(p2)) => p1.max(p2),
                (Some(p), None) | (None, Some(p)) => p,
                (None, None) => {
                    // Already caught by Test 2; skip here to avoid double-reporting.
                    continue;
                }
            };

            for &generic in GENERIC_DO_NOT {
                if let Some(pos_generic) = content.find(generic) {
                    if pos_generic < project_rules_end {
                        failures.push(format!(
                            "{}: {:?} appears at byte {} which is BEFORE the last project-specific \
                             placeholder ({{{{DO_NOT_1}}}}/{{{{DO_NOT_2}}}}) at byte {}",
                            relative_path, generic, pos_generic, project_rules_end
                        ));
                    }
                }
            }
        }

        assert!(
            failures.is_empty(),
            "Project-specific DO NOT rules must appear before generic rules:\n{}",
            failures.join("\n")
        );
    }

    // ─── Test 4: Section headers are in the same order across all files ───────

    #[test]
    fn section_headers_are_in_consistent_order_across_all_files() {
        let base = templates_dir();

        /// Extract lines starting with `##` (level-2 headers) from content.
        fn extract_h2_headers(content: &str) -> Vec<String> {
            content
                .lines()
                .filter(|l| l.starts_with("## "))
                .map(|l| l.to_string())
                .collect()
        }

        let mut all_headers: Vec<(&str, Vec<String>)> = Vec::new();

        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            let raw = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => {
                    // Already caught by Test 1; skip gracefully here.
                    continue;
                }
            };
            let content = strip_frontmatter(&raw).to_string();
            let headers = extract_h2_headers(&content);
            all_headers.push((relative_path, headers));
        }

        if all_headers.is_empty() {
            return;
        }

        // Use the first file as the reference order.
        let (reference_file, reference_headers) = &all_headers[0];

        let mut failures: Vec<String> = Vec::new();

        for (file, headers) in &all_headers[1..] {
            if headers != reference_headers {
                failures.push(format!(
                    "{} has section headers {:?}\n  but {} (reference) has {:?}",
                    file, headers, reference_file, reference_headers
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "Section headers differ across instruction template files:\n{}",
            failures.join("\n\n")
        );
    }

    // ─── Test 5: Quick Facts block present in all files ──────────────────────

    #[test]
    fn all_instruction_files_contain_quick_facts_block() {
        let base = templates_dir();
        let mut failures: Vec<String> = Vec::new();

        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            let raw = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let content = strip_frontmatter(&raw);

            if !content.contains("<!-- KRONN:FACTS") {
                failures.push(format!("{}: missing <!-- KRONN:FACTS block", relative_path));
            }
            if !content.contains("<!-- END KRONN:FACTS -->") {
                failures.push(format!("{}: missing <!-- END KRONN:FACTS --> closing", relative_path));
            }
            if !content.contains("{{TEST_CMD}}") {
                failures.push(format!("{}: missing {{{{TEST_CMD}}}} placeholder", relative_path));
            }
            if !content.contains("{{LINT_CMD}}") {
                failures.push(format!("{}: missing {{{{LINT_CMD}}}} placeholder", relative_path));
            }
        }

        assert!(
            failures.is_empty(),
            "Quick Facts block issues:\n{}",
            failures.join("\n")
        );
    }

    // ─── Test 6: decisions.md template exists ────────────────────────────────

    #[test]
    fn decisions_template_exists_and_has_required_structure() {
        let base = templates_dir();
        let path = base.join("ai/decisions.md");
        assert!(path.exists(), "templates/ai/decisions.md must exist");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Architecture decisions"), "decisions.md must have 'Architecture decisions' header");
        assert!(content.contains("What NOT to do"), "decisions.md must have 'What NOT to do' column");
    }
}
