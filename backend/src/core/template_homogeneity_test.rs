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

        /// Strings that every instruction file must contain (after stripping
        /// frontmatter). KT-841 — redirectors are now a PURE pointer to
        /// `docs/AGENTS.md`: the project header stays (name/stack),
        /// but there is no local "## Critical rules" section any more —
        /// that content lives ONLY in `docs/AGENTS.md` (single source of
        /// truth), reached via `## More context`.
        const REQUIRED: &[&str] = &[
            "{{PROJECT_NAME}}",
            "{{STACK_SUMMARY}}",
            "Working language: see `docs/AGENTS.md`.",
            "## More context",
            "docs/AGENTS.md",
        ];

        /// KT-841 — strings that must NEVER reappear in a redirector: they
        /// were the "two sources of truth" bug (DO_NOT_1/2 + a local
        /// critical-rules section duplicating `docs/AGENTS.md`).
        const FORBIDDEN: &[&str] = &[
            "## Critical rules",
            "{{DO_NOT_1}}",
            "{{DO_NOT_2}}",
            "{{PROJECT_LANGUAGE}}",
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

            for &required in REQUIRED {
                if !content.contains(required) {
                    failures.push(format!(
                        "{}: missing required string {:?}",
                        relative_path, required
                    ));
                }
            }
            for &forbidden in FORBIDDEN {
                if content.contains(forbidden) {
                    failures.push(format!(
                        "{}: must NOT duplicate {:?} — that rule lives only in docs/AGENTS.md",
                        relative_path, forbidden
                    ));
                }
            }
        }

        assert!(
            failures.is_empty(),
            "Instruction template files violate the pure-redirect contract:\n{}",
            failures.join("\n")
        );
    }

    // ─── Test 3: no DO NOT rule is duplicated in a redirector ─────────────────

    #[test]
    fn do_not_project_rules_appear_before_generic_rules() {
        // KT-841 — redirectors used to carry BOTH `{{DO_NOT_1}}`/`{{DO_NOT_2}}`
        // (project-specific) and a fixed set of generic "DO NOT" bullets,
        // ordered so the project-specific ones came first. The pure-redirect
        // contract removed ALL of them from every adapter file — those rules
        // now live exclusively in `docs/AGENTS.md`. This test now asserts
        // that absence directly, instead of an ordering that no longer
        // applies (there is nothing left to order).
        let base = templates_dir();

        const DO_NOT_MARKERS: &[&str] = &[
            "{{DO_NOT_1}}",
            "{{DO_NOT_2}}",
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

            for &marker in DO_NOT_MARKERS {
                if content.contains(marker) {
                    failures.push(format!(
                        "{}: still carries a DO NOT rule ({:?}) — must be redirect-only, \
                         the rule belongs in docs/AGENTS.md",
                        relative_path, marker
                    ));
                }
            }
        }

        assert!(
            failures.is_empty(),
            "Redirector files must carry NO DO NOT rule of their own:\n{}",
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

    // ─── Test 5: no Quick Facts block is duplicated, redirect line is homogeneous ──

    #[test]
    fn all_instruction_files_contain_quick_facts_block() {
        // KT-841 — inverted contract: the `<!-- KRONN:FACTS -->` block
        // (Test:/Lint: commands) used to be copy-rendered into every
        // adapter file from filesystem heuristics, independently of
        // `docs/AGENTS.md` (the audit-verified source) — two sources of
        // truth that could silently drift apart (the historical `Lint:
        // phpcs` false positive). Every redirector must now be free of it,
        // AND the redirect sentence itself must be IDENTICAL across every
        // adapter (homogeneity) so there is exactly one place a human reads
        // to know where the real facts live.
        let base = templates_dir();
        let mut failures: Vec<String> = Vec::new();

        const FACTS_MARKERS: &[&str] = &[
            "<!-- KRONN:FACTS",
            "<!-- END KRONN:FACTS -->",
            "{{TEST_CMD}}",
            "{{LINT_CMD}}",
        ];

        /// First line right after `## More context` — the actual redirect
        /// sentence. AGENTS.md (the always-installed shared entry point)
        /// legitimately carries one extra follow-up line the vendor
        /// adapters don't, so only this first line is compared for
        /// homogeneity, not the whole section.
        fn redirect_sentence(content: &str) -> Option<&str> {
            let (_, after) = content.split_once("## More context")?;
            after.trim_start_matches('\n').lines().next()
        }

        let mut reference: Option<(&str, String)> = None;

        for &relative_path in INSTRUCTION_FILES {
            let full_path = base.join(relative_path);
            let raw = match std::fs::read_to_string(&full_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let content = strip_frontmatter(&raw);

            for &marker in FACTS_MARKERS {
                if content.contains(marker) {
                    failures.push(format!(
                        "{}: must NOT duplicate the Quick Facts block ({:?}) — \
                         docs/AGENTS.md is the single source of truth for test/lint commands",
                        relative_path, marker
                    ));
                }
            }

            match redirect_sentence(content).map(str::to_string) {
                None => failures.push(format!(
                    "{}: missing a redirect sentence under '## More context'",
                    relative_path
                )),
                Some(sentence) => match &reference {
                    None => reference = Some((relative_path, sentence)),
                    Some((ref_file, ref_sentence)) if *ref_sentence != sentence => {
                        failures.push(format!(
                            "{}: redirect sentence differs from {} (reference)\n  {}: {:?}\n  {}: {:?}",
                            relative_path, ref_file, ref_file, ref_sentence, relative_path, sentence
                        ));
                    }
                    Some(_) => {}
                },
            }
        }

        assert!(
            failures.is_empty(),
            "Redirector files violate the pure-redirect / no-duplicate-facts contract:\n{}",
            failures.join("\n")
        );
    }

    // ─── Test 6: decisions.md template exists ────────────────────────────────

    #[test]
    fn decisions_template_exists_and_has_required_structure() {
        // 0.7.1 pivot — templates/ai/ → templates/docs/
        let base = templates_dir();
        let path = base.join("docs/decisions.md");
        assert!(path.exists(), "templates/docs/decisions.md must exist");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("Architecture decisions"),
            "decisions.md must have 'Architecture decisions' header"
        );
        assert!(
            content.contains("What NOT to do"),
            "decisions.md must have 'What NOT to do' column"
        );
    }
}
