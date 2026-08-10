//! Reader for Vibe's workspace-trust store (`~/.vibe/trusted_folders.toml`).
//!
//! Kronn writes `<project>/.vibe/config.toml`, but Vibe only loads that file
//! when its trust store approves the containing `.vibe/` directory. An
//! untrusted entry makes Vibe drop the whole config layer, so every
//! Kronn-managed MCP server vanishes with no error on either side. Reading the
//! store lets the sync say so instead of writing a file nobody reads.
//!
//! Kronn never writes here: trust is the user's security decision.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Tri-state matching Vibe's `TrustedFoldersManager.is_trusted`, which returns
/// `None` when no ancestor carries a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibeTrust {
    Trusted,
    Untrusted,
    Undecided,
}

impl VibeTrust {
    /// Whether Vibe would refuse to load a config under this path.
    pub fn blocks_config_load(self) -> bool {
        matches!(self, VibeTrust::Untrusted)
    }
}

/// The two decision lists, already normalised for comparison.
#[derive(Debug, Default, Clone)]
pub struct TrustStore {
    trusted: BTreeSet<String>,
    untrusted: BTreeSet<String>,
}

/// Vibe honours `VIBE_HOME` above `~/.vibe`; `KRONN_HOST_HOME` covers the
/// Docker case where the container's `HOME` is not the user's.
pub fn vibe_home() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("VIBE_HOME") {
        if !explicit.trim().is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    for key in ["KRONN_HOST_HOME", "HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(key) {
            if !home.trim().is_empty() {
                return Some(PathBuf::from(home).join(".vibe"));
            }
        }
    }
    None
}

pub fn trusted_folders_path() -> Option<PathBuf> {
    vibe_home().map(|dir| dir.join("trusted_folders.toml"))
}

/// Canonicalise when the path exists so symlinked roots compare equal; fall
/// back to the literal path, which is what Vibe's `Path.resolve()` does for
/// entries whose target has since been deleted.
fn normalize(path: &Path) -> String {
    match path.canonicalize() {
        Ok(resolved) => resolved.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().trim_end_matches('/').to_string(),
    }
}

impl TrustStore {
    pub fn from_toml(raw: &str) -> Self {
        let parsed = match toml::from_str::<toml::Table>(raw) {
            Ok(t) => t,
            // Vibe rewrites an unparseable store as empty, i.e. every path
            // becomes undecided. Mirror that rather than guessing.
            Err(_) => return Self::default(),
        };
        let list = |key: &str| -> BTreeSet<String> {
            parsed
                .get(key)
                .and_then(toml::Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .map(|s| normalize(Path::new(s)))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            trusted: list("trusted"),
            untrusted: list("untrusted"),
        }
    }

    pub fn load() -> Option<Self> {
        let path = trusted_folders_path()?;
        let raw = std::fs::read_to_string(path).ok()?;
        Some(Self::from_toml(&raw))
    }

    /// Closest-ancestor decision, mirroring Vibe's `_closest_decision`: the
    /// nearest ancestor carrying a decision wins, so an untrusted `.vibe`
    /// overrides a trusted repository root.
    pub fn decide(&self, path: &Path) -> VibeTrust {
        let mut current = PathBuf::from(normalize(path));
        loop {
            let key = current.to_string_lossy();
            if self.trusted.contains(key.as_ref()) {
                return VibeTrust::Trusted;
            }
            if self.untrusted.contains(key.as_ref()) {
                return VibeTrust::Untrusted;
            }
            match current.parent() {
                Some(parent) if parent != current => current = parent.to_path_buf(),
                _ => return VibeTrust::Undecided,
            }
        }
    }

    /// Every explicitly untrusted `.vibe` directory that currently holds a
    /// Kronn-managed config — i.e. the configs Kronn writes and Vibe ignores.
    pub fn blocked_kronn_configs(&self) -> Vec<PathBuf> {
        self.untrusted
            .iter()
            .map(PathBuf::from)
            .filter(|dir| dir.file_name().is_some_and(|n| n == ".vibe"))
            .filter(|dir| is_kronn_managed(&dir.join("config.toml")))
            .collect()
    }
}

/// The header `merge_vibe_config` stamps on every file it owns.
const KRONN_HEADER: &str = "# MCP section managed by Kronn";

fn is_kronn_managed(config: &Path) -> bool {
    std::fs::read_to_string(config).is_ok_and(|raw| raw.starts_with(KRONN_HEADER))
}

/// Trust verdict for the `.vibe` directory Kronn writes for `project_path`.
pub fn project_config_trust(project_path: &Path) -> VibeTrust {
    match TrustStore::load() {
        Some(store) => store.decide(&project_path.join(".vibe")),
        None => VibeTrust::Undecided,
    }
}

/// Shown wherever the blocked state surfaces, so the user gets the fix and not
/// just the diagnosis. Kronn cannot apply it: re-trusting is the user's call.
pub fn remediation_hint(config_dir: &Path) -> String {
    format!(
        "Vibe will ignore it until the directory is trusted again: run `vibe` from \
         {} and accept the workspace-trust prompt, or remove \"{}\" from the \
         `untrusted` list in {}.",
        config_dir.parent().unwrap_or(config_dir).display(),
        config_dir.display(),
        trusted_folders_path()
            .unwrap_or_else(|| PathBuf::from("~/.vibe/trusted_folders.toml"))
            .display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(trusted: &[&str], untrusted: &[&str]) -> TrustStore {
        TrustStore {
            trusted: trusted.iter().map(|s| s.to_string()).collect(),
            untrusted: untrusted.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn untrusted_vibe_dir_overrides_trusted_repo_root() {
        // The observed failure: the repo root is trusted, the `.vibe` child is
        // not, and Vibe drops every Kronn-managed MCP server.
        let s = store(&["/repo"], &["/repo/.vibe"]);
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Untrusted);
        assert!(s.decide(Path::new("/repo/.vibe")).blocks_config_load());
    }

    #[test]
    fn trusted_root_covers_undecided_child() {
        let s = store(&["/repo"], &[]);
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Trusted);
    }

    #[test]
    fn no_decision_anywhere_is_undecided() {
        let s = store(&["/other"], &["/elsewhere"]);
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Undecided);
    }

    #[test]
    fn closest_decision_wins_over_a_more_distant_one() {
        let s = store(&["/repo/.vibe"], &["/repo"]);
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Trusted);
    }

    #[test]
    fn trusted_wins_when_a_path_sits_in_both_lists() {
        // Vibe checks `trusted` before `untrusted` at each level.
        let s = store(&["/repo/.vibe"], &["/repo/.vibe"]);
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Trusted);
    }

    #[test]
    fn parses_the_real_store_shape() {
        let s = TrustStore::from_toml(
            r#"
trusted = ["/repo"]
untrusted = ["/repo/.vibe", "/other"]
"#,
        );
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Untrusted);
        assert_eq!(s.decide(Path::new("/repo/src")), VibeTrust::Trusted);
    }

    #[test]
    fn unreadable_store_leaves_everything_undecided() {
        let s = TrustStore::from_toml("this is not = valid = toml");
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Undecided);
    }

    #[test]
    fn missing_lists_do_not_panic() {
        let s = TrustStore::from_toml("unrelated = true");
        assert_eq!(s.decide(Path::new("/repo/.vibe")), VibeTrust::Undecided);
    }

    #[test]
    fn blocked_configs_report_only_kronn_managed_vibe_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Untrusted `.vibe` holding a config Kronn owns → reported.
        let managed = root.join("managed/.vibe");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(
            managed.join("config.toml"),
            format!("{KRONN_HEADER}; preserved.\n\n[[mcp_servers]]\nname = \"x\"\n"),
        )
        .unwrap();

        // Untrusted `.vibe` the user wrote themselves → not ours to report.
        let foreign = root.join("foreign/.vibe");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("config.toml"), "theme = \"dark\"\n").unwrap();

        // Untrusted directory that is not a `.vibe` at all → irrelevant.
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();

        let store = TrustStore {
            trusted: BTreeSet::new(),
            untrusted: [&managed, &foreign, &plain]
                .iter()
                .map(|p| normalize(p))
                .collect(),
        };

        let blocked = store.blocked_kronn_configs();
        assert_eq!(
            blocked.len(),
            1,
            "expected only the Kronn-managed dir: {blocked:?}"
        );
        assert!(blocked[0].ends_with(".vibe"));
        assert!(blocked[0].to_string_lossy().contains("managed"));
    }
}
