//! Native skill & profile file sync — writes SKILL.md and agent files
//! to project directories for all 5 agent platforms.
//!
//! Pattern mirrors `mcp_scanner.rs`: write per-agent-format files to disk,
//! triggered at startup + config change. Agents then discover them natively
//! (progressive disclosure) instead of receiving full content via prompt.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::models::{AgentProfile, AgentType, Skill};

// ─── Ownership ledger ────────────────────────────────────────────────────────

/// What Kronn wrote into a project, by relative path, with the digest of the
/// bytes it wrote. Only a listed, unmodified, untracked file is ever removed:
/// a repository's own skills and agents are never Kronn's to delete.
const LEDGER_PATH: &str = ".kronn/native-files.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    files: BTreeMap<String, String>,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn load_ledger(root: &Path) -> Ledger {
    std::fs::read(root.join(LEDGER_PATH))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_ledger(root: &Path, ledger: &Ledger) {
    let path = root.join(LEDGER_PATH);
    let Ok(content) = serde_json::to_string_pretty(ledger) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = crate::core::mcp_scanner::atomic_write(&path, &content) {
        tracing::warn!("Cannot write {}: {}", path.display(), e);
    }
}

/// Whether git tracks `rel` in the repository at `root`. Outside a repository,
/// or without git, nothing is tracked.
fn is_tracked(root: &Path, rel: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", rel])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A file Kronn may write: absent, or already its own. A tracked or foreign
/// file with the same name is left as the repository has it.
fn may_write(root: &Path, rel: &str, ledger: &Ledger) -> bool {
    let path = root.join(rel);
    match std::fs::symlink_metadata(&path) {
        Err(_) => true,
        Ok(meta) if meta.file_type().is_symlink() => false,
        Ok(_) => ledger.files.contains_key(rel) && !is_tracked(root, rel),
    }
}

fn write_owned(root: &Path, rel: &str, content: &str, ledger: &mut Ledger) -> bool {
    if !may_write(root, rel, ledger) {
        tracing::info!("Kept {}: not written by Kronn", root.join(rel).display());
        return false;
    }
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("Cannot create {}: {}", parent.display(), e);
            return false;
        }
    }
    if let Err(e) = crate::core::mcp_scanner::atomic_write(&path, content) {
        tracing::warn!("Cannot write {}: {}", path.display(), e);
        return false;
    }
    ledger
        .files
        .insert(rel.to_string(), digest(content.as_bytes()));
    true
}

/// Remove `rel` only when Kronn wrote it, it still holds those bytes, it is
/// not a symlink and git does not track it. It leaves the ledger either way.
fn remove_owned(root: &Path, rel: &str, ledger: &mut Ledger) {
    let Some(written) = ledger.files.remove(rel) else {
        return;
    };
    let path = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return;
    };
    let unchanged =
        meta.is_file() && std::fs::read(&path).is_ok_and(|bytes| digest(&bytes) == written);
    if unchanged && !is_tracked(root, rel) {
        let _ = std::fs::remove_file(&path);
        tracing::info!("Removed stale native file: {}", path.display());
    } else {
        tracing::info!("Kept {}: edited, linked or tracked", path.display());
    }
}

// ─── Directory mappings ──────────────────────────────────────────────────────

/// Returns the skill directory path for a given agent type (relative to project root).
fn skill_dir(agent: &AgentType) -> Option<&'static str> {
    match agent {
        AgentType::ClaudeCode => Some(".claude/skills"),
        AgentType::Codex => Some(".agents/skills"),
        AgentType::Vibe => Some(".vibe/skills"),
        AgentType::Kiro => Some(".kiro/skills"),
        AgentType::GeminiCli => Some(".gemini/skills"),
        _ => None,
    }
}

/// Returns the agent/profile file directory for a given agent type.
fn profile_dir(agent: &AgentType) -> Option<&'static str> {
    match agent {
        AgentType::ClaudeCode => Some(".claude/agents"),
        AgentType::GeminiCli => Some(".gemini/agents"),
        AgentType::Codex => Some(".codex/agents"),
        AgentType::CopilotCli => Some(".copilot/agents"),
        AgentType::Kiro => Some(".kiro/steering"),
        _ => None, // Vibe has no project-level agent file support
    }
}

/// File extension for profile files per agent.
fn profile_ext(agent: &AgentType) -> &'static str {
    match agent {
        AgentType::Codex => ".toml",
        _ => ".md",
    }
}

// ─── Skill renderers ─────────────────────────────────────────────────────────

/// Render optional agentskills.io frontmatter fields (license, allowed-tools).
fn render_skill_optional_fields(skill: &Skill) -> String {
    let mut extra = String::new();
    if let Some(ref license) = skill.license {
        extra.push_str(&format!("license: {}\n", license));
    }
    if let Some(ref tools) = skill.allowed_tools {
        extra.push_str(&format!("allowed-tools: {}\n", tools));
    }
    extra
}

/// Render a SKILL.md for Claude Code (agentskills.io compliant).
fn render_skill_claude(skill: &Skill) -> String {
    format!(
        "---\nname: {}\ndescription: {}\nuser-invocable: true\n{}---\n\n{}",
        slug(&skill.id),
        skill.description,
        render_skill_optional_fields(skill),
        skill.content,
    )
}

/// Render a SKILL.md for Codex / Gemini / Kiro (minimal + optional fields).
fn render_skill_codex(skill: &Skill) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n{}---\n\n{}",
        slug(&skill.id),
        skill.description,
        render_skill_optional_fields(skill),
        skill.content,
    )
}

/// Render a SKILL.md for Vibe.
fn render_skill_vibe(skill: &Skill) -> String {
    format!(
        "---\nname: {}\ndescription: {}\nuser-invocable: true\n{}---\n\n{}",
        slug(&skill.id),
        skill.description,
        render_skill_optional_fields(skill),
        skill.content,
    )
}

/// Render a SKILL.md for Kiro.
fn render_skill_kiro(skill: &Skill) -> String {
    render_skill_codex(skill)
}

/// Render a SKILL.md for Gemini CLI.
fn render_skill_gemini(skill: &Skill) -> String {
    render_skill_codex(skill)
}

fn render_skill(agent: &AgentType, skill: &Skill) -> String {
    match agent {
        AgentType::ClaudeCode => render_skill_claude(skill),
        AgentType::Codex => render_skill_codex(skill),
        AgentType::Vibe => render_skill_vibe(skill),
        AgentType::Kiro => render_skill_kiro(skill),
        AgentType::GeminiCli => render_skill_gemini(skill),
        _ => String::new(),
    }
}

// ─── Profile renderers ───────────────────────────────────────────────────────

/// Render a Claude Code agent file (.claude/agents/{name}.md).
fn render_profile_claude(profile: &AgentProfile) -> String {
    format!(
        "---\nname: {}\ndescription: {} — {}\nmodel: inherit\n---\n\n{}",
        slug(&profile.id),
        profile.name,
        profile.role,
        profile.persona_prompt,
    )
}

/// Render a Gemini CLI agent file (.gemini/agents/{name}.md).
fn render_profile_gemini(profile: &AgentProfile) -> String {
    format!(
        "---\nname: {}\ndescription: {} — {}\n---\n\n{}",
        slug(&profile.id),
        profile.name,
        profile.role,
        profile.persona_prompt,
    )
}

/// Render a Codex agent file (.codex/agents/{name}.toml).
fn render_profile_codex(profile: &AgentProfile) -> String {
    // Escape TOML multiline strings
    let instructions = profile
        .persona_prompt
        .replace("\\", "\\\\")
        .replace("\"\"\"", "\\\"\\\"\\\"");
    format!(
        "# Auto-generated by Kronn — do not edit manually\n\
         name = \"{}\"\n\
         description = \"{} — {}\"\n\
         developer_instructions = \"\"\"\n{}\n\"\"\"\n",
        slug(&profile.id),
        profile.name,
        profile.role,
        instructions,
    )
}

/// Render a Copilot CLI agent file (.copilot/agents/{name}.md).
fn render_profile_copilot(profile: &AgentProfile) -> String {
    format!(
        "---\nname: {}\ndescription: {} — {}\n---\n\n{}",
        slug(&profile.id),
        profile.name,
        profile.role,
        profile.persona_prompt,
    )
}

/// Render a Kiro steering file (.kiro/steering/{name}.md).
fn render_profile_kiro(profile: &AgentProfile) -> String {
    format!(
        "---\ninclusion: auto\nname: {}\ndescription: {} — {}\n---\n\n{}",
        slug(&profile.id),
        profile.name,
        profile.role,
        profile.persona_prompt,
    )
}

fn render_profile(agent: &AgentType, profile: &AgentProfile) -> Option<String> {
    match agent {
        AgentType::ClaudeCode => Some(render_profile_claude(profile)),
        AgentType::GeminiCli => Some(render_profile_gemini(profile)),
        AgentType::Codex => Some(render_profile_codex(profile)),
        AgentType::CopilotCli => Some(render_profile_copilot(profile)),
        AgentType::Kiro => Some(render_profile_kiro(profile)),
        _ => None, // Vibe: no native agent files
    }
}

// ─── Native support detection ────────────────────────────────────────────────

/// Returns true if this agent type discovers SKILL.md files natively
/// when launched by Kronn (i.e., in the mode Kronn uses to spawn it).
///
/// - Claude Code: `claude --print` starts a full session → discovers `.claude/skills/`
/// - Codex: `codex exec` → discovers `.agents/skills/`
/// - Gemini CLI: `gemini -p` → discovers `.gemini/skills/`
/// - Vibe: Kronn uses a custom Python script (run_programmatic), NOT the CLI → NO discovery
/// - Kiro: IDE-focused, headless skill discovery NOT confirmed
pub fn supports_native_skills(agent: &AgentType) -> bool {
    matches!(
        agent,
        AgentType::ClaudeCode | AgentType::Codex | AgentType::GeminiCli
    )
}

/// Returns true if this agent type discovers agent/profile files natively.
pub fn supports_native_profiles(agent: &AgentType) -> bool {
    matches!(
        agent,
        AgentType::ClaudeCode | AgentType::GeminiCli | AgentType::Codex | AgentType::CopilotCli
    )
}

// ─── Sync functions ──────────────────────────────────────────────────────────

/// Agents to sync native skill files for (only those that discover them).
const SKILL_SYNC_AGENTS: &[AgentType] = &[
    AgentType::ClaudeCode,
    AgentType::Codex,
    AgentType::GeminiCli,
    // Vibe: Kronn bypasses CLI → won't discover project-local skills
    // Kiro: IDE-only, headless discovery unconfirmed
];

/// Agents to sync native profile/agent files for.
const PROFILE_SYNC_AGENTS: &[AgentType] = &[
    AgentType::ClaudeCode,
    AgentType::GeminiCli,
    AgentType::Codex,
    AgentType::CopilotCli,
    // Kiro steering: unconfirmed in headless
];

/// Sync native skill + profile files for a project (additive — no cleanup).
/// Use this from `runner.rs` before spawning an agent: it only ADDS files,
/// never removes, so parallel discussions don't delete each other's skills.
pub fn sync_project_native_files(
    project_path: &str,
    skill_ids: &[String],
    profile_ids: &[String],
) -> Result<(), String> {
    sync_impl(project_path, skill_ids, profile_ids, false)
}

/// Full sync with cleanup: writes selected skills/profiles AND removes stale ones.
/// Use this at startup and when project default skills change — NOT from runner.
pub fn sync_project_native_files_full(
    project_path: &str,
    skill_ids: &[String],
    profile_ids: &[String],
) -> Result<(), String> {
    sync_impl(project_path, skill_ids, profile_ids, true)
}

fn sync_impl(
    project_path: &str,
    skill_ids: &[String],
    profile_ids: &[String],
    cleanup: bool,
) -> Result<(), String> {
    if project_path.is_empty() {
        return Ok(()); // No project context (general discussion)
    }

    let resolved = resolve_host_path(project_path);
    let root = Path::new(&resolved);
    if !root.exists() {
        return Err(format!("Project path not found: {}", resolved));
    }

    // Load skills and profiles
    let skills = crate::core::skills::get_skills_by_ids(skill_ids);
    let profiles: Vec<AgentProfile> = profile_ids
        .iter()
        .filter_map(|id| crate::core::profiles::get_profile(id))
        .collect();

    let skill_slugs: Vec<String> = skills.iter().map(|s| slug(&s.id)).collect();
    let profile_slugs: Vec<String> = profiles.iter().map(|p| slug(&p.id)).collect();
    let mut ledger = load_ledger(root);

    // ── Skills: only for agents that discover SKILL.md natively ──
    for agent in SKILL_SYNC_AGENTS {
        let Some(dir) = skill_dir(agent) else {
            continue;
        };

        for skill in &skills {
            let skill_slug = slug(&skill.id);
            let skill_rel = format!("{dir}/{skill_slug}");
            let content = render_skill(agent, skill);
            if write_owned(
                root,
                &format!("{skill_rel}/SKILL.md"),
                &content,
                &mut ledger,
            ) {
                // Ignored from inside the folder Kronn owns, so the repository's
                // own `.gitignore` never gains a rule over its tracked skills.
                write_owned(root, &format!("{skill_rel}/.gitignore"), "*\n", &mut ledger);
            }
        }

        // Only cleanup stale files during full sync (startup / project config change)
        if cleanup {
            cleanup_stale_dirs(root, dir, &skill_slugs, &mut ledger);
        }
    }

    // ── Profiles → Agent files: only for agents that discover them natively ──
    for agent in PROFILE_SYNC_AGENTS {
        let Some(dir) = profile_dir(agent) else {
            continue;
        };
        let ext = profile_ext(agent);

        for profile in &profiles {
            if let Some(content) = render_profile(agent, profile) {
                let rel = format!("{dir}/{}{ext}", slug(&profile.id));
                if write_owned(root, &rel, &content, &mut ledger)
                    && !gitignore_negates_under(root, dir)
                {
                    // Only the file Kronn wrote, never the agent's whole folder.
                    crate::core::mcp_scanner::ensure_gitignore_public(
                        project_path,
                        &format!("/{rel}"),
                    );
                }
            }
        }

        // Only cleanup stale files during full sync
        if cleanup {
            cleanup_stale_files(root, dir, ext, &profile_slugs, &mut ledger);
        }
    }

    if cleanup {
        repair_folder_ignores(root);
    }
    save_ledger(root, &ledger);

    if !skills.is_empty() || !profiles.is_empty() {
        tracing::info!(
            "Synced native files for {} (cleanup={}): {} skills, {} profiles",
            project_path,
            cleanup,
            skills.len(),
            profiles.len()
        );
    }

    Ok(())
}

// ─── Detection helpers (used by runner to decide injection vs reference) ─────

/// Check if native SKILL.md files exist for this agent type in the project.
pub fn has_native_skills(project_path: &str, agent: &AgentType) -> bool {
    let Some(dir) = skill_dir(agent) else {
        return false;
    };
    let resolved = resolve_host_path(project_path);
    let skill_root = Path::new(&resolved).join(dir);
    skill_root.is_dir() && has_skill_md_files(&skill_root)
}

/// Check if native agent/profile files exist for this agent type.
pub fn has_native_profiles(project_path: &str, agent: &AgentType) -> bool {
    let Some(dir) = profile_dir(agent) else {
        return false;
    };
    let ext = profile_ext(agent);
    let resolved = resolve_host_path(project_path);
    let agent_root = Path::new(&resolved).join(dir);
    if !agent_root.is_dir() {
        return false;
    }

    // Check for at least one file with the right extension containing Kronn marker
    std::fs::read_dir(&agent_root).ok().is_some_and(|entries| {
        entries.filter_map(|e| e.ok()).any(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(ext)
        })
    })
}

/// Build a lightweight reference prompt for skills that exist as native files.
/// The agent discovers all installed SKILL.md files automatically — this prompt
/// just tells it which ones to prioritize for the current task.
/// ~25 tokens instead of ~500-800 for full injection.
pub fn build_skills_reference_prompt(skill_ids: &[String]) -> String {
    let skills = crate::core::skills::get_skills_by_ids(skill_ids);
    if skills.is_empty() {
        return String::new();
    }

    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    format!(
        "For this task, prioritize your {} skills.",
        names.join(", ")
    )
}

// ─── Cleanup ─────────────────────────────────────────────────────────────────

/// Remove the skill folders Kronn wrote whose skill is no longer selected.
/// Only ledger entries are considered: a folder Kronn never wrote is not even
/// looked at, and one holding other files keeps them.
fn cleanup_stale_dirs(root: &Path, dir: &str, active_slugs: &[String], ledger: &mut Ledger) {
    let prefix = format!("{dir}/");
    let stale: Vec<String> = ledger
        .files
        .keys()
        .filter(|rel| {
            rel.strip_prefix(&prefix)
                .and_then(|rest| rest.split_once('/'))
                .is_some_and(|(slug, _)| !active_slugs.iter().any(|active| active == slug))
        })
        .cloned()
        .collect();
    let mut folders = std::collections::BTreeSet::new();
    for rel in stale {
        remove_owned(root, &rel, ledger);
        if let Some((folder, _)) = rel.rsplit_once('/') {
            folders.insert(folder.to_string());
        }
    }
    for folder in folders {
        // Only succeeds once the folder is empty.
        let _ = std::fs::remove_dir(root.join(folder));
    }
}

/// Remove the agent files Kronn wrote whose profile is no longer selected.
fn cleanup_stale_files(
    root: &Path,
    dir: &str,
    ext: &str,
    active_slugs: &[String],
    ledger: &mut Ledger,
) {
    let prefix = format!("{dir}/");
    let stale: Vec<String> = ledger
        .files
        .keys()
        .filter(|rel| {
            rel.strip_prefix(&prefix)
                .and_then(|name| name.strip_suffix(ext))
                .is_some_and(|stem| {
                    !stem.contains('/') && !active_slugs.iter().any(|active| active == stem)
                })
        })
        .cloned()
        .collect();
    for rel in stale {
        remove_owned(root, &rel, ledger);
    }
}

/// Folders whose whole-folder ignore rule earlier syncs appended. `.gemini/`
/// and `.kiro/` are not listed: MCP settings holding credentials live there.
const LEGACY_FOLDER_IGNORES: &[&str] = &[".agents", ".claude", ".codex", ".copilot", ".vibe"];

fn gitignore_lines(root: &Path) -> Option<Vec<String>> {
    let path = root.join(".gitignore");
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .map(|content| content.lines().map(str::to_string).collect())
}

/// Whether the repository re-includes something under `dir`, e.g.
/// `!.agents/skills/`: an ignore rule added after it would cancel that.
fn gitignore_negates_under(root: &Path, dir: &str) -> bool {
    let top = dir.split('/').next().unwrap_or(dir);
    gitignore_lines(root).is_some_and(|lines| {
        lines.iter().any(|line| {
            let rule = line.trim().trim_start_matches('!').trim_start_matches('/');
            line.trim().starts_with('!') && (rule == top || rule.starts_with(&format!("{top}/")))
        })
    })
}

/// Drop a whole-folder rule (`.agents/`) that an earlier sync appended when the
/// repository re-includes something under that folder: the rule made its
/// tracked skills invisible to git.
fn repair_folder_ignores(root: &Path) {
    let Some(lines) = gitignore_lines(root) else {
        return;
    };
    let conflicting: Vec<&str> = LEGACY_FOLDER_IGNORES
        .iter()
        .copied()
        .filter(|top| {
            lines.iter().any(|line| line.trim() == format!("{top}/"))
                && gitignore_negates_under(root, top)
        })
        .collect();
    if conflicting.is_empty() {
        return;
    }
    let kept: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| {
            !conflicting
                .iter()
                .any(|top| line.trim() == format!("{top}/"))
        })
        .collect();
    let mut content = kept.join("\n");
    content.push('\n');
    if let Err(e) = crate::core::mcp_scanner::atomic_write(&root.join(".gitignore"), &content) {
        tracing::warn!("Cannot repair {}/.gitignore: {}", root.display(), e);
    } else {
        tracing::info!(
            "Removed {:?} from {}/.gitignore: it hid re-included files",
            conflicting,
            root.display()
        );
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Convert an ID to an agentskills.io-compliant slug.
/// Rules: lowercase a-z + 0-9 + hyphens, no leading/trailing/consecutive hyphens, max 64 chars.
pub fn slug(id: &str) -> String {
    let raw: String = id
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    // Collapse consecutive hyphens
    let mut result = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c == '-' && result.ends_with('-') {
            continue;
        }
        result.push(c);
    }
    // Strip leading/trailing hyphens, truncate to 64 chars
    let trimmed = result.trim_matches('-');
    if trimmed.len() > 64 {
        trimmed[..64].trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

/// Check if a directory contains at least one SKILL.md file (in any subdirectory).
fn has_skill_md_files(dir: &Path) -> bool {
    std::fs::read_dir(dir).ok().is_some_and(|entries| {
        entries.filter_map(|e| e.ok()).any(|e| {
            e.file_type().is_ok_and(|ft| ft.is_dir()) && e.path().join("SKILL.md").exists()
        })
    })
}

fn resolve_host_path(path: &str) -> String {
    crate::core::scanner::resolve_host_path(path)
        .to_string_lossy()
        .to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(name: &str) -> PathBuf {
        let tmp =
            std::env::temp_dir().join(format!("kronn-native-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    fn sample_skill() -> Skill {
        Skill {
            id: "rust".into(),
            name: "Rust".into(),
            description: "Systems programming with ownership and zero-cost abstractions".into(),
            icon: "🦀".into(),
            category: crate::models::SkillCategory::Language,
            content: "Expert Rust knowledge. Prefer references over cloning.".into(),
            is_builtin: true,
            token_estimate: 150,
            license: None,
            allowed_tools: None,
            auto_triggers: None,
            external: false,
            source_url: None,
        }
    }

    fn sample_profile() -> AgentProfile {
        AgentProfile {
            id: "architect".into(),
            name: "Architect".into(),
            persona_name: "Kai".into(),
            role: "Software Architect".into(),
            avatar: "🏗️".into(),
            color: "#4d9fff".into(),
            category: crate::models::ProfileCategory::Technical,
            persona_prompt: "You are a senior software architect.".into(),
            default_engine: None,
            is_builtin: true,
            token_estimate: 200,
        }
    }

    // ── Skill rendering tests ──

    #[test]
    fn render_skill_claude_has_user_invocable() {
        let skill = sample_skill();
        let content = render_skill_claude(&skill);
        assert!(content.contains("user-invocable: true"));
        assert!(content.contains("name: rust"));
        assert!(content.contains("Expert Rust knowledge"));
    }

    #[test]
    fn render_skill_codex_minimal_frontmatter() {
        let skill = sample_skill();
        let content = render_skill_codex(&skill);
        assert!(content.contains("name: rust"));
        assert!(content.contains("description:"));
        assert!(!content.contains("user-invocable")); // Codex doesn't use this
    }

    #[test]
    fn render_skill_vibe_has_user_invocable() {
        let skill = sample_skill();
        let content = render_skill_vibe(&skill);
        assert!(content.contains("user-invocable: true"));
        assert!(content.contains("name: rust"));
    }

    // ── Profile rendering tests ──

    #[test]
    fn render_profile_claude_has_model_inherit() {
        let profile = sample_profile();
        let content = render_profile_claude(&profile);
        assert!(content.contains("model: inherit"));
        assert!(content.contains("name: architect"));
        assert!(content.contains("You are a senior software architect"));
    }

    #[test]
    fn render_profile_codex_toml_format() {
        let profile = sample_profile();
        let content = render_profile_codex(&profile);
        assert!(content.contains("name = \"architect\""));
        assert!(content.contains("developer_instructions"));
        assert!(content.contains("You are a senior software architect"));
    }

    #[test]
    fn render_profile_kiro_has_inclusion_auto() {
        let profile = sample_profile();
        let content = render_profile_kiro(&profile);
        assert!(content.contains("inclusion: auto"));
        assert!(content.contains("name: architect"));
    }

    #[test]
    fn render_profile_copilot_md_format() {
        let profile = sample_profile();
        let content = render_profile_copilot(&profile);
        assert!(content.contains("name: architect"));
        assert!(content.contains("description: Architect — Software Architect"));
        assert!(content.contains("You are a senior software architect"));
        // Copilot uses same .md format as Gemini (no model: inherit)
        assert!(!content.contains("model:"));
    }

    #[test]
    fn copilot_in_profile_sync_agents() {
        assert!(
            PROFILE_SYNC_AGENTS.contains(&AgentType::CopilotCli),
            "CopilotCli must be in PROFILE_SYNC_AGENTS"
        );
        assert!(
            supports_native_profiles(&AgentType::CopilotCli),
            "CopilotCli must support native profiles"
        );
        assert_eq!(
            profile_dir(&AgentType::CopilotCli),
            Some(".copilot/agents"),
            "CopilotCli profile dir must be .copilot/agents"
        );
    }

    #[test]
    fn render_profile_vibe_returns_none() {
        let profile = sample_profile();
        assert!(render_profile(&AgentType::Vibe, &profile).is_none());
    }

    // ── Slug tests (agentskills.io spec) ──

    #[test]
    fn slug_normalizes() {
        assert_eq!(slug("Rust"), "rust");
        assert_eq!(slug("data_engineering"), "data-engineering");
        assert_eq!(slug("API Design!"), "api-design");
    }

    #[test]
    fn slug_collapses_consecutive_hyphens() {
        assert_eq!(slug("C++ Expert"), "c-expert");
        assert_eq!(slug("My -- Skill"), "my-skill");
        assert_eq!(slug("a---b"), "a-b");
    }

    #[test]
    fn slug_strips_leading_trailing_hyphens() {
        assert_eq!(slug("--test--"), "test");
        assert_eq!(slug("-abc-"), "abc");
    }

    #[test]
    fn slug_truncates_to_64_chars() {
        let long = "a".repeat(100);
        let result = slug(&long);
        assert!(
            result.len() <= 64,
            "Slug must be max 64 chars, got {}",
            result.len()
        );
    }

    #[test]
    fn slug_empty_input() {
        assert_eq!(slug("!!!"), "");
        assert_eq!(slug("---"), "");
    }

    // ── agentskills.io optional fields ──

    #[test]
    fn render_skill_without_optional_fields() {
        let skill = sample_skill();
        let content = render_skill_claude(&skill);
        assert!(!content.contains("license:"));
        assert!(!content.contains("allowed-tools:"));
    }

    #[test]
    fn render_skill_with_license_and_allowed_tools() {
        let mut skill = sample_skill();
        skill.license = Some("MIT".into());
        skill.allowed_tools = Some("Bash Read Grep".into());
        let content = render_skill_claude(&skill);
        assert!(content.contains("license: MIT"));
        assert!(content.contains("allowed-tools: Bash Read Grep"));
        assert!(content.contains("user-invocable: true"));
    }

    #[test]
    fn render_skill_codex_with_optional_fields() {
        let mut skill = sample_skill();
        skill.license = Some("Apache-2.0".into());
        let content = render_skill_codex(&skill);
        assert!(content.contains("license: Apache-2.0"));
        assert!(!content.contains("user-invocable"));
    }

    // ── Reference prompt tests ──

    #[test]
    fn reference_prompt_with_known_skills() {
        // Uses real builtin skills (names are now lowercase slugs per agentskills.io spec)
        let ids = vec!["rust".into(), "security".into()];
        let prompt = build_skills_reference_prompt(&ids);
        assert!(prompt.contains("rust"));
        assert!(prompt.contains("security"));
        assert!(prompt.contains("prioritize"));
        // Must be short (< 100 chars)
        assert!(
            prompt.len() < 100,
            "Reference prompt too long: {} chars",
            prompt.len()
        );
    }

    #[test]
    fn reference_prompt_empty_skills() {
        let prompt = build_skills_reference_prompt(&[]);
        assert!(prompt.is_empty());
    }

    // ── Cleanup tests ──

    fn git(repo: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// A repository that versions a skill and an agent file of its own, and
    /// re-includes `.agents/skills/` under an `.agents/*` rule.
    fn repo_with_own_skill(name: &str) -> PathBuf {
        let repo = tmp_dir(name);
        assert!(git(&repo, &["init", "-q"]));
        std::fs::write(
            repo.join(".gitignore"),
            ".agents/*\n!.agents/skills/\n.gemini/\n",
        )
        .unwrap();
        for rel in [
            ".agents/skills/mine/SKILL.md",
            ".claude/skills/mine/SKILL.md",
            ".claude/agents/mine.md",
        ] {
            let path = repo.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "# The repository's own\n").unwrap();
        }
        assert!(git(&repo, &["add", "-A"]));
        assert!(git(&repo, &["commit", "-q", "-m", "own skills"]));
        repo
    }

    fn sync(repo: &Path, skills: &[&str], profiles: &[&str], full: bool) {
        let skills: Vec<String> = skills.iter().map(|id| id.to_string()).collect();
        let profiles: Vec<String> = profiles.iter().map(|id| id.to_string()).collect();
        let path = repo.to_string_lossy();
        if full {
            sync_project_native_files_full(&path, &skills, &profiles).unwrap();
        } else {
            sync_project_native_files(&path, &skills, &profiles).unwrap();
        }
    }

    #[test]
    fn a_repository_keeps_its_versioned_skills_and_agents_through_every_sync() {
        let repo = repo_with_own_skill("own-skills");
        // A restart with no default skill, a change of the defaults, then back.
        sync(&repo, &[], &[], true);
        sync(&repo, &["accessibility"], &["architect"], true);
        sync(&repo, &["api-design"], &["tech-lead"], true);
        sync(&repo, &[], &[], true);

        for rel in [
            ".agents/skills/mine/SKILL.md",
            ".claude/skills/mine/SKILL.md",
            ".claude/agents/mine.md",
        ] {
            assert!(repo.join(rel).exists(), "{rel} must survive the sync");
        }
        // Only `.gitignore` may change: it gains the exact paths Kronn wrote.
        assert!(
            git(
                &repo,
                &["diff", "--quiet", "HEAD", "--", ".agents", ".claude"]
            ),
            "no tracked skill or agent file was touched"
        );
    }

    #[test]
    fn a_deselected_kronn_skill_is_removed_and_a_foreign_folder_never_is() {
        let repo = repo_with_own_skill("deselect");
        let foreign = repo.join(".claude/skills/untracked-mine/SKILL.md");
        std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        std::fs::write(&foreign, "# Mine, not committed yet\n").unwrap();

        sync(
            &repo,
            &["accessibility", "api-design"],
            &["architect"],
            false,
        );
        assert!(repo.join(".claude/skills/accessibility/SKILL.md").exists());
        assert!(repo.join(".claude/agents/architect.md").exists());
        // The reader edits one of Kronn's files: it is no longer Kronn's to delete.
        std::fs::write(
            repo.join(".agents/skills/api-design/SKILL.md"),
            "# edited\n",
        )
        .unwrap();

        sync(&repo, &[], &[], true);
        assert!(
            !repo.join(".claude/skills/accessibility").exists(),
            "Kronn's folder is gone"
        );
        assert!(
            !repo.join(".claude/agents/architect.md").exists(),
            "Kronn's agent file is gone"
        );
        assert!(
            repo.join(".agents/skills/api-design/SKILL.md").exists(),
            "an edited file stays"
        );
        assert!(foreign.exists(), "a folder Kronn never wrote stays");
    }

    #[test]
    fn the_sync_never_ignores_what_the_repository_re_includes() {
        let repo = repo_with_own_skill("ignore");
        // An earlier sync appended a whole-folder rule after the re-inclusion.
        let mut gitignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
        gitignore.push_str(".agents/\n");
        std::fs::write(repo.join(".gitignore"), gitignore).unwrap();

        sync(&repo, &["accessibility"], &["architect"], true);

        let gitignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
        assert!(
            !gitignore.lines().any(|line| line.trim() == ".agents/"),
            "{gitignore}"
        );
        assert!(
            !gitignore.lines().any(|line| line.trim() == ".claude/"),
            "{gitignore}"
        );
        assert!(
            gitignore.lines().any(|line| line.trim() == ".gemini/"),
            "credentials stay ignored"
        );
        let ignored = |rel: &str| git(&repo, &["check-ignore", "-q", rel]);
        std::fs::create_dir_all(repo.join(".agents/skills/new")).unwrap();
        std::fs::write(repo.join(".agents/skills/new/SKILL.md"), "# new\n").unwrap();
        assert!(
            !ignored(".agents/skills/new/SKILL.md"),
            "a new repository skill is not ignored"
        );
        assert!(!ignored(".claude/agents/mine.md"));
        // What Kronn wrote is ignored from its own folder or by its exact path.
        assert!(ignored(".agents/skills/accessibility/SKILL.md"));
        assert!(ignored(".claude/agents/architect.md"));
    }

    #[test]
    fn a_tracked_file_named_like_a_kronn_skill_is_never_overwritten() {
        let repo = repo_with_own_skill("collision");
        let own = repo.join(".claude/skills/accessibility/SKILL.md");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "# The repository's accessibility skill\n").unwrap();
        assert!(git(&repo, &["add", "-A"]));
        assert!(git(&repo, &["commit", "-q", "-m", "collision"]));

        sync(&repo, &["accessibility"], &[], true);
        sync(&repo, &[], &[], true);
        assert_eq!(
            std::fs::read_to_string(&own).unwrap(),
            "# The repository's accessibility skill\n"
        );
    }

    // ── has_native_skills / has_skill_md_files tests ──

    #[test]
    fn has_skill_md_files_true_when_exists() {
        let tmp = tmp_dir("has-skill");
        let skill_dir = tmp.join("rust");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "test").unwrap();

        assert!(has_skill_md_files(&tmp));
    }

    #[test]
    fn has_skill_md_files_false_when_empty() {
        let tmp = tmp_dir("no-skill");
        assert!(!has_skill_md_files(&tmp));
    }
}
