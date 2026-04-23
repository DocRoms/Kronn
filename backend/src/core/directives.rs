//! Directives loader — reads builtin (embedded) and custom directives from disk.
//!
//! Directives represent HOW the agent should behave/format output (multi-select).
//! Categories: Output (builtin), Language (custom only — language is managed via config).
//!
//! Builtin directives are embedded at compile time from `src/directives/*.md`.
//! Custom directives live in `~/.config/kronn/directives/` as Markdown files with YAML frontmatter.

use std::path::PathBuf;

use crate::models::{Directive, DirectiveCategory};

// ─── Builtin directives (embedded at compile time) ──────────────────────────

struct BuiltinDirective {
    id: &'static str,
    content: &'static str,
}

const BUILTIN_DIRECTIVES: &[BuiltinDirective] = &[
    // Output
    BuiltinDirective { id: "token-saver", content: include_str!("../directives/token-saver.md") },
    BuiltinDirective { id: "json-output", content: include_str!("../directives/json-output.md") },
    BuiltinDirective { id: "code-only", content: include_str!("../directives/code-only.md") },
    BuiltinDirective { id: "markdown-report", content: include_str!("../directives/markdown-report.md") },
    BuiltinDirective { id: "step-by-step", content: include_str!("../directives/step-by-step.md") },
    BuiltinDirective { id: "verbose", content: include_str!("../directives/verbose.md") },
    BuiltinDirective { id: "diff-only", content: include_str!("../directives/diff-only.md") },
    BuiltinDirective { id: "caveman", content: include_str!("../directives/caveman.md") },
];

// ─── Frontmatter parsing ────────────────────────────────────────────────────

fn parse_directive_markdown(id: &str, raw: &str, is_builtin: bool) -> Option<Directive> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        tracing::warn!("Directive '{}' missing YAML frontmatter", id);
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 4..].trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut icon = String::new();
    let mut category = DirectiveCategory::Output;
    let mut conflicts = Vec::new();
    let mut source_url: Option<String> = None;

    for line in yaml_str.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("icon:") {
            icon = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("category:") {
            category = match val.trim() {
                "output" => DirectiveCategory::Output,
                "language" => DirectiveCategory::Language,
                _ => DirectiveCategory::Output,
            };
        } else if let Some(val) = line.strip_prefix("conflicts:") {
            let val = val.trim();
            if val != "[]" && !val.is_empty() {
                let inner = val.trim_start_matches('[').trim_end_matches(']');
                conflicts = inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
        } else if let Some(val) = line.strip_prefix("source_url:") {
            let val = val.trim();
            if !val.is_empty() {
                source_url = Some(val.to_string());
            }
        }
    }

    if name.is_empty() {
        tracing::warn!("Directive '{}' has no name in frontmatter", id);
        return None;
    }

    let token_estimate = ((body.len() + name.len() + 20) / 4) as u32;

    Some(Directive {
        id: id.to_string(),
        name,
        description,
        icon,
        category,
        content: body,
        is_builtin,
        conflicts,
        token_estimate,
        source_url,
    })
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Custom directives directory: ~/.config/kronn/directives/
fn custom_directives_dir() -> Option<PathBuf> {
    let config_dir = crate::core::config::config_dir().ok()?;
    Some(config_dir.join("directives"))
}

/// List all available directives (builtin + custom).
pub fn list_all_directives() -> Vec<Directive> {
    let mut directives = Vec::new();

    for builtin in BUILTIN_DIRECTIVES {
        if let Some(directive) = parse_directive_markdown(builtin.id, builtin.content, true) {
            directives.push(directive);
        }
    }

    if let Some(dir) = custom_directives_dir() {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("md") {
                        let id = format!("custom-{}", path.file_stem().unwrap_or_default().to_string_lossy());
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Some(directive) = parse_directive_markdown(&id, &content, false) {
                                directives.push(directive);
                            }
                        }
                    }
                }
            }
        }
    }

    directives
}

/// Get a single directive by ID.
pub fn get_directive(id: &str) -> Option<Directive> {
    list_all_directives().into_iter().find(|d| d.id == id)
}

/// Get directives by their IDs, preserving order. Skips unknown IDs.
pub fn get_directives_by_ids(ids: &[String]) -> Vec<Directive> {
    let all = list_all_directives();
    ids.iter()
        .filter_map(|id| all.iter().find(|d| d.id == *id).cloned())
        .collect()
}

/// Build the combined directive prompt text for injection.
/// Returns empty string if no directives are selected.
pub fn build_directives_prompt(directive_ids: &[String]) -> String {
    let directives = get_directives_by_ids(directive_ids);
    if directives.is_empty() {
        return String::new();
    }

    let mut prompt = String::from("=== Active Directives ===\n\n");
    for directive in &directives {
        prompt.push_str(&format!("--- {} ---\n{}\n\n", directive.name, directive.content));
    }
    prompt
}

/// Validate that selected directives don't conflict with each other.
/// Returns a list of conflict pairs if any are found.
pub fn validate_no_conflicts(directive_ids: &[String]) -> Vec<(String, String)> {
    let directives = get_directives_by_ids(directive_ids);
    let mut conflicts = Vec::new();

    for d in &directives {
        for conflict_id in &d.conflicts {
            if directive_ids.contains(conflict_id) {
                let pair = if d.id < *conflict_id {
                    (d.id.clone(), conflict_id.clone())
                } else {
                    (conflict_id.clone(), d.id.clone())
                };
                if !conflicts.contains(&pair) {
                    conflicts.push(pair);
                }
            }
        }
    }

    conflicts
}

/// Save a custom directive to disk. Returns the generated ID.
pub fn save_custom_directive(
    name: &str,
    description: &str,
    icon: &str,
    category: &DirectiveCategory,
    content: &str,
    conflicts: &[String],
) -> Result<String, String> {
    let dir = custom_directives_dir().ok_or("Cannot determine config directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create directives dir: {}", e))?;

    let slug: String = name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let id = format!("custom-{}", slug);
    let cat_str = match category {
        DirectiveCategory::Output => "output",
        DirectiveCategory::Language => "language",
    };

    let conflicts_str = if conflicts.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", conflicts.join(", "))
    };

    let desc_line = if description.is_empty() { String::new() } else { format!("description: {}\n", description) };
    let file_content = format!(
        "---\nname: {}\n{}category: {}\nicon: {}\nbuiltin: false\nconflicts: {}\n---\n{}",
        name, desc_line, cat_str, icon, conflicts_str, content
    );

    let path = dir.join(format!("{}.md", slug));
    std::fs::write(&path, file_content).map_err(|e| format!("Cannot write directive: {}", e))?;

    Ok(id)
}

/// Delete a custom directive from disk.
pub fn delete_custom_directive(id: &str) -> Result<bool, String> {
    if !id.starts_with("custom-") {
        return Err("Cannot delete builtin directives".into());
    }
    let slug = id.strip_prefix("custom-").unwrap();
    let dir = custom_directives_dir().ok_or("Cannot determine config directory")?;
    let path = dir.join(format!("{}.md", slug));

    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Cannot delete directive: {}", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_builtin_directives() {
        let directives = list_all_directives();
        assert!(directives.len() >= 8, "Expected at least 8 builtin directives, got {}", directives.len());

        let token_saver = directives.iter().find(|d| d.id == "token-saver").unwrap();
        assert_eq!(token_saver.name, "Token Saver");
        assert_eq!(token_saver.icon, "⚡");
        assert_eq!(token_saver.category, DirectiveCategory::Output);
        assert!(token_saver.is_builtin);
        assert!(!token_saver.content.is_empty());
        assert!(token_saver.conflicts.contains(&"verbose".to_string()));
        // Token Saver has no source_url — it's a Kronn-native directive.
        assert!(token_saver.source_url.is_none());
    }

    #[test]
    fn caveman_directive_carries_source_url_and_attributes_upstream() {
        // Caveman is adapted from a third-party project (MIT). The UI
        // surfaces `source_url` as a clickable "↗ Source" link so the
        // user can read the original. Removing the attribution without
        // the project's consent would break license terms.
        let d = get_directive("caveman").expect("caveman directive missing");
        assert_eq!(d.name, "Caveman");
        assert_eq!(
            d.source_url.as_deref(),
            Some("https://github.com/JuliusBrussee/caveman"),
        );
        // Telegraphic style conflicts with Verbose by design.
        assert!(d.conflicts.iter().any(|c| c == "verbose"));
    }

    #[test]
    fn all_builtins_are_marked_builtin() {
        let directives = list_all_directives();
        for d in &directives {
            if !d.id.starts_with("custom-") {
                assert!(d.is_builtin, "Directive '{}' should be builtin", d.id);
            }
        }
    }

    #[test]
    fn all_builtins_have_required_fields() {
        let directives = list_all_directives();
        for d in &directives {
            assert!(!d.name.is_empty(), "Directive '{}' has empty name", d.id);
            assert!(!d.icon.is_empty(), "Directive '{}' has empty icon", d.id);
            assert!(!d.content.is_empty(), "Directive '{}' has empty content", d.id);
        }
    }

    #[test]
    fn builtin_ids_are_unique() {
        let directives = list_all_directives();
        let mut ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        let count_before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count_before, "Duplicate directive IDs found");
    }

    #[test]
    fn output_category_represented() {
        let directives = list_all_directives();
        assert!(directives.iter().any(|d| d.category == DirectiveCategory::Output), "No output directives");
    }

    #[test]
    fn language_directives_removed() {
        let directives = list_all_directives();
        let ids: Vec<&str> = directives.iter().map(|d| d.id.as_str()).collect();
        assert!(!ids.contains(&"repondre-en-francais"), "French language directive should be removed");
        assert!(!ids.contains(&"reply-in-english"), "English language directive should be removed");
        assert!(!ids.contains(&"responder-en-espanol"), "Spanish language directive should be removed");
        // No builtin language directives should remain
        assert!(!directives.iter().any(|d| d.category == DirectiveCategory::Language && d.is_builtin),
            "No builtin language directives should exist");
    }

    #[test]
    fn get_directive_found() {
        let d = get_directive("token-saver");
        assert!(d.is_some());
        assert_eq!(d.unwrap().name, "Token Saver");
    }

    #[test]
    fn get_directive_not_found() {
        let d = get_directive("nonexistent-directive");
        assert!(d.is_none());
    }

    #[test]
    fn get_directives_by_ids_preserves_order() {
        let ds = get_directives_by_ids(&["verbose".into(), "token-saver".into()]);
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].id, "verbose");
        assert_eq!(ds[1].id, "token-saver");
    }

    #[test]
    fn get_directives_by_ids_skips_unknown() {
        let ds = get_directives_by_ids(&["token-saver".into(), "nope".into(), "verbose".into()]);
        assert_eq!(ds.len(), 2);
    }

    #[test]
    fn build_directives_prompt_empty() {
        let prompt = build_directives_prompt(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_directives_prompt_with_ids() {
        let prompt = build_directives_prompt(&["token-saver".into(), "json-output".into()]);
        assert!(prompt.contains("Token Saver"));
        assert!(prompt.contains("JSON Output"));
        assert!(prompt.contains("=== Active Directives ==="));
    }

    #[test]
    fn validate_no_conflicts_clean() {
        let conflicts = validate_no_conflicts(&["token-saver".into(), "json-output".into()]);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn validate_no_conflicts_detects_conflict() {
        let conflicts = validate_no_conflicts(&["token-saver".into(), "verbose".into()]);
        assert!(!conflicts.is_empty(), "token-saver and verbose should conflict");
    }


    #[test]
    fn parse_frontmatter_valid() {
        let raw = "---\nname: Test\nicon: ⭐\ncategory: output\nbuiltin: false\nconflicts: []\n---\nDo things.";
        let d = parse_directive_markdown("test", raw, false).unwrap();
        assert_eq!(d.name, "Test");
        assert_eq!(d.icon, "⭐");
        assert_eq!(d.category, DirectiveCategory::Output);
        assert_eq!(d.content, "Do things.");
        assert!(d.conflicts.is_empty());
    }

    #[test]
    fn parse_frontmatter_language_category() {
        let raw = "---\nname: X\nicon: I\ncategory: language\nbuiltin: true\nconflicts: []\n---\nc";
        let d = parse_directive_markdown("x", raw, true).unwrap();
        assert_eq!(d.category, DirectiveCategory::Language);
    }

    #[test]
    fn parse_frontmatter_with_conflicts() {
        let raw = "---\nname: X\nicon: I\ncategory: output\nbuiltin: false\nconflicts: [a, b]\n---\nc";
        let d = parse_directive_markdown("x", raw, false).unwrap();
        assert_eq!(d.conflicts, vec!["a", "b"]);
    }

    #[test]
    fn parse_frontmatter_missing_yields_none() {
        assert!(parse_directive_markdown("bad", "No frontmatter", false).is_none());
    }

    #[test]
    fn parse_frontmatter_no_name_yields_none() {
        let raw = "---\nicon: I\ncategory: output\nconflicts: []\n---\nc";
        assert!(parse_directive_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn delete_builtin_directive_rejected() {
        let result = delete_custom_directive("token-saver");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("builtin"));
    }
}
