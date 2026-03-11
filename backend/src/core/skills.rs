//! Skills loader — reads builtin (embedded) and custom skills from disk.
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
    BuiltinSkill { id: "token-saver", content: include_str!("../skills/token-saver.md") },
    BuiltinSkill { id: "typescript-dev", content: include_str!("../skills/typescript-dev.md") },
    BuiltinSkill { id: "rust-dev", content: include_str!("../skills/rust-dev.md") },
    BuiltinSkill { id: "security-auditor", content: include_str!("../skills/security-auditor.md") },
    BuiltinSkill { id: "product-owner", content: include_str!("../skills/product-owner.md") },
    BuiltinSkill { id: "devils-advocate", content: include_str!("../skills/devils-advocate.md") },
    BuiltinSkill { id: "qa-engineer", content: include_str!("../skills/qa-engineer.md") },
    BuiltinSkill { id: "devops-expert", content: include_str!("../skills/devops-expert.md") },
    BuiltinSkill { id: "seo-expert", content: include_str!("../skills/seo-expert.md") },
    BuiltinSkill { id: "green-it-expert", content: include_str!("../skills/green-it-expert.md") },
    BuiltinSkill { id: "data-engineer", content: include_str!("../skills/data-engineer.md") },
    BuiltinSkill { id: "tech-lead", content: include_str!("../skills/tech-lead.md") },
];

// ─── Frontmatter parsing ────────────────────────────────────────────────────

fn parse_skill_markdown(id: &str, raw: &str, is_builtin: bool) -> Option<Skill> {
    // Split frontmatter from content
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        tracing::warn!("Skill '{}' missing YAML frontmatter", id);
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 4..].trim().to_string();

    // Parse YAML frontmatter manually (avoid adding a full YAML dep)
    let mut name = String::new();
    let mut description = String::new();
    let mut icon = String::new();
    let mut category = SkillCategory::Meta;
    let mut conflicts = Vec::new();

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
                "Technical" => SkillCategory::Technical,
                "Business" => SkillCategory::Business,
                _ => SkillCategory::Meta,
            };
        } else if let Some(val) = line.strip_prefix("conflicts:") {
            let val = val.trim();
            if val != "[]" && !val.is_empty() {
                // Simple inline array: [a, b, c]
                let inner = val.trim_start_matches('[').trim_end_matches(']');
                conflicts = inner.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
        }
    }

    if name.is_empty() {
        tracing::warn!("Skill '{}' has no name in frontmatter", id);
        return None;
    }

    Some(Skill {
        id: id.to_string(),
        name,
        description,
        icon,
        category,
        content: body,
        is_builtin,
        conflicts,
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

    // Load builtins
    for builtin in BUILTIN_SKILLS {
        if let Some(skill) = parse_skill_markdown(builtin.id, builtin.content, true) {
            skills.push(skill);
        }
    }

    // Load custom skills
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
pub fn save_custom_skill(name: &str, description: &str, icon: &str, category: &SkillCategory, content: &str) -> Result<String, String> {
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
        SkillCategory::Technical => "Technical",
        SkillCategory::Business => "Business",
        SkillCategory::Meta => "Meta",
    };

    let file_content = format!(
        "---\nname: {}\ndescription: {}\nicon: {}\ncategory: {}\nconflicts: []\n---\n{}",
        name, description, icon, cat_str, content
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

    // ── Builtin skills loading ──────────────────────────────────────────────

    #[test]
    fn parse_builtin_skills() {
        let skills = list_all_skills();
        assert!(skills.len() >= 12, "Expected at least 12 builtin skills, got {}", skills.len());

        let token_saver = skills.iter().find(|s| s.id == "token-saver").unwrap();
        assert_eq!(token_saver.name, "Token Saver");
        assert_eq!(token_saver.icon, "Zap");
        assert_eq!(token_saver.category, SkillCategory::Meta);
        assert!(token_saver.is_builtin);
        assert!(!token_saver.content.is_empty());
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
            assert!(!skill.description.is_empty(), "Skill '{}' has empty description", skill.id);
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
        assert!(skills.iter().any(|s| s.category == SkillCategory::Technical));
        assert!(skills.iter().any(|s| s.category == SkillCategory::Business));
        assert!(skills.iter().any(|s| s.category == SkillCategory::Meta));
    }

    // ── get_skill / get_skills_by_ids ───────────────────────────────────────

    #[test]
    fn get_skill_found() {
        let skill = get_skill("rust-dev");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().name, "Rust Dev");
    }

    #[test]
    fn get_skill_not_found() {
        let skill = get_skill("nonexistent-skill");
        assert!(skill.is_none());
    }

    #[test]
    fn get_skills_by_ids_preserves_order() {
        let skills = get_skills_by_ids(&["rust-dev".into(), "token-saver".into()]);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].id, "rust-dev");
        assert_eq!(skills[1].id, "token-saver");
    }

    #[test]
    fn get_skills_by_ids_skips_unknown() {
        let skills = get_skills_by_ids(&["token-saver".into(), "nonexistent".into(), "rust-dev".into()]);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].id, "token-saver");
        assert_eq!(skills[1].id, "rust-dev");
    }

    #[test]
    fn get_skills_by_ids_empty_input() {
        let skills = get_skills_by_ids(&[]);
        assert!(skills.is_empty());
    }

    // ── build_skills_prompt ─────────────────────────────────────────────────

    #[test]
    fn build_skills_prompt_empty() {
        let prompt = build_skills_prompt(&[]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_with_ids() {
        let prompt = build_skills_prompt(&["token-saver".into(), "rust-dev".into()]);
        assert!(prompt.contains("Token Saver"));
        assert!(prompt.contains("Rust Dev"));
        assert!(prompt.contains("=== Active Skills ==="));
    }

    #[test]
    fn build_skills_prompt_unknown_ids_ignored() {
        let prompt = build_skills_prompt(&["nonexistent-1".into(), "nonexistent-2".into()]);
        assert!(prompt.is_empty());
    }

    #[test]
    fn build_skills_prompt_single_skill() {
        let prompt = build_skills_prompt(&["security-auditor".into()]);
        assert!(prompt.contains("Security Auditor"));
        assert!(prompt.contains("=== Active Skills ==="));
    }

    // ── Frontmatter parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_frontmatter_valid() {
        let raw = "---\nname: Test Skill\ndescription: A test\nicon: Star\ncategory: Technical\nconflicts: []\n---\nDo the thing.";
        let skill = parse_skill_markdown("test", raw, false).unwrap();
        assert_eq!(skill.name, "Test Skill");
        assert_eq!(skill.description, "A test");
        assert_eq!(skill.icon, "Star");
        assert_eq!(skill.category, SkillCategory::Technical);
        assert_eq!(skill.content, "Do the thing.");
        assert!(!skill.is_builtin);
        assert!(skill.conflicts.is_empty());
    }

    #[test]
    fn parse_frontmatter_business_category() {
        let raw = "---\nname: PO\ndescription: d\nicon: I\ncategory: Business\nconflicts: []\n---\ncontent";
        let skill = parse_skill_markdown("po", raw, true).unwrap();
        assert_eq!(skill.category, SkillCategory::Business);
        assert!(skill.is_builtin);
    }

    #[test]
    fn parse_frontmatter_unknown_category_defaults_meta() {
        let raw = "---\nname: X\ndescription: d\nicon: I\ncategory: Unknown\nconflicts: []\n---\ncontent";
        let skill = parse_skill_markdown("x", raw, false).unwrap();
        assert_eq!(skill.category, SkillCategory::Meta);
    }

    #[test]
    fn parse_frontmatter_with_conflicts() {
        let raw = "---\nname: X\ndescription: d\nicon: I\ncategory: Meta\nconflicts: [token-saver, verbose]\n---\ncontent";
        let skill = parse_skill_markdown("x", raw, false).unwrap();
        assert_eq!(skill.conflicts, vec!["token-saver", "verbose"]);
    }

    #[test]
    fn parse_frontmatter_missing_yields_none() {
        let raw = "No frontmatter here, just content.";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_no_name_yields_none() {
        let raw = "---\ndescription: d\nicon: I\ncategory: Meta\nconflicts: []\n---\ncontent";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_unclosed_yields_none() {
        let raw = "---\nname: X\ndescription: d\nicon: I\ncategory: Meta\nconflicts: []\ncontent without closing frontmatter";
        assert!(parse_skill_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_multiline_content() {
        let raw = "---\nname: Multi\ndescription: d\nicon: I\ncategory: Technical\nconflicts: []\n---\nLine 1\nLine 2\nLine 3";
        let skill = parse_skill_markdown("multi", raw, false).unwrap();
        assert!(skill.content.contains("Line 1"));
        assert!(skill.content.contains("Line 3"));
    }

    // ── delete_custom_skill validation ──────────────────────────────────────

    #[test]
    fn delete_builtin_skill_rejected() {
        let result = delete_custom_skill("token-saver");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("builtin"));
    }
}
