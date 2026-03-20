use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksumsFile {
    pub audited_at: String,
    pub mappings: Vec<ChecksumMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksumMapping {
    pub ai_file: String,
    pub audit_step: usize,
    pub sources: Vec<String>,
    pub checksums: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftResult {
    pub audit_date: Option<String>,
    pub stale_sections: Vec<StaleSection>,
    pub fresh_sections: Vec<String>,
    pub total_sections: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleSection {
    pub ai_file: String,
    pub audit_step: usize,
    pub changed_sources: Vec<String>,
}

/// Compute the SHA-256 hex digest of a file. Returns None if the file cannot be read.
pub fn compute_sha256(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Some(format!("{:x}", result))
}

/// Expand a glob-like pattern within a directory, returning matching (relative, absolute) pairs.
fn expand_glob(project_path: &Path, pattern: &str) -> Vec<(String, PathBuf)> {
    // Split pattern into directory prefix and file glob part
    let pattern_path = Path::new(pattern);
    let parent = pattern_path.parent().unwrap_or(Path::new(""));
    let file_pattern = pattern_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let search_dir = project_path.join(parent);
    let entries = match std::fs::read_dir(&search_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut results = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches_simple_glob(&file_pattern, &name) {
            let rel = if parent.as_os_str().is_empty() {
                name.clone()
            } else {
                format!("{}/{}", parent.display(), name)
            };
            results.push((rel, entry.path()));
        }
    }
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// Simple glob matching: only supports `*` as a wildcard that matches any sequence of characters.
fn matches_simple_glob(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == name;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let prefix = parts[0];
        let suffix = parts[1];
        return name.starts_with(prefix) && name.ends_with(suffix) && name.len() >= prefix.len() + suffix.len();
    }
    // Fallback: exact match
    pattern == name
}

/// Run a git command in the given directory, returning stdout trimmed. Returns None on failure.
fn run_git(project_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(project_path);
    for arg in args {
        cmd.arg(arg);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Compute checksums for a set of source patterns relative to the project root.
///
/// Special patterns:
/// - `__GIT_HEAD__`: stores the current HEAD commit hash.
/// - `__GIT_LS_FILES__`: stores a SHA-256 of the sorted `git ls-files` output.
/// - Patterns containing `*`: expanded as simple directory globs.
/// - Everything else: treated as a plain file path.
pub fn compute_step_checksums(project_path: &Path, patterns: &[&str]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    for &pattern in patterns {
        match pattern {
            "__GIT_HEAD__" => {
                if let Some(hash) = run_git(project_path, &["rev-parse", "HEAD"]) {
                    map.insert("__GIT_HEAD__".to_string(), hash);
                }
            }
            "__GIT_LS_FILES__" => {
                if let Some(files_output) = run_git(project_path, &["ls-files"]) {
                    let mut lines: Vec<&str> = files_output.lines().collect();
                    lines.sort();
                    let sorted = lines.join("\n");
                    let mut hasher = Sha256::new();
                    hasher.update(sorted.as_bytes());
                    let digest = format!("{:x}", hasher.finalize());
                    map.insert("__GIT_LS_FILES__".to_string(), digest);
                }
            }
            p if p.contains('*') => {
                for (rel, abs) in expand_glob(project_path, p) {
                    if let Some(hash) = compute_sha256(&abs) {
                        map.insert(rel, hash);
                    }
                }
            }
            _ => {
                let full = project_path.join(pattern);
                if let Some(hash) = compute_sha256(&full) {
                    map.insert(pattern.to_string(), hash);
                }
            }
        }
    }

    map
}

/// Write a checksums file to `ai/checksums.json` inside the project directory.
pub fn write_checksums_file(
    project_path: &Path,
    mappings: &[ChecksumMapping],
) -> Result<(), String> {
    let ai_dir = project_path.join("ai");
    std::fs::create_dir_all(&ai_dir).map_err(|e| format!("Failed to create ai/ dir: {e}"))?;

    let file = ChecksumsFile {
        audited_at: chrono::Utc::now().to_rfc3339(),
        mappings: mappings.to_vec(),
    };

    let json =
        serde_json::to_string_pretty(&file).map_err(|e| format!("JSON serialize error: {e}"))?;

    std::fs::write(ai_dir.join("checksums.json"), json)
        .map_err(|e| format!("Failed to write checksums.json: {e}"))?;

    Ok(())
}

/// Read and parse the checksums file from `ai/checksums.json`. Returns None if missing or malformed.
pub fn read_checksums_file(project_path: &Path) -> Option<ChecksumsFile> {
    let path = project_path.join("ai").join("checksums.json");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Check whether any audited sources have drifted since the last checksums were recorded.
pub fn check_drift(project_path: &Path) -> DriftResult {
    let checksums_file = match read_checksums_file(project_path) {
        Some(f) => f,
        None => {
            return DriftResult {
                audit_date: None,
                stale_sections: Vec::new(),
                fresh_sections: Vec::new(),
                total_sections: 0,
            };
        }
    };

    let mut stale_sections = Vec::new();
    let mut fresh_sections = Vec::new();

    for mapping in &checksums_file.mappings {
        let patterns: Vec<&str> = mapping.sources.iter().map(|s| s.as_str()).collect();
        let current = compute_step_checksums(project_path, &patterns);

        let mut changed_sources = Vec::new();

        // Check for changed or disappeared files
        for (key, old_hash) in &mapping.checksums {
            match current.get(key) {
                Some(new_hash) if new_hash != old_hash => {
                    changed_sources.push(key.clone());
                }
                None => {
                    changed_sources.push(format!("{} (deleted)", key));
                }
                _ => {}
            }
        }

        // Check for new files
        for key in current.keys() {
            if !mapping.checksums.contains_key(key) {
                changed_sources.push(format!("{} (new)", key));
            }
        }

        if changed_sources.is_empty() {
            fresh_sections.push(format!(
                "{}#step{}",
                mapping.ai_file, mapping.audit_step
            ));
        } else {
            stale_sections.push(StaleSection {
                ai_file: mapping.ai_file.clone(),
                audit_step: mapping.audit_step,
                changed_sources,
            });
        }
    }

    let total = checksums_file.mappings.len();

    DriftResult {
        audit_date: Some(checksums_file.audited_at),
        stale_sections,
        fresh_sections,
        total_sections: total,
    }
}

#[cfg(test)]
#[path = "checksums_test.rs"]
mod tests;
