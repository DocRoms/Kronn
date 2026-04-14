//! Profiles loader — reads builtin (embedded) and custom profiles from disk.
//!
//! Profiles represent WHO the agent is (persona, single-select).
//! New frontmatter: name, role, avatar (emoji), color (hex), category, default_engine.
//!
//! Builtin profiles are embedded at compile time from `src/profiles/*.md`.
//! Custom profiles live in `~/.config/kronn/profiles/` as Markdown files with YAML frontmatter.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::models::{AgentProfile, ProfileCategory};

/// Persona name overrides file: ~/.config/kronn/persona_overrides.json
fn persona_overrides_path() -> Option<PathBuf> {
    let config_dir = crate::core::config::config_dir().ok()?;
    Some(config_dir.join("persona_overrides.json"))
}

fn load_persona_overrides() -> HashMap<String, String> {
    persona_overrides_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

pub fn save_persona_override(profile_id: &str, persona_name: &str) -> Result<(), String> {
    let path = persona_overrides_path().ok_or("Cannot determine config directory")?;
    let dir = path.parent().ok_or("Invalid path")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create config dir: {}", e))?;

    let mut overrides = load_persona_overrides();
    if persona_name.is_empty() {
        overrides.remove(profile_id);
    } else {
        overrides.insert(profile_id.to_string(), persona_name.to_string());
    }

    let json = serde_json::to_string_pretty(&overrides).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Cannot write overrides: {}", e))?;
    Ok(())
}

// ─── Builtin profiles (embedded at compile time) ─────────────────────────────

struct BuiltinProfile {
    id: &'static str,
    content: &'static str,
}

const BUILTIN_PROFILES: &[BuiltinProfile] = &[
    // Technical
    BuiltinProfile { id: "architect", content: include_str!("../profiles/architect.md") },
    BuiltinProfile { id: "tech-lead", content: include_str!("../profiles/tech-lead.md") },
    BuiltinProfile { id: "qa-engineer", content: include_str!("../profiles/qa-engineer.md") },
    // Business
    BuiltinProfile { id: "product-owner", content: include_str!("../profiles/product-owner.md") },
    BuiltinProfile { id: "scrum-master", content: include_str!("../profiles/scrum-master.md") },
    BuiltinProfile { id: "technical-writer", content: include_str!("../profiles/technical-writer.md") },
    // Meta
    BuiltinProfile { id: "devils-advocate", content: include_str!("../profiles/devils-advocate.md") },
    BuiltinProfile { id: "mentor", content: include_str!("../profiles/mentor.md") },
    // Business
    BuiltinProfile { id: "entrepreneur", content: include_str!("../profiles/entrepreneur.md") },
    BuiltinProfile { id: "ux-designer", content: include_str!("../profiles/ux-designer.md") },
    // Technical
    BuiltinProfile { id: "game-developer", content: include_str!("../profiles/game-developer.md") },
    // Data
    BuiltinProfile { id: "data-analyst", content: include_str!("../profiles/data-analyst.md") },
    BuiltinProfile { id: "data-engineer", content: include_str!("../profiles/data-engineer.md") },
    // Growth
    BuiltinProfile { id: "seo-growth", content: include_str!("../profiles/seo-growth.md") },
    // Ops & Strategy
    BuiltinProfile { id: "sre", content: include_str!("../profiles/sre.md") },
    BuiltinProfile { id: "staff-engineer", content: include_str!("../profiles/staff-engineer.md") },
    // Communication
    BuiltinProfile { id: "translator", content: include_str!("../profiles/translator.md") },
];

// ─── Frontmatter parsing ────────────────────────────────────────────────────

fn parse_profile_markdown(id: &str, raw: &str, is_builtin: bool) -> Option<AgentProfile> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        tracing::warn!("Profile '{}' missing YAML frontmatter", id);
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 4..].trim().to_string();

    let mut name = String::new();
    let mut persona_name = String::new();
    let mut role = String::new();
    let mut avatar = String::new();
    let mut color = String::from("#6b7280");
    let mut category = ProfileCategory::Meta;
    let mut default_engine: Option<String> = None;

    for line in yaml_str.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("persona_name:") {
            persona_name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("role:") {
            role = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("avatar:") {
            avatar = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("color:") {
            color = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("category:") {
            category = match val.trim() {
                "technical" => ProfileCategory::Technical,
                "business" => ProfileCategory::Business,
                _ => ProfileCategory::Meta,
            };
        } else if let Some(val) = line.strip_prefix("default_engine:") {
            let v = val.trim().to_string();
            if !v.is_empty() {
                default_engine = Some(v);
            }
        }
    }

    if name.is_empty() {
        tracing::warn!("Profile '{}' has no name in frontmatter", id);
        return None;
    }

    // Default persona_name to first 3 chars of name if not set
    if persona_name.is_empty() {
        persona_name = name.chars().take(3).collect();
    }

    let token_estimate = ((body.len() + persona_name.len() + role.len() + 30) / 4) as u32;

    Some(AgentProfile {
        id: id.to_string(),
        name,
        persona_name,
        role,
        avatar,
        color,
        category,
        token_estimate,
        persona_prompt: body,
        default_engine,
        is_builtin,
    })
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Custom profiles directory: ~/.config/kronn/profiles/
fn custom_profiles_dir() -> Option<PathBuf> {
    let config_dir = crate::core::config::config_dir().ok()?;
    Some(config_dir.join("profiles"))
}

/// List all available profiles (builtin + custom).
pub fn list_all_profiles() -> Vec<AgentProfile> {
    let mut profiles = Vec::new();

    for builtin in BUILTIN_PROFILES {
        if let Some(profile) = parse_profile_markdown(builtin.id, builtin.content, true) {
            profiles.push(profile);
        }
    }

    if let Some(dir) = custom_profiles_dir() {
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("md") {
                        let id = format!("custom-{}", path.file_stem().unwrap_or_default().to_string_lossy());
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Some(profile) = parse_profile_markdown(&id, &content, false) {
                                profiles.push(profile);
                            }
                        }
                    }
                }
            }
        }
    }

    // Apply persona name overrides
    let overrides = load_persona_overrides();
    for profile in &mut profiles {
        if let Some(override_name) = overrides.get(&profile.id) {
            profile.persona_name = override_name.clone();
        }
    }

    profiles
}

/// Get a single profile by ID.
pub fn get_profile(id: &str) -> Option<AgentProfile> {
    list_all_profiles().into_iter().find(|p| p.id == id)
}

/// Build the persona prompt text for multiple profiles.
/// When multiple profiles are selected, adds a collaborative instruction.
pub fn build_profiles_prompt(profile_ids: &[String]) -> String {
    if profile_ids.is_empty() {
        return String::new();
    }

    let profiles: Vec<AgentProfile> = profile_ids.iter()
        .filter_map(|id| get_profile(id))
        .collect();

    if profiles.is_empty() {
        return String::new();
    }

    if profiles.len() == 1 {
        let p = &profiles[0];
        return format!("=== Agent Profile: {} ({}) ===\n{}\n\n", p.name, p.role, p.persona_prompt);
    }

    // Multi-profile: structured discussion mode
    let mut prompt = String::from("=== Multi-Profile Discussion Mode ===\n\n");
    prompt.push_str("You have multiple expert profiles active. You MUST respond as a structured discussion.\n");
    prompt.push_str("Each profile speaks individually with their own voice, perspective, and expertise.\n\n");
    prompt.push_str("FORMAT — follow this EXACTLY:\n\n");
    prompt.push_str("1. For EACH profile below, write a response block using this format:\n");
    prompt.push_str("   > **[Avatar] [FirstName] ([Role])** : [their response in their own voice]\n\n");
    prompt.push_str("   Each profile MUST:\n");
    prompt.push_str("   - Speak from their specific expertise and role\n");
    prompt.push_str("   - Have a distinct opinion (agree, disagree, nuance — not all the same)\n");
    prompt.push_str("   - Use a natural, conversational tone (not bullet points)\n");
    prompt.push_str("   - Be concise (3-6 sentences per profile)\n\n");
    prompt.push_str("2. After ALL profiles have spoken, add a synthesis:\n");
    prompt.push_str("   **Synthesis** : [balanced summary of agreements, disagreements, and final recommendation]\n\n");
    prompt.push_str("IMPORTANT:\n");
    prompt.push_str("- Each profile is a DISTINCT voice. Do NOT merge perspectives.\n");
    prompt.push_str("- Use the blockquote format (>) for each profile response.\n");
    prompt.push_str("- The synthesis is NOT in a blockquote.\n\n");

    prompt.push_str("--- Active Profiles ---\n\n");
    for p in &profiles {
        prompt.push_str(&format!("FirstName: {}\nAvatar: {}\nRole: {}\n{}\n\n",
            p.persona_name, p.avatar, p.role, p.persona_prompt));
    }

    prompt
}

/// Build a compact profiles prompt for agents with small context windows.
/// Uses a terse one-line format instead of full persona prompts (~80% token savings).
pub fn build_profiles_prompt_compact(profile_ids: &[String]) -> String {
    if profile_ids.is_empty() {
        return String::new();
    }

    let profiles: Vec<AgentProfile> = profile_ids.iter()
        .filter_map(|id| get_profile(id))
        .collect();

    if profiles.is_empty() {
        return String::new();
    }

    let mut prompt = String::from("=== Profiles ===\n");
    for p in &profiles {
        prompt.push_str(&format!("[{} {} | {} | {}]\n", p.avatar, p.persona_name, p.role,
            // Extract first 2 sentences of persona_prompt for better context
            {
                let sentences: Vec<&str> = p.persona_prompt.splitn(3, '.').collect();
                let s = if sentences.len() >= 2 {
                    format!("{}. {}.", sentences[0].trim(), sentences[1].trim())
                } else {
                    sentences[0].trim().to_string()
                };
                s
            }
        ));
    }

    if profiles.len() > 1 {
        prompt.push_str("Respond as each profile separately. Be concise (3-5 sentences each). End with a synthesis.\n");
    }

    prompt
}

// Keep the old function for backward compat but mark deprecated
pub fn build_profile_prompt(profile_id: &str) -> String {
    build_profiles_prompt(&[profile_id.to_string()])
}

/// Data for creating/updating a custom profile.
pub struct CustomProfileData<'a> {
    pub name: &'a str,
    pub persona_name: &'a str,
    pub role: &'a str,
    pub avatar: &'a str,
    pub color: &'a str,
    pub category: &'a ProfileCategory,
    pub persona_prompt: &'a str,
    pub default_engine: Option<&'a str>,
}

/// Save a custom profile to disk. Returns the generated ID.
pub fn save_custom_profile(data: &CustomProfileData) -> Result<String, String> {
    let dir = custom_profiles_dir().ok_or("Cannot determine config directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create profiles dir: {}", e))?;

    let slug: String = data.name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let id = format!("custom-{}", slug);
    let cat_str = match data.category {
        ProfileCategory::Technical => "technical",
        ProfileCategory::Business => "business",
        ProfileCategory::Meta => "meta",
    };

    let engine_line = match data.default_engine {
        Some(e) => format!("default_engine: {}\n", e),
        None => String::new(),
    };

    let pn = if data.persona_name.is_empty() { data.name.chars().take(3).collect::<String>() } else { data.persona_name.to_string() };

    let file_content = format!(
        "---\nname: {}\npersona_name: {}\nrole: {}\navatar: {}\ncolor: \"{}\"\ncategory: {}\nbuiltin: false\n{}---\n{}",
        data.name, pn, data.role, data.avatar, data.color, cat_str, engine_line, data.persona_prompt
    );

    let path = dir.join(format!("{}.md", slug));
    std::fs::write(&path, file_content).map_err(|e| format!("Cannot write profile: {}", e))?;

    Ok(id)
}

/// Delete a custom profile from disk.
pub fn delete_custom_profile(id: &str) -> Result<bool, String> {
    if !id.starts_with("custom-") {
        return Err("Cannot delete builtin profiles".into());
    }
    let slug = id.strip_prefix("custom-").unwrap();
    let dir = custom_profiles_dir().ok_or("Cannot determine config directory")?;
    let path = dir.join(format!("{}.md", slug));

    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Cannot delete profile: {}", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_builtin_profiles() {
        let profiles = list_all_profiles();
        assert!(profiles.len() >= 17, "Expected at least 17 builtin profiles, got {}", profiles.len());

        let architect = profiles.iter().find(|p| p.id == "architect").unwrap();
        assert_eq!(architect.name, "Architect");
        assert_eq!(architect.role, "Software Architect");
        assert_eq!(architect.avatar, "🏗️");
        assert!(architect.color.starts_with('#'));
        assert_eq!(architect.category, ProfileCategory::Technical);
        assert!(architect.is_builtin);
        assert!(!architect.persona_prompt.is_empty());
    }

    #[test]
    fn all_builtins_are_marked_builtin() {
        let profiles = list_all_profiles();
        for profile in &profiles {
            if !profile.id.starts_with("custom-") {
                assert!(profile.is_builtin, "Profile '{}' should be builtin", profile.id);
            }
        }
    }

    #[test]
    fn all_builtins_have_required_fields() {
        let profiles = list_all_profiles();
        for profile in &profiles {
            if profile.is_builtin {
                assert!(!profile.name.is_empty(), "Profile '{}' has empty name", profile.id);
                assert!(!profile.role.is_empty(), "Profile '{}' has empty role", profile.id);
                assert!(!profile.avatar.is_empty(), "Profile '{}' has empty avatar", profile.id);
                assert!(!profile.color.is_empty(), "Profile '{}' has empty color", profile.id);
                assert!(!profile.persona_prompt.is_empty(), "Profile '{}' has empty persona_prompt", profile.id);
            }
        }
    }

    #[test]
    fn builtin_ids_are_unique() {
        let profiles = list_all_profiles();
        let mut ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let count_before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count_before, "Duplicate profile IDs found");
    }

    #[test]
    fn all_three_categories_represented() {
        let profiles = list_all_profiles();
        assert!(profiles.iter().any(|p| p.category == ProfileCategory::Technical));
        assert!(profiles.iter().any(|p| p.category == ProfileCategory::Business));
        assert!(profiles.iter().any(|p| p.category == ProfileCategory::Meta));
    }

    #[test]
    fn new_profiles_exist() {
        for (id, category) in [
            ("entrepreneur", ProfileCategory::Business),
            ("game-developer", ProfileCategory::Technical),
            ("ux-designer", ProfileCategory::Business),
        ] {
            let profile = get_profile(id);
            assert!(profile.is_some(), "Profile '{}' must exist", id);
            let p = profile.unwrap();
            assert_eq!(p.category, category, "Profile '{}' wrong category", id);
            assert!(!p.persona_prompt.is_empty(), "Profile '{}' must have content", id);
        }
    }

    #[test]
    fn new_data_and_seo_profiles_exist() {
        for (id, category, expected_persona) in [
            ("data-analyst", ProfileCategory::Business, "Ren"),
            ("data-engineer", ProfileCategory::Technical, "Ash"),
            ("seo-growth", ProfileCategory::Business, "Rio"),
            ("sre", ProfileCategory::Technical, "Ops"),
            ("staff-engineer", ProfileCategory::Technical, "Dex"),
            ("translator", ProfileCategory::Business, "Lin"),
        ] {
            let profile = get_profile(id);
            assert!(profile.is_some(), "Profile '{}' must exist", id);
            let p = profile.unwrap();
            assert_eq!(p.category, category, "Profile '{}' wrong category", id);
            assert_eq!(p.persona_name, expected_persona, "Profile '{}' wrong persona_name", id);
            assert!(!p.persona_prompt.is_empty(), "Profile '{}' must have content", id);
            assert!(p.default_engine.is_some(), "Profile '{}' must have default_engine", id);
        }
    }

    #[test]
    fn all_persona_names_are_unique() {
        let profiles = list_all_profiles();
        let mut names: Vec<&str> = profiles.iter().map(|p| p.persona_name.as_str()).collect();
        let count_before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count_before, "Duplicate persona names found");
    }

    #[test]
    fn bootstrap_includes_entrepreneur() {
        // The bootstrap discussion should include the entrepreneur profile.
        // This is a documentation test — the actual code is in api/projects.rs
        // but we verify the profile exists and is usable.
        let profile = get_profile("entrepreneur");
        assert!(profile.is_some(), "entrepreneur profile must exist for bootstrap");
        assert_eq!(profile.unwrap().category, ProfileCategory::Business);
    }

    #[test]
    fn entrepreneur_has_unique_persona_and_engine() {
        let p = get_profile("entrepreneur").expect("entrepreneur must exist");
        assert_eq!(p.persona_name, "Lea", "Entrepreneur persona should be Lea (not Max)");
        assert!(p.default_engine.is_some(), "Entrepreneur must have default_engine");
        // No other profile should share the name "Lea"
        let all = list_all_profiles();
        let lea_count = all.iter().filter(|p| p.persona_name == "Lea").count();
        assert_eq!(lea_count, 1, "Only entrepreneur should be named Lea");
    }

    #[test]
    fn game_developer_has_default_engine() {
        let p = get_profile("game-developer").expect("game-developer must exist");
        assert!(p.default_engine.is_some(), "Game Developer must have default_engine");
    }

    #[test]
    fn qa_engineer_is_technical_category() {
        let p = get_profile("qa-engineer").expect("qa-engineer must exist");
        assert_eq!(p.category, ProfileCategory::Technical,
            "QA Engineer should be Technical, not Business");
    }

    #[test]
    fn multi_profile_synthesis_label_is_english() {
        // The synthesis label should be language-neutral (English)
        let prompt = build_profiles_prompt(&["architect".into(), "tech-lead".into()]);
        assert!(prompt.contains("Synthesis"), "Multi-profile prompt should use 'Synthesis' (English)");
        assert!(!prompt.contains("Synthese"), "Should NOT use French 'Synthese'");
    }

    #[test]
    fn get_profile_found() {
        let profile = get_profile("architect");
        assert!(profile.is_some());
        assert_eq!(profile.unwrap().name, "Architect");
    }

    #[test]
    fn get_profile_not_found() {
        let profile = get_profile("nonexistent-profile");
        assert!(profile.is_none());
    }

    #[test]
    fn build_profile_prompt_found() {
        let prompt = build_profile_prompt("architect");
        assert!(prompt.contains("=== Agent Profile: Architect"));
        assert!(prompt.contains("Software Architect"));
    }

    #[test]
    fn build_profile_prompt_not_found() {
        let prompt = build_profile_prompt("nonexistent");
        assert!(prompt.is_empty());
    }

    #[test]
    fn parse_frontmatter_valid() {
        let raw = "---\nname: Test\nrole: Tester\navatar: 🧪\ncolor: \"#ff0000\"\ncategory: technical\nbuiltin: false\n---\nYou are a test agent.";
        let profile = parse_profile_markdown("test", raw, false).unwrap();
        assert_eq!(profile.name, "Test");
        assert_eq!(profile.role, "Tester");
        assert_eq!(profile.avatar, "🧪");
        assert_eq!(profile.color, "#ff0000");
        assert_eq!(profile.category, ProfileCategory::Technical);
        assert_eq!(profile.persona_prompt, "You are a test agent.");
        assert!(!profile.is_builtin);
    }

    #[test]
    fn parse_frontmatter_with_default_engine() {
        let raw = "---\nname: X\nrole: R\navatar: A\ncolor: \"#000\"\ncategory: meta\ndefault_engine: claude-code\n---\ncontent";
        let profile = parse_profile_markdown("x", raw, false).unwrap();
        assert_eq!(profile.default_engine, Some("claude-code".to_string()));
    }

    #[test]
    fn parse_frontmatter_missing_yields_none() {
        let raw = "No frontmatter here.";
        assert!(parse_profile_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn parse_frontmatter_no_name_yields_none() {
        let raw = "---\nrole: R\navatar: A\ncolor: \"#000\"\ncategory: meta\n---\ncontent";
        assert!(parse_profile_markdown("bad", raw, false).is_none());
    }

    #[test]
    fn delete_builtin_profile_rejected() {
        let result = delete_custom_profile("architect");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("builtin"));
    }
}
