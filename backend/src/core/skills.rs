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
    // Domain
    BuiltinSkill { id: "security", content: include_str!("../skills/security.md") },
    BuiltinSkill { id: "devops", content: include_str!("../skills/devops.md") },
    BuiltinSkill { id: "data-engineering", content: include_str!("../skills/data-engineering.md") },
    BuiltinSkill { id: "database", content: include_str!("../skills/database.md") },
    // Business
    BuiltinSkill { id: "seo", content: include_str!("../skills/seo.md") },
    BuiltinSkill { id: "web-performance", content: include_str!("../skills/web-performance.md") },
    BuiltinSkill { id: "green-it", content: include_str!("../skills/green-it.md") },
    BuiltinSkill { id: "accessibility", content: include_str!("../skills/accessibility.md") },
    BuiltinSkill { id: "gdpr", content: include_str!("../skills/gdpr.md") },
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
    let mut icon = String::new();
    let mut category = SkillCategory::Domain;

    for line in yaml_str.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("icon:") {
            icon = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("category:") {
            category = match val.trim() {
                "language" => SkillCategory::Language,
                "domain" => SkillCategory::Domain,
                "business" => SkillCategory::Business,
                _ => SkillCategory::Domain,
            };
        }
    }

    if name.is_empty() {
        tracing::warn!("Skill '{}' has no name in frontmatter", id);
        return None;
    }

    Some(Skill {
        id: id.to_string(),
        name,
        icon,
        category,
        content: body,
        is_builtin,
    })
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

/// Save a custom skill to disk. Returns the generated ID.
pub fn save_custom_skill(name: &str, icon: &str, category: &SkillCategory, content: &str) -> Result<String, String> {
    let dir = custom_skills_dir().ok_or("Cannot determine config directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create skills dir: {}", e))?;

    let slug: String = name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let id = format!("custom-{}", slug);
    let cat_str = match category {
        SkillCategory::Language => "language",
        SkillCategory::Domain => "domain",
        SkillCategory::Business => "business",
    };

    let file_content = format!(
        "---\nname: {}\ncategory: {}\nicon: {}\nbuiltin: false\n---\n{}",
        name, cat_str, icon, content
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
        assert!(skills.len() >= 14, "Expected at least 14 builtin skills, got {}", skills.len());

        let rust = skills.iter().find(|s| s.id == "rust").unwrap();
        assert_eq!(rust.name, "Rust");
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
        assert_eq!(skill.unwrap().name, "TypeScript");
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
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("TypeScript"));
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
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("=== Active Skills ==="));
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
}
