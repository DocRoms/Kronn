//! Skills loader — reads builtin (embedded) and custom skills from disk.
//!
//! Skills represent WHAT domain expertise the agent has (multi-select).
//! Categories: Language, Domain, Business.
//!
//! Builtin skills are embedded at compile time from `src/skills/*.md`.
//! Custom skills live in `~/.config/kronn/skills/` as Markdown files with YAML frontmatter.

use std::path::PathBuf;

use crate::models::{Skill, SkillCategory};

// ─── Builtin skills (embedded at compile time) ──────────────────────────────

struct BuiltinSkill {
    id: &'static str,
    content: &'static str,
}

const BUILTIN_SKILLS: &[BuiltinSkill] = &[
    // Language
    BuiltinSkill { id: "rust", content: include_str!("../skills/rust.md") },
    BuiltinSkill { id: "typescript", content: include_str!("../skills/typescript.md") },
    BuiltinSkill { id: "python", content: include_str!("../skills/python.md") },
    BuiltinSkill { id: "go", content: include_str!("../skills/go.md") },
    BuiltinSkill { id: "php", content: include_str!("../skills/php.md") },
    BuiltinSkill { id: "java", content: include_str!("../skills/java.md") },
    BuiltinSkill { id: "kotlin", content: include_str!("../skills/kotlin.md") },
    BuiltinSkill { id: "swift", content: include_str!("../skills/swift.md") },
    BuiltinSkill { id: "csharp", content: include_str!("../skills/csharp.md") },
    // Domain
    BuiltinSkill { id: "security", content: include_str!("../skills/security.md") },
    BuiltinSkill { id: "devops", content: include_str!("../skills/devops.md") },
    BuiltinSkill { id: "data-engineering", content: include_str!("../skills/data-engineering.md") },
    BuiltinSkill { id: "database", content: include_str!("../skills/database.md") },
    BuiltinSkill { id: "terraform", content: include_str!("../skills/terraform.md") },
    BuiltinSkill { id: "testing", content: include_str!("../skills/testing.md") },
    BuiltinSkill { id: "api-design", content: include_str!("../skills/api-design.md") },
    BuiltinSkill { id: "mobile", content: include_str!("../skills/mobile.md") },
    BuiltinSkill { id: "workflow-architect", content: include_str!("../skills/workflow-architect.md") },
    BuiltinSkill { id: "bootstrap-architect", content: include_str!("../skills/bootstrap-architect.md") },
    BuiltinSkill { id: "kronn-docs", content: include_str!("../skills/kronn-docs.md") },
    // Business
    BuiltinSkill { id: "seo", content: include_str!("../skills/seo.md") },
    BuiltinSkill { id: "web-performance", content: include_str!("../skills/web-performance.md") },
    BuiltinSkill { id: "green-it", content: include_str!("../skills/green-it.md") },
    BuiltinSkill { id: "accessibility", content: include_str!("../skills/accessibility.md") },
    BuiltinSkill { id: "gdpr", content: include_str!("../skills/gdpr.md") },
    // Meta
    BuiltinSkill { id: "structured-questions", content: include_str!("../skills/structured-questions.md") },
    // External skills (vendored from third-party MIT-licensed projects).
    // See THIRD_PARTY_SKILLS.md at repo root for sources, licenses, commit
    // hashes, and update process. Each skill's frontmatter includes
    // `external: true` + `source_url` for in-app attribution.
    BuiltinSkill { id: "test-driven-development", content: include_str!("../skills/external/test-driven-development.md") },
    BuiltinSkill { id: "systematic-debugging", content: include_str!("../skills/external/systematic-debugging.md") },
    BuiltinSkill { id: "writing-plans", content: include_str!("../skills/external/writing-plans.md") },
    BuiltinSkill { id: "brainstorming", content: include_str!("../skills/external/brainstorming.md") },
    BuiltinSkill { id: "verification-before-completion", content: include_str!("../skills/external/verification-before-completion.md") },
    BuiltinSkill { id: "requesting-code-review", content: include_str!("../skills/external/requesting-code-review.md") },
    BuiltinSkill { id: "receiving-code-review", content: include_str!("../skills/external/receiving-code-review.md") },
    BuiltinSkill { id: "finishing-a-development-branch", content: include_str!("../skills/external/finishing-a-development-branch.md") },
];

// ─── Frontmatter parsing ────────────────────────────────────────────────────

fn parse_skill_markdown(id: &str, raw: &str, is_builtin: bool) -> Option<Skill> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        tracing::warn!("Skill '{}' missing YAML frontmatter", id);
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 4..].trim().to_string();

    let mut name = String::new();
    let mut description = String::new();
    let mut icon = String::new();
    let mut category = SkillCategory::Domain;
    let mut license: Option<String> = None;
    let mut allowed_tools: Option<String> = None;
    let mut external = false;
    let mut source_url: Option<String> = None;
    let auto_triggers = parse_auto_triggers_block(yaml_str);

    for line in yaml_str.lines() {
        // Skip lines that are nested under a known block — line-based
        // scalar extraction shouldn't accidentally pick up e.g. the
        // `name:` inside an `auto_triggers:` sub-map.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("icon:") {
            icon = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("category:") {
            category = match val.trim() {
                "language" => SkillCategory::Language,
                "domain" => SkillCategory::Domain,
                "business" => SkillCategory::Business,
                _ => SkillCategory::Domain,
            };
        } else if let Some(val) = line.strip_prefix("license:") {
            let v = val.trim().to_string();
            if !v.is_empty() { license = Some(v); }
        } else if let Some(val) = line.strip_prefix("allowed-tools:") {
            let v = val.trim().to_string();
            if !v.is_empty() { allowed_tools = Some(v); }
        } else if let Some(val) = line.strip_prefix("external:") {
            external = matches!(val.trim(), "true" | "yes" | "1");
        } else if let Some(val) = line.strip_prefix("source_url:") {
            let v = val.trim().to_string();
            if !v.is_empty() { source_url = Some(v); }
        }
    }

    if name.is_empty() {
        tracing::warn!("Skill '{}' has no name in frontmatter", id);
        return None;
    }

    // Estimate token cost: ~4 chars per token, including framing overhead
    let token_estimate = ((body.len() + name.len() + 20) / 4) as u32;

    Some(Skill {
        id: id.to_string(),
        name,
        description,
        icon,
        category,
        content: body,
        is_builtin,
        token_estimate,
        license,
        allowed_tools,
        auto_triggers,
        external,
        source_url,
    })
}

/// Extract the optional `auto_triggers:` YAML sub-map from a frontmatter
/// string. Avoids pulling in serde_yaml for this one block — a minimal
/// indentation-based state machine is enough since frontmatter is tiny
/// and we control the convention (see `kronn-docs.md`).
///
/// Shape understood:
///
/// ```yaml
/// auto_triggers:
///   common:
///     - "pattern1"
///     - "pattern2"
///   fr:
///     - "génér.+(rapport|fichier)"
///   en:
///     - "generate.+(report|file)"
/// ```
fn parse_auto_triggers_block(yaml: &str) -> Option<crate::models::AutoTriggers> {
    let mut in_block = false;
    let mut current_bucket: Option<String> = None;
    let mut common: Vec<String> = Vec::new();
    let mut locales: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for line in yaml.lines() {
        // Track entry/exit of the `auto_triggers:` block by indentation.
        if !in_block {
            if line.trim_start().starts_with("auto_triggers:")
                && line.chars().take_while(|c| c.is_whitespace()).count() == 0
            {
                in_block = true;
            }
            continue;
        }
        // A non-indented line means we left the block.
        if !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
        {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        // "- pattern" → value for the current bucket.
        if let Some(rest) = trimmed.strip_prefix("- ") {
            let value = strip_yaml_scalar(rest);
            match current_bucket.as_deref() {
                Some("common") => common.push(value),
                Some(key) => locales.entry(key.to_string()).or_default().push(value),
                None => tracing::warn!("auto_triggers: list item without parent key: {trimmed}"),
            }
            continue;
        }
        // "bucket:" → start a new bucket (common / fr / en / es / ...).
        if let Some(key) = trimmed.strip_suffix(':') {
            current_bucket = Some(key.to_string());
        }
    }

    if common.is_empty() && locales.is_empty() {
        None
    } else {
        Some(crate::models::AutoTriggers { common, locales })
    }
}

/// Unwrap a YAML scalar value: strip matching outer quotes and decode
/// the `\\` → `\` escape so regex patterns written as `"\\b"` in YAML
/// arrive as the literal 2-char sequence `\b` expected by JS `RegExp`.
fn strip_yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        // Conservative unescape: only `\\` → `\` and `\"` → `"`.
        // Anything else (`\n`, `\t`, `\u{...}`) is extremely unlikely in
        // a regex trigger list and not worth a full YAML parser.
        inner.replace("\\\\", "\\").replace("\\\"", "\"")
    } else if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Custom skills directory: ~/.config/kronn/skills/
fn custom_skills_dir() -> Option<PathBuf> {
    let config_dir = crate::core::config::config_dir().ok()?;
    Some(config_dir.join("skills"))
}

/// List all available skills (builtin + custom).
pub fn list_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();

    for builtin in BUILTIN_SKILLS {
        if let Some(skill) = parse_skill_markdown(builtin.id, builtin.content, true) {
            skills.push(skill);
        }
    }

    if let Some(dir) = custom_skills_dir() {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("md") {
                        let id = format!("custom-{}", path.file_stem().unwrap_or_default().to_string_lossy());
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Some(skill) = parse_skill_markdown(&id, &content, false) {
                                skills.push(skill);
                            }
                        }
                    }
                }
            }
        }
    }

    skills
}

/// Get a single skill by ID.
pub fn get_skill(id: &str) -> Option<Skill> {
    list_all_skills().into_iter().find(|s| s.id == id)
}

/// Get skills by their IDs, preserving order. Skips unknown IDs.
pub fn get_skills_by_ids(ids: &[String]) -> Vec<Skill> {
    let all = list_all_skills();
    ids.iter()
        .filter_map(|id| all.iter().find(|s| s.id == *id).cloned())
        .collect()
}

/// Build the combined skill prompt text for injection.
/// Returns empty string if no skills are selected.
pub fn build_skills_prompt(skill_ids: &[String]) -> String {
    let skills = get_skills_by_ids(skill_ids);
    if skills.is_empty() {
        return String::new();
    }

    let mut prompt = String::from("=== Active Skills ===\n\n");
    for skill in &skills {
        prompt.push_str(&format!("--- {} ---\n{}\n\n", skill.name, skill.content));
    }
    prompt
}

/// Build a compact skills prompt for agents with small context windows.
/// Uses the first 2-3 meaningful lines instead of just 1 (~40% token savings).
pub fn build_skills_prompt_compact(skill_ids: &[String]) -> String {
    let skills = get_skills_by_ids(skill_ids);
    if skills.is_empty() {
        return String::new();
    }

    let mut prompt = String::from("=== Skills ===\n");
    for skill in &skills {
        // Take first 2-3 meaningful lines (up to ~200 chars) for better context
        let mut summary = String::new();
        for line in skill.content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }
            if !summary.is_empty() { summary.push(' '); }
            summary.push_str(trimmed);
            if summary.len() > 150 { break; }
        }
        if summary.is_empty() { summary = skill.name.clone(); }
        prompt.push_str(&format!("[{}: {}]\n", skill.name, summary));
    }
    prompt
}

/// Save a custom skill to disk. Returns the generated ID.
pub fn save_custom_skill(
    name: &str, description: &str, icon: &str, category: &SkillCategory, content: &str,
    license: Option<&str>, allowed_tools: Option<&str>,
) -> Result<String, String> {
    // Validate per agentskills.io spec
    if name.is_empty() { return Err("Skill name is required".into()); }
    if description.len() > 1024 { return Err("Description must be at most 1024 characters".into()); }

    let dir = custom_skills_dir().ok_or("Cannot determine config directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create skills dir: {}", e))?;

    let slug = super::native_files::slug(name);
    if slug.is_empty() { return Err("Skill name must contain at least one alphanumeric character".into()); }

    let id = format!("custom-{}", slug);
    let cat_str = match category {
        SkillCategory::Language => "language",
        SkillCategory::Domain => "domain",
        SkillCategory::Business => "business",
    };

    let desc_line = if description.is_empty() { String::new() } else { format!("description: {}\n", description) };
    let license_line = license.filter(|s| !s.is_empty()).map(|s| format!("license: {}\n", s)).unwrap_or_default();
    let tools_line = allowed_tools.filter(|s| !s.is_empty()).map(|s| format!("allowed-tools: {}\n", s)).unwrap_or_default();
    let file_content = format!(
        "---\nname: {}\n{}category: {}\nicon: {}\n{}{}builtin: false\n---\n{}",
        name, desc_line, cat_str, icon, license_line, tools_line, content
    );

    let path = dir.join(format!("{}.md", slug));
    std::fs::write(&path, file_content).map_err(|e| format!("Cannot write skill: {}", e))?;

    Ok(id)
}

/// Delete a custom skill from disk.
pub fn delete_custom_skill(id: &str) -> Result<bool, String> {
    if !id.starts_with("custom-") {
        return Err("Cannot delete builtin skills".into());
    }
    let slug = id.strip_prefix("custom-").unwrap();
    let dir = custom_skills_dir().ok_or("Cannot determine config directory")?;
    let path = dir.join(format!("{}.md", slug));

    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Cannot delete skill: {}", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_builtin_skills() {
        let skills = list_all_skills();
        assert!(skills.len() >= 22, "Expected at least 22 builtin skills, got {}", skills.len());

        let rust = skills.iter().find(|s| s.id == "rust").unwrap();
        assert_eq!(rust.name, "rust");
        assert_eq!(rust.icon, "🦀");
        assert_eq!(rust.category, SkillCategory::Language);
        assert!(rust.is_builtin);
        assert!(!rust.content.is_empty());
    }

    #[test]
    fn all_builtins_are_marked_builtin() {
        let skills = list_all_skills();
        for skill in &skills {
            if !skill.id.starts_with("custom-") {
                assert!(skill.is_builtin, "Skill '{}' should be builtin", skill.id);
            }
        }
    }

    #[test]
    fn all_builtins_have_required_fields() {
        let skills = list_all_skills();
        for skill in &skills {
            assert!(!skill.name.is_empty(), "Skill '{}' has empty name", skill.id);
            assert!(!skill.icon.is_empty(), "Skill '{}' has empty icon", skill.id);
            assert!(!skill.content.is_empty(), "Skill '{}' has empty content", skill.id);
        }
    }

    #[test]
    fn builtin_ids_are_unique() {
        let skills = list_all_skills();
        let mut ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
        let count_before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count_before, "Duplicate skill IDs found");
    }

    #[test]
    fn all_three_categories_represented() {
        let skills = list_all_skills();
        assert!(skills.iter().any(|s| s.category == SkillCategory::Language), "No language skills");
        assert!(skills.iter().any(|s| s.category == SkillCategory::Domain), "No domain skills");
        assert!(skills.iter().any(|s| s.category == SkillCategory::Business), "No business skills");
    }

    #[test]
    fn get_skill_found() {
        let skill = get_skill("typescript");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().name, "typescript");
    }

    #[test]
    fn get_skill_not_found() {
        let skill = get_skill("nonexistent-skill");
        assert!(skill.is_none());
    }

    #[test]
    fn get_skills_by_ids_preserves_order() {
        let skills = get_skills_by_ids(&["python".into(), "rust".into()]);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].id, "python");
        assert_eq!(skills[1].id, "rust");
    }

    #[test]
    fn get_skills_by_ids_skips_unknown() {
        let skills = get_skills_by_ids(&["rust".into(), "nonexistent".into(), "typescript".into()]);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].id, "rust");
        assert_eq!(skills[1].id, "typescript");
    }

    #[test]
    fn get_skills_by_ids_empty_input() {
        let skills = get_skills_by_ids(&[]);
        assert!(skills.is_empty());
    }

    #[test]
    fn build_skills_prompt_empty() {
        let prompt = build_skills_prompt(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_with_ids() {
        let prompt = build_skills_prompt(&["rust".into(), "typescript".into()]);
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("typescript"));
        assert!(prompt.contains("=== Active Skills ==="));
    }

    #[test]
    fn build_skills_prompt_unknown_ids_ignored() {
        let prompt = build_skills_prompt(&["nonexistent-1".into(), "nonexistent-2".into()]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_single_skill() {
        let prompt = build_skills_prompt(&["rust".into()]);
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("=== Active Skills ==="));
    }

    #[test]
    fn new_language_skills_exist() {
        for id in ["java", "kotlin", "swift", "csharp"] {
            let skill = get_skill(id);
            assert!(skill.is_some(), "Language skill '{}' must exist", id);
            assert_eq!(skill.unwrap().category, SkillCategory::Language);
        }
    }

    #[test]
    fn new_domain_skills_exist() {
        for id in ["terraform", "testing", "api-design", "mobile"] {
            let skill = get_skill(id);
            assert!(skill.is_some(), "Domain skill '{}' must exist", id);
            assert_eq!(skill.unwrap().category, SkillCategory::Domain);
        }
    }

    #[test]
    fn parse_frontmatter_valid() {
        let raw = "---\nname: Test Skill\nicon: ⭐\ncategory: domain\nbuiltin: false\n---\nDo the thing.";
        let skill = parse_skill_markdown("test", raw, false).unwrap();
        assert_eq!(skill.name, "Test Skill");
        assert_eq!(skill.icon, "⭐");
        assert_eq!(skill.category, SkillCategory::Domain);
        assert_eq!(skill.content, "Do the thing.");
        assert!(!skill.is_builtin);
    }

    #[test]
    fn parse_frontmatter_language_category() {
        let raw = "---\nname: Go\nicon: 🐹\ncategory: language\nbuiltin: true\n---\ncontent";
        let skill = parse_skill_markdown("go", raw, true).unwrap();
        assert_eq!(skill.category, SkillCategory::Language);
        assert!(skill.is_builtin);
    }

    #[test]
    fn parse_frontmatter_business_category() {
        let raw = "---\nname: SEO\nicon: 🔎\ncategory: business\nbuiltin: true\n---\ncontent";
        let skill = parse_skill_markdown("seo", raw, true).unwrap();
        assert_eq!(skill.category, SkillCategory::Business);
    }

    #[test]
    fn parse_frontmatter_unknown_category_defaults_domain() {
        let raw = "---\nname: X\nicon: I\ncategory: Unknown\nbuiltin: false\n---\ncontent";
        let skill = parse_skill_markdown("x", raw, false).unwrap();
        assert_eq!(skill.category, SkillCategory::Domain);
    }

    #[test]
    fn parse_frontmatter_missing_yields_none() {
        let raw = "No frontmatter here, just content.";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_no_name_yields_none() {
        let raw = "---\nicon: I\ncategory: domain\nbuiltin: false\n---\ncontent";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_unclosed_yields_none() {
        let raw = "---\nname: X\nicon: I\ncategory: domain\ncontent without closing frontmatter";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn delete_builtin_skill_rejected() {
        let result = delete_custom_skill("rust");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("builtin"));
    }

    // ─── build_skills_prompt_compact ─────────────────────────────────────

    #[test]
    fn build_skills_prompt_compact_empty() {
        let prompt = build_skills_prompt_compact(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_compact_with_ids() {
        let prompt = build_skills_prompt_compact(&["rust".into(), "typescript".into()]);
        assert!(prompt.contains("=== Skills ==="));
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("typescript"));
    }

    #[test]
    fn build_skills_prompt_compact_unknown_ids_ignored() {
        let prompt = build_skills_prompt_compact(&["nonexistent-1".into()]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_compact_shorter_than_full() {
        let ids: Vec<String> = vec!["rust".into(), "typescript".into(), "python".into()];
        let full = build_skills_prompt(&ids);
        let compact = build_skills_prompt_compact(&ids);
        assert!(
            compact.len() < full.len(),
            "Compact prompt ({} bytes) should be shorter than full ({} bytes)",
            compact.len(),
            full.len()
        );
    }

    // ─── token_estimate ──────────────────────────────────────────────────

    #[test]
    fn token_estimate_is_positive_for_all_builtins() {
        let skills = list_all_skills();
        for skill in &skills {
            assert!(
                skill.token_estimate > 0,
                "Skill '{}' should have positive token estimate",
                skill.id
            );
        }
    }

    // ─── parse_skill_markdown edge cases ─────────────────────────────────

    #[test]
    fn parse_frontmatter_with_description() {
        let raw = "---\nname: Test\nicon: T\ndescription: A test skill\ncategory: domain\n---\ncontent";
        let skill = parse_skill_markdown("test", raw, false).unwrap();
        assert_eq!(skill.description, "A test skill");
    }

    #[test]
    fn parse_frontmatter_whitespace_before_start() {
        let raw = "  \n---\nname: Test\nicon: T\ncategory: domain\n---\ncontent";
        let skill = parse_skill_markdown("test", raw, false).unwrap();
        assert_eq!(skill.name, "Test");
    }

    #[test]
    fn parse_frontmatter_multiline_content() {
        let raw = "---\nname: Multi\nicon: M\ncategory: language\n---\nLine 1\nLine 2\nLine 3";
        let skill = parse_skill_markdown("multi", raw, false).unwrap();
        assert!(skill.content.contains("Line 1"));
        assert!(skill.content.contains("Line 3"));
    }

    #[test]
    fn parse_auto_triggers_full_yaml_block() {
        // Realistic block with common + FR + EN patterns. YAML-escaped
        // backslashes land as single backslashes so regexes work in JS.
        let raw = r#"---
name: Kronn Docs
icon: 📄
category: domain
auto_triggers:
  common:
    - "\\b(pdf|docx?|xlsx?)\\b"
  fr:
    - "génér.+(fichier|rapport)"
  en:
    - "generate.+(file|report)"
---
body content"#;
        let skill = parse_skill_markdown("kronn-docs", raw, true).unwrap();
        let at = skill.auto_triggers.as_ref().expect("auto_triggers must parse");
        assert_eq!(at.common.len(), 1, "common patterns: {:?}", at.common);
        // YAML `"\\b(pdf|docx?|xlsx?)\\b"` → runtime string `\b(pdf|docx?|xlsx?)\b`
        assert_eq!(at.common[0], r"\b(pdf|docx?|xlsx?)\b", "got: {}", at.common[0]);

        let fr = at.locales.get("fr").expect("fr bucket");
        assert_eq!(fr.len(), 1);
        assert!(fr[0].contains("génér"), "accents preserved: {}", fr[0]);
        let en = at.locales.get("en").expect("en bucket");
        assert_eq!(en.len(), 1);
    }

    #[test]
    fn parse_auto_triggers_absent_yields_none() {
        let raw = "---\nname: Plain\nicon: P\ncategory: language\n---\nbody";
        let skill = parse_skill_markdown("plain", raw, true).unwrap();
        assert!(skill.auto_triggers.is_none());
    }

    #[test]
    fn parse_auto_triggers_does_not_swallow_siblings() {
        // The `name:` after `auto_triggers:` block-level must be seen,
        // not treated as a nested trigger-bucket heading.
        let raw = r#"---
auto_triggers:
  common:
    - "pdf"
name: After
icon: A
category: domain
---
body"#;
        let skill = parse_skill_markdown("after", raw, true).unwrap();
        assert_eq!(skill.name, "After");
        assert_eq!(skill.auto_triggers.as_ref().unwrap().common, vec!["pdf".to_string()]);
    }

    /// Guard test: the workflow-architect skill MUST teach the new
    /// step types (ApiCall, Notify, BatchQuickPrompt) and the
    /// désagentification rule. Locked in 0.6.0 after the skill was found
    /// to be ~1 year stale (only Agent steps known). If a future edit
    /// strips these signals, this test fails immediately — better than
    /// noticing months later that AI-generated workflows are still
    /// emitting curl-in-bash steps for APIs we have plugins for.
    #[test]
    fn workflow_architect_skill_teaches_new_step_types() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;

        // Step types must all be referenced.
        assert!(c.contains("ApiCall"),
            "skill must mention ApiCall step type — désagentification");
        assert!(c.contains("Notify"),
            "skill must mention Notify webhook step type");
        assert!(c.contains("BatchQuickPrompt"),
            "skill must mention BatchQuickPrompt step type");
        // The cost-decision narrative must be explicit.
        assert!(c.contains("désagentification") || c.contains("Désagentification"),
            "skill must spell out 'désagentification' so the agent treats it as a first-class concept");
        assert!(c.contains("0 tokens") || c.contains("zero token") || c.contains("Zero tokens"),
            "skill must claim zero-token cost on Notify/ApiCall paths to motivate the routing");
        // Concrete API plugin references — without these the agent doesn't
        // know what's typically available and can't propose ApiCall steps.
        assert!(c.contains("chartbeat") || c.contains("Chartbeat"),
            "skill must reference at least one named API plugin (Chartbeat is the canonical example)");
        // Decision tree must be visible.
        assert!(c.contains("decision tree") || c.contains("Decision tree"),
            "skill must teach an explicit decision tree for picking step types");
    }

    /// 0.8.3 guard: the architect skill MUST teach the new
    /// Feasibility-Gated pattern + the `on_invalid: Fail` flag. Without
    /// these, AI-generated workflows for big tickets stay all-Agent,
    /// silently improvise, and burn ~50% more tokens than necessary
    /// for no traceability win.
    #[test]
    fn workflow_architect_skill_teaches_feasibility_gated_pattern() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;

        // The pattern must be referenced by name and the 4 categories
        // must all be mentioned (those are the manifest contract).
        assert!(c.contains("Feasibility-Gated"),
            "skill must reference the Feasibility-Gated pattern by name");
        for category in ["clear", "decided", "mocked", "blocked"] {
            assert!(c.contains(category),
                "skill must teach the '{category}' manifest category");
        }

        // The 3 KRONN-* marker variants must appear so the agent
        // knows to insert them when generating implement-step prompts.
        assert!(c.contains("KRONN-ASSUMED"),
            "skill must mention KRONN-ASSUMED marker");
        assert!(c.contains("KRONN-MOCKED"),
            "skill must mention KRONN-MOCKED marker");
        assert!(c.contains("KRONN-TODO"),
            "skill must mention KRONN-TODO marker");

        // The `[TRIAGE]` description marker triggers the runner-side
        // "audit, don't code" addendum (see triage::is_triage_step).
        // Without this hint the agent skips the addendum entirely.
        assert!(c.contains("[TRIAGE]"),
            "skill must instruct the agent to prefix triage step descriptions with '[TRIAGE]'");

        // The `on_invalid: Fail` flag is what makes the triage
        // contract strict — if it's not surfaced, agents will design
        // workflows where a malformed manifest silently propagates.
        assert!(c.contains("on_invalid"),
            "skill must document the TypedSchema `on_invalid` flag");
        assert!(c.contains("\"Fail\"") || c.contains("`Fail`"),
            "skill must show the `Fail` value of `on_invalid`");

        // The preset shortcut should be visible so the agent doesn't
        // hand-roll the 7 steps when the user just wants the default.
        assert!(c.contains("feasibility-autopilot"),
            "skill must reference the `feasibility-autopilot` preset id");

        // 0.8.3 cross-repo evidence — the skill must teach the
        // runner-side companion-repo injection (see runner.rs:124
        // and triage::TRIAGE_PROMPT_ADDENDUM). Without this section
        // an architect designing a migration workflow won't realize
        // the runtime injects linked_repos AND that the agent is
        // expected to cite `evidence: <repo>/<path>:<line>` in
        // `decided`/`mocked` entries — the killer differentiator
        // versus a flat "agent improvises in isolation" pipeline.
        assert!(c.contains("Cross-repo evidence"),
            "skill must teach the cross-repo evidence section (linked_repos auto-injection)");
        assert!(c.contains("linked_repos") || c.contains("Linked repositories"),
            "skill must mention `linked_repos` / `Linked repositories` block by name");
        assert!(c.contains("evidence:"),
            "skill must teach the `evidence: <repo>/<path>:<line>` citation format");
        assert!(c.contains("lift"),
            "skill must teach `lift` vs `invent` for evidence-backed values");
    }

    /// 0.8.3 guard: the architect counts step types correctly.
    /// Kronn ships **8** step types (Agent, ApiCall, BatchApiCall,
    /// BatchQuickPrompt, Exec, Gate, JsonData, Notify). A skill that
    /// still says "six step types" leaves an LLM consuming it unable
    /// to enumerate the catalog correctly. This test fires the moment
    /// the count drifts.
    #[test]
    fn workflow_architect_skill_counts_eight_step_types() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;
        assert!(c.contains("eight step types") || c.contains("8 step types"),
            "skill must say 'eight step types' (not 'six') — the catalog has grown");
        // The old wrong counts must NOT linger anywhere as authoritative.
        // We tolerate the word 'six' in other phrases but the literal
        // 'six step types' / 'seven step types' is the regression.
        assert!(!c.contains("six step types"),
            "skill still says 'six step types' — stale count, must be 'eight'");
        assert!(!c.contains("seven step types"),
            "skill still says 'seven step types' — stale count, must be 'eight'");
        assert!(!c.contains("The 7 step types"),
            "skill still says 'The 7 step types cover every case' — stale count");
    }

    /// 0.8.3 guard: the architect must surface the **reuse-first**
    /// pathway — Quick APIs, Quick Prompts, Custom API plugins, and
    /// the AI helper bubbles — BEFORE it hand-rolls inline configs.
    /// Without these signals, the architect re-emits the same 4-line
    /// `api_*` configs in every workflow and never tells the user
    /// they could declare a Custom API plugin for their private
    /// vendor (the 0.8.1 feature stays invisible).
    #[test]
    fn workflow_architect_skill_teaches_reuse_and_helpers() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;

        // The three reuse layers.
        assert!(c.contains("quick_prompt_id"),
            "skill must teach `quick_prompt_id` (Quick Prompt reuse)");
        assert!(c.contains("quick_api_id"),
            "skill must teach `quick_api_id` (Quick API reuse)");
        assert!(c.contains("Custom API plugin"),
            "skill must teach Custom API plugins as the alternative when no built-in plugin matches");

        // The AI helper bubbles — without these, the architect dictates
        // 10 lines of `api_*` config the wizard could fill in 2 clicks.
        assert!(c.contains("ApiCallAiHelper") || c.contains("ApiCall AI helper") || c.contains("🪄"),
            "skill must mention the ApiCall AI helper bubble (🪄) so the user uses it instead of hand-rolling");
        assert!(c.contains("CustomApiAiHelper") || c.contains("Custom API AI helper") || c.contains("curl"),
            "skill must mention the Custom API helper or the curl-paste pattern");

        // The reuse-first principle must be a top-level instruction,
        // not buried in optimization rules.
        assert!(c.contains("Reuse-first") || c.contains("reuse-first") || c.contains("Reuse first"),
            "skill must phrase the reuse-first principle explicitly");
    }

    /// 0.8.3 guard: the workflow-architect MUST teach the
    /// `KRONN:BUNDLE_READY` protocol (the atomic-creation killer flow
    /// for workflow+QP+QA+Custom API together). Without this signal
    /// in the skill, the architect tells users "first go create your
    /// QP in the Quick Prompts tab, then come back" — 3-tab dance the
    /// 0.8.3 bundle endpoint exists precisely to eliminate.
    #[test]
    fn workflow_architect_skill_teaches_bundle_protocol() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;

        // The signal name + the wire prefix.
        assert!(c.contains("KRONN:BUNDLE_READY"),
            "skill must reference the KRONN:BUNDLE_READY signal");
        assert!(c.contains("@bundle:"),
            "skill must teach the @bundle:<id> sentinel for cross-artifact references");

        // The bundle endpoint path must be discoverable (so the agent
        // can mention it when explaining what the button does).
        assert!(c.contains("/api/workflows/bundle"),
            "skill must reference the /api/workflows/bundle endpoint");

        // Each artifact category must be enumerated so the agent
        // knows what `bundle_id` placeholders to emit.
        assert!(c.contains("quick_prompts"),
            "skill must enumerate the `quick_prompts` bundle category");
        assert!(c.contains("quick_apis"),
            "skill must enumerate the `quick_apis` bundle category");
        assert!(c.contains("custom_apis"),
            "skill must enumerate the `custom_apis` bundle category");

        // The agent should default to BUNDLE_READY when in doubt.
        assert!(c.to_lowercase().contains("prefer `bundle_ready`")
            || c.to_lowercase().contains("prefer bundle_ready")
            || c.to_lowercase().contains("preferred when"),
            "skill must instruct the agent to PREFER BUNDLE_READY (superset over WORKFLOW_READY)");
    }

    /// 0.8.3 guard: the workflow-architect skill MUST enforce that
    /// any emitted `ApiCall` step carries an `api_plugin_slug` from
    /// the built-in list (or escalates to Custom API via BUNDLE).
    /// Without this rule, agents emit half-baked workflows where
    /// the user has to hand-pick the plugin from a dropdown post-
    /// creation (the exact UX the user flagged 2026-05-14).
    #[test]
    fn workflow_architect_skill_requires_api_plugin_slug_and_post_emission_disclaimer() {
        let skills = list_all_skills();
        let arch = skills.iter().find(|s| s.id == "workflow-architect")
            .expect("workflow-architect skill must exist");
        let c = &arch.content;

        // The api_plugin_slug field must be flagged REQUIRED for
        // ApiCall steps, with the recognized plugin list inline so
        // the agent can route the endpoint shape to a slug without
        // a follow-up question.
        assert!(c.contains("REQUIRED for any `ApiCall` step")
             || c.contains("REQUIRED for any ApiCall step"),
            "skill must mark api_plugin_slug as REQUIRED for ApiCall steps");
        // Recognized built-in slugs must be enumerated in the field
        // doc so the agent picks one verbatim.
        for slug in ["chartbeat", "jira", "github", "adobe-analytics"] {
            assert!(c.contains(&format!("\"{slug}\"")),
                "skill must enumerate the `{slug}` built-in slug in the api_plugin_slug field doc");
        }
        // Routing instruction: the endpoint path → slug mapping
        // must be explicit so the agent doesn't have to guess.
        assert!(c.contains("/rest/api/3/") || c.contains("/repos/{owner}/{repo}"),
            "skill must give example endpoint-paths → slug mappings (Jira, GitHub patterns)");
        // Fallback path: when no built-in matches, route via Custom
        // API plugin INSIDE the bundle, not hand-roll.
        assert!(c.to_lowercase().contains("custom api plugin"),
            "skill must mention Custom API plugin as the fallback when no built-in matches");

        // Post-emission disclaimer: the workflow architect MUST add
        // a "Template — review before triggering" warning AFTER the
        // signal line. Locks the explicit-responsibility-transfer
        // rule the user requested 2026-05-14 (the "ça nous dédouane"
        // sentence). Without this guard, future skill edits could
        // silently drop the disclaimer and ship half-broken
        // workflows without warning.
        assert!(c.contains("Template — review before triggering")
             || c.contains("template — review before triggering"),
            "skill must instruct the agent to emit the 'Template — review before triggering' disclaimer after KRONN:WORKFLOW_READY / KRONN:BUNDLE_READY");
        // The disclaimer must call out at least the field types
        // most likely to be incomplete in auto-generated steps.
        assert!(c.contains("api_plugin_slug") && c.contains("quick_prompt_id"),
            "post-emission disclaimer must name the fields most likely to be left empty");
    }

    /// 0.7.1 / 0.8.3 guard: the bootstrap-architect MUST instruct the
    /// agent to write `docs/AGENTS.md` (the canonical project context
    /// entry point after the 0.7.1 pivot) — and MUST NOT default to the
    /// legacy `ai/` path. It should also leverage the
    /// `structured-questions` skill for Stage 1 clarifying questions,
    /// and hand off to `workflow-architect` at the end of Stage 3.
    #[test]
    fn bootstrap_architect_skill_writes_docs_agents_and_chains_skills() {
        let skills = list_all_skills();
        let s = skills.iter().find(|s| s.id == "bootstrap-architect")
            .expect("bootstrap-architect skill must exist");
        let c = &s.content;

        // Post-0.7.1: the canonical output file MUST be docs/AGENTS.md.
        assert!(c.contains("docs/AGENTS.md"),
            "bootstrap-architect must reference docs/AGENTS.md (0.7.1 pivot — canonical project context entry point)");

        // The skill must explicitly warn against the legacy `ai/` path,
        // otherwise an LLM trained on older Kronn docs may regenerate
        // there and create silent drift across the project.
        assert!(c.to_lowercase().contains("legacy `ai/`") || c.to_lowercase().contains("deprecated in 0.7.1"),
            "bootstrap-architect must call out the legacy `ai/` path as deprecated since 0.7.1");

        // Skill composition: Stage 1 clarifying Q&A should reference
        // the structured-questions syntax. Without this hint the agent
        // falls back to free-prose questions and answers can't be
        // captured cleanly downstream.
        assert!(c.contains("structured-questions") || c.contains("{{var}}: question"),
            "bootstrap-architect Stage 1 must reference the structured-questions skill or its {{var}}: syntax");

        // Stage 3 should hand off to workflow-architect once the
        // tracker issues exist — that's the natural next-mile for
        // converting epics into automation.
        assert!(c.contains("workflow-architect"),
            "bootstrap-architect must reference workflow-architect as the next-step skill after Stage 3");
    }

    /// `kronn-docs` is about document EXPORT (PDF/DOCX/XLSX/PPTX),
    /// not about Kronn's `docs/AGENTS.md` project-context system.
    /// The name collision is a known UX trap so the skill MUST
    /// disambiguate in the description, AND it MUST follow the
    /// project rule `feedback_no_real_names` (no `EW-XXXX` / real
    /// vendor prefixes in examples).
    #[test]
    fn kronn_docs_skill_is_disambiguated_and_uses_generic_placeholders() {
        let skills = list_all_skills();
        let s = skills.iter().find(|s| s.id == "Kronn Docs")
            .or_else(|| skills.iter().find(|s| s.id.eq_ignore_ascii_case("kronn-docs")))
            .or_else(|| skills.iter().find(|s| s.name == "Kronn Docs"))
            .expect("kronn-docs skill must exist (search by id, slug, or display name)");
        let c = &s.content;

        // No real-name placeholders. The project rule
        // `feedback_no_real_names.md` is explicit: tests/placeholders
        // must use generic prefixes (PRJ-, TestUser, PeerAlpha, etc.).
        assert!(!c.contains("EW-1234") && !c.contains("EW-2210"),
            "kronn-docs must not use real ticket prefixes (EW-…) — see feedback_no_real_names rule. Replace with PRJ-…");

        // Description must trigger reliably on common phrasings.
        // The skill-creator review flagged this as undertriggering.
        let d = &s.description.to_lowercase();
        assert!(d.contains("pdf") || d.contains("docx") || d.contains("xlsx"),
            "kronn-docs description must enumerate at least one format token (pdf/docx/xlsx)");
        assert!(d.contains("export") || d.contains("download") || d.contains("report"),
            "kronn-docs description should include 'export' / 'download' / 'report' triggers");
    }

    /// `structured-questions` MUST teach the two silent-failure
    /// gotchas the parser enforces: question text on the SAME line as
    /// `{{var}}:` (line-break breaks the form), and empty `{{var}}:`
    /// is dropped silently. Without these, agents emit forms that
    /// only render half the fields they meant to ask about.
    #[test]
    fn structured_questions_skill_documents_parser_gotchas() {
        let skills = list_all_skills();
        let s = skills.iter().find(|s| s.id == "structured-questions")
            .expect("structured-questions skill must exist");
        let c = &s.content;

        // The "SAME line" rule.
        assert!(c.contains("SAME line") || c.contains("same line"),
            "structured-questions must warn that the question text must be on the SAME line as `{{var}}:`");

        // The empty-body silent drop.
        assert!(c.contains("empty") || c.contains("skipped silently") || c.contains("dropped silently"),
            "structured-questions must warn that an empty `{{var}}:` (no question body) is dropped silently");

        // The frontend renderer's name must surface so the agent
        // tells the user "your input appears as a form" — not just
        // "I'll ask you some questions".
        assert!(c.contains("AgentQuestionForm") || c.contains("inline form"),
            "structured-questions must reference the `AgentQuestionForm` renderer or 'inline form' so the agent surfaces the UI affordance");
    }
}
