//! Latest-known-version registry for the CLIs Kronn integrates with.
//!
//! Why a hardcoded table and not a live GitHub-API hit?
//!   - Zero network dependency in detection (works offline, in Docker, in CI).
//!   - No rate-limit/auth concerns; no opaque silent failure when GitHub
//!     burps. Detection latency stays at "the slowest agent --version call".
//!   - Per-release we bump this table as part of the Kronn release notes
//!     anyway — there's already a human checkpoint to keep it current.
//!
//! How the freshness signal works:
//!   - The detection layer compares `installed_version` to `latest_known()`.
//!   - If `installed < latest_known`, the frontend shows an "update
//!     available" pill alongside the install command (which doubles as the
//!     update command for every entry — npm / curl / uv pipelines are
//!     idempotent re-runs).
//!   - Comparison is **lenient semver** (dotted numeric prefix); pre-release
//!     suffixes (-beta, -rc1) are ignored. If a version string doesn't parse,
//!     we conservatively report "up to date" rather than nag spuriously.
//!
//! Keep `LATEST_AGENT_VERSIONS` and `LATEST_RTK_VERSION` updated on every
//! Kronn release. The reference E2E `agents_freshness_table_is_recent` test
//! catches accidental drift over many releases.

use crate::models::AgentType;

/// Latest known *stable* version of `rtk` (https://github.com/rtk-ai/rtk).
/// Bump on each Kronn release after verifying with `rtk --version` against
/// the GitHub releases page.
pub const LATEST_RTK_VERSION: &str = "0.37.2";

/// Shell command that re-installs RTK over an existing install. The RTK
/// upstream install.sh is idempotent — running it again upgrades in place.
pub const RTK_UPDATE_CMD: &str =
    "curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/main/install.sh | sh";

/// Latest known versions of the agent CLIs Kronn detects. Source of truth:
/// each vendor's release page / npm registry. Pairs (agent → version).
///
/// Bumped per Kronn release; see `docs/AGENTS.md` for the bump checklist.
/// Captured 2026-05-11.
pub fn latest_known_agent_version(agent_type: &AgentType) -> Option<&'static str> {
    match agent_type {
        // @anthropic-ai/claude-code on npm
        AgentType::ClaudeCode => Some("2.0.51"),
        // @openai/codex on npm
        AgentType::Codex => Some("0.62.0"),
        // mistral-vibe on PyPI
        AgentType::Vibe => Some("0.0.16"),
        // @google/gemini-cli on npm
        AgentType::GeminiCli => Some("0.18.0"),
        // ollama (binary release on ollama.com)
        AgentType::Ollama => Some("0.4.7"),
        // @github/copilot on npm
        AgentType::CopilotCli => Some("0.0.346"),
        // Kiro (preview, AWS distributes via cli.kiro.dev — no stable version
        // promise yet; we don't surface a freshness pill).
        AgentType::Kiro => None,
        AgentType::Custom => None,
    }
}

/// Lenient semver comparison: returns `true` when `installed` is strictly
/// older than `latest`. Strips any leading `v`, ignores pre-release and
/// build metadata suffixes (anything after `-` or `+`). On any parse error
/// we return `false` — better to under-nag than to falsely claim a stale
/// install on a version string we don't understand.
pub fn update_available(installed: &str, latest: &str) -> bool {
    fn parse(v: &str) -> Option<Vec<u64>> {
        let trimmed = v.trim().trim_start_matches('v');
        // Strip pre-release / build metadata: take everything before the
        // first `-` or `+` (semver convention).
        let core = trimmed.split(['-', '+']).next()?;
        let parts: Result<Vec<u64>, _> = core.split('.').map(|s| s.parse::<u64>()).collect();
        parts.ok()
    }
    let (Some(i), Some(l)) = (parse(installed), parse(latest)) else {
        return false;
    };
    // Compare component-by-component, zero-padding the shorter list.
    let len = i.len().max(l.len());
    for k in 0..len {
        let iv = i.get(k).copied().unwrap_or(0);
        let lv = l.get(k).copied().unwrap_or(0);
        if iv < lv {
            return true;
        }
        if iv > lv {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_available_strict_patch_bump() {
        assert!(update_available("1.2.3", "1.2.4"));
        assert!(update_available("1.2", "1.2.1"));
    }

    #[test]
    fn update_available_minor_and_major_bumps() {
        assert!(update_available("1.2.99", "1.3.0"));
        assert!(update_available("1.99.99", "2.0.0"));
    }

    #[test]
    fn equal_versions_are_up_to_date() {
        assert!(!update_available("1.2.3", "1.2.3"));
        assert!(!update_available("v1.2.3", "1.2.3"));
        assert!(!update_available("1.2.3", "v1.2.3"));
    }

    #[test]
    fn installed_ahead_of_known_is_up_to_date() {
        // Bleeding-edge user: pinned to a future release. We must not nag.
        assert!(!update_available("2.0.0", "1.99.99"));
    }

    #[test]
    fn pre_release_suffix_is_ignored() {
        // Regression: rtk 0.37.2-rc1 should compare as 0.37.2.
        assert!(!update_available("0.37.2-rc1", "0.37.2"));
        assert!(update_available("0.37.1-rc1", "0.37.2"));
    }

    #[test]
    fn build_metadata_suffix_is_ignored() {
        assert!(!update_available("1.2.3+sha.abc", "1.2.3"));
    }

    #[test]
    fn unparsable_versions_default_to_up_to_date() {
        // Better silent than wrong: never claim "update available" on a
        // version we can't compare cleanly (custom forks, dev builds).
        assert!(!update_available("dev", "1.2.3"));
        assert!(!update_available("1.2.3", "not-a-version"));
        assert!(!update_available("", "1.2.3"));
    }

    #[test]
    fn three_vs_two_segments_zero_pad() {
        // `1.2` should equal `1.2.0`, not be treated as a different shape.
        assert!(!update_available("1.2", "1.2.0"));
        assert!(update_available("1.2", "1.2.1"));
    }

    #[test]
    fn latest_agent_versions_cover_supported_agents() {
        // Hard constraint: every agent we actively integrate must have a
        // version in the table, or the freshness pill silently disappears
        // after a new agent lands.
        for agent in [
            AgentType::ClaudeCode, AgentType::Codex, AgentType::Vibe,
            AgentType::GeminiCli, AgentType::CopilotCli, AgentType::Ollama,
        ] {
            assert!(
                latest_known_agent_version(&agent).is_some(),
                "{:?} must have a latest_known version (was None)", agent,
            );
        }
        // Kiro is preview; no freshness signal expected. Custom is user-defined.
        assert!(latest_known_agent_version(&AgentType::Kiro).is_none());
        assert!(latest_known_agent_version(&AgentType::Custom).is_none());
    }

    #[test]
    fn rtk_latest_version_is_parseable() {
        // Smoke: the constant we ship must itself satisfy our comparator.
        assert!(!update_available(LATEST_RTK_VERSION, LATEST_RTK_VERSION));
    }
}
