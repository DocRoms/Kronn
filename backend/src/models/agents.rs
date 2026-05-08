// Agent capability definitions: Skills (WHAT — domain expertise),
// Profiles (WHO — persona), Directives (HOW — output behavior).
//
// All three are user-pickable in the discussion launcher and stack
// onto the agent's system prompt at run time.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─── Skills (multi-select) ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SkillCategory {
    Language,
    Domain,
    Business,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: SkillCategory,
    pub content: String,
    pub is_builtin: bool,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
    /// agentskills.io: SPDX license identifier or reference to bundled LICENSE file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// agentskills.io: space-delimited list of pre-approved tools (e.g. "Bash Read Grep").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    /// Optional auto-activation trigger regexes keyed by locale. When
    /// the user types a message matching one of these patterns, the
    /// frontend auto-adds this skill to the current discussion. The
    /// `common` entry always applies; the locale-specific entries
    /// apply when the discussion's language matches. See the YAML
    /// frontmatter convention in `backend/src/skills/kronn-docs.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_triggers: Option<AutoTriggers>,
    /// 0.7+ — true when the skill content was vendored from a third-party
    /// open-source project (see `THIRD_PARTY_SKILLS.md` at repo root).
    /// The frontend renders a "🔗 External" badge to make attribution
    /// visible in-app. Set via the `external: true` frontmatter field on
    /// builtin skills under `backend/src/skills/external/`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
    /// 0.7+ — when `external` is true, points to the upstream project
    /// (clickable in the UI for attribution). Set via the `source_url`
    /// frontmatter field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// Auto-trigger regex buckets declared in a skill's frontmatter YAML.
///
/// ```yaml
/// auto_triggers:
///   common:
///     - "\\b(pdf|docx?|xlsx?)\\b"
///   fr:
///     - "génér.+(fichier|rapport)"
///   en:
///     - "generate.+(file|report)"
/// ```
///
/// The frontend combines `common` + the entry matching the discussion
/// language (or `en` as fallback) into a single regex list, and tests
/// every pattern against the pending message.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoTriggers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub common: Vec<String>,
    /// Per-locale patterns keyed by IETF language tag (`fr`, `en`, `es`,
    /// ...). Additional locales can be added without a code change.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string[]>")]
    pub locales: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateSkillRequest {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: SkillCategory,
    pub content: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
}

// ─── Agent Profiles (single-select) ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProfileCategory {
    Technical,
    Business,
    Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub persona_name: String,
    pub role: String,
    pub avatar: String,
    pub color: String,
    pub category: ProfileCategory,
    pub persona_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_engine: Option<String>,
    pub is_builtin: bool,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(default)]
    pub persona_name: String,
    pub role: String,
    pub avatar: String,
    pub color: String,
    pub category: ProfileCategory,
    pub persona_prompt: String,
    #[serde(default)]
    pub default_engine: Option<String>,
}

// ─── Directives (multi-select) ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DirectiveCategory {
    Output,
    Language,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Directive {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: DirectiveCategory,
    pub content: String,
    pub is_builtin: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
    /// Optional URL to the source project — set on directives that adapt
    /// third-party prompts (e.g. Caveman → github.com/JuliusBrussee/caveman).
    /// Surfaces as a small "↗ Source" link in the settings card. MIT-licensed
    /// adaptations should include this for attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateDirectiveRequest {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: DirectiveCategory,
    pub content: String,
    #[serde(default)]
    pub conflicts: Vec<String>,
}
