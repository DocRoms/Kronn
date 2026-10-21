//! The documents a completed audit actually checked. This per-run baseline is
//! distinct from human provenance and citation fingerprints: neither a pull
//! nor a later edit can silently replace the completed run's contents.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub type Fingerprints = BTreeMap<String, String>;

/// The same Markdown surface as provenance (excluding legacy/var), read
/// strictly: unreadable entries and symlinks cannot become an empty baseline.
pub fn capture(project: &Path) -> Result<Fingerprints, String> {
    let docs = crate::core::scanner::detect_docs_dir(project);
    let mut fingerprints = BTreeMap::new();
    let entries = walkdir::WalkDir::new(&docs)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.depth() == 1 && matches!(entry.file_name().to_str(), Some("legacy" | "var")))
        });
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot fingerprint audit documents: {e}"))?;
        if entry.file_type().is_symlink() {
            return Err(format!(
                "Cannot fingerprint symbolic link {}",
                entry.path().display()
            ));
        }
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "md") {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(project)
            .map_err(|e| e.to_string())?;
        let content = std::fs::read(entry.path())
            .map_err(|e| format!("Cannot fingerprint {}: {e}", relative.display()))?;
        let digest = Sha256::digest(&content)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fingerprints.insert(relative.to_string_lossy().replace('\\', "/"), digest);
    }
    Ok(fingerprints)
}

pub fn capture_json(project: &Path) -> Result<String, String> {
    serde_json::to_string(&capture(project)?).map_err(|e| e.to_string())
}

/// Refuse every added, removed or modified document, including an old run for
/// which no final baseline exists. Reverification establishes a new baseline.
pub fn verify(project: &Path, recorded: Option<&str>) -> Result<(), String> {
    let recorded = recorded.ok_or_else(||
        "The completed audit has no final document fingerprints — run the audit again to reverify the documentation before validating.".to_string())?;
    let changed = changed_since(project, recorded)?;
    if changed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Documents changed since the completed audit — reverify before validating: {}",
            changed.join(", ")
        ))
    }
}

pub fn changed_since(project: &Path, recorded: &str) -> Result<Vec<String>, String> {
    let before: Fingerprints = serde_json::from_str(recorded)
        .map_err(|e| format!("Invalid final audit document fingerprints: {e}"))?;
    let current = capture(project)?;
    let changed: std::collections::BTreeSet<_> = before
        .keys()
        .chain(current.keys())
        .filter(|path| before.get(*path) != current.get(*path))
        .cloned()
        .collect();
    Ok(changed.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_final_baseline_detects_changed_added_and_deleted_docs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/AGENTS.md"), "# Équipe\nOriginal\n").unwrap();
        let baseline = capture_json(dir.path()).unwrap();
        verify(dir.path(), Some(&baseline)).unwrap();
        assert!(verify(dir.path(), None).unwrap_err().contains("reverify"));
        std::fs::write(
            dir.path().join("docs/AGENTS.md"),
            "# Équipe\nPulled from a colleague\n",
        )
        .unwrap();
        assert!(verify(dir.path(), Some(&baseline))
            .unwrap_err()
            .contains("docs/AGENTS.md"));
        std::fs::write(dir.path().join("docs/AGENTS.md"), "# Équipe\nOriginal\n").unwrap();
        std::fs::write(dir.path().join("docs/extra.md"), "New doc").unwrap();
        assert!(verify(dir.path(), Some(&baseline))
            .unwrap_err()
            .contains("docs/extra.md"));
        let new_baseline = capture_json(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("docs/extra.md")).unwrap();
        assert!(verify(dir.path(), Some(&new_baseline))
            .unwrap_err()
            .contains("docs/extra.md"));
        verify(dir.path(), Some(&baseline)).unwrap();
    }

    #[test]
    fn missing_or_malformed_baselines_never_validate_empty_docs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(capture(dir.path()).is_err());
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        assert!(verify(dir.path(), None).is_err());
        assert!(verify(dir.path(), Some("invalid")).is_err());
    }
}
