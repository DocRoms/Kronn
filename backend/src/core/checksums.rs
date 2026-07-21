use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use super::cmd::sync_cmd;

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
    Some(sha256_of_bytes(&data))
}

/// Inner helper that computes the hex SHA-256 of a byte slice. Exposed
/// so tests can pin the exact byte-to-hex contract.
fn sha256_of_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    // sha2 0.11 returns `hybrid_array::Array<u8, _>` instead of
    // `GenericArray<u8, _>`. The new type does not implement `LowerHex`,
    // so `format!("{:x}", _)` no longer compiles. Use `result.iter()`
    // and a manual hex encoding (no extra dep).
    result.iter().map(|b| format!("{:02x}", b)).collect()
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

/// Root agent-context files the audit writes into. The four
/// [`crate::core::root_agent_files::KRONN_ROOT_AGENT_FILES`] carry the
/// managed-block contract (user content around the block preserved
/// byte-identical); the other three are template-rendered at install and
/// carry a regenerated `KRONN:FACTS` region. NONE of them is excluded from
/// the fingerprint — they are hashed with the Kronn-managed regions
/// stripped, so user-authored rules in them DO count as source while Kronn's
/// own (re)generation does not.
const ROOT_AGENT_CONTEXT_FILES: &[&str] = &[
    "CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules",
    "AGENTS.md", "GEMINI.md", ".github/copilot-instructions.md",
];

/// Paths Kronn itself generates wholesale — excluded from the source-tree
/// fingerprint (`__GIT_SOURCE_TREE__`) so committing an audit's own output
/// does NOT count as a source change and self-inflict drift (F27). Tight on
/// purpose: only the DETECTED docs dir (a project whose `ai/` is real source
/// keeps it counted when its docs live in `docs/`) and the `.kronn` state
/// files. Root agent files are NOT here — they get normalized hashing.
fn is_kronn_generated_path(path: &str, docs_dir_rel: &str) -> bool {
    (!docs_dir_rel.is_empty() && (path == docs_dir_rel || path.starts_with(&format!("{docs_dir_rel}/"))))
        || path == ".kronn.json"
        || path == ".kronn.lock"
        || path.starts_with(".kronn/")
}

/// Strip the Kronn-managed regions from a root agent file: the
/// `KRONN-MANAGED-BLOCK` (prepended by `root_agent_files::inject_or_update`)
/// and the `KRONN:FACTS` region (regenerated by each audit). What remains is
/// user-owned content — the part that must count as source.
fn strip_kronn_regions(content: &str) -> String {
    let mut out = content.to_string();
    for (start, end) in [
        (
            crate::core::root_agent_files::KRONN_BLOCK_START,
            crate::core::root_agent_files::KRONN_BLOCK_END,
        ),
        ("<!-- KRONN:FACTS", "<!-- END KRONN:FACTS -->"),
    ] {
        while let Some(s) = out.find(start) {
            let Some(e) = out[s..].find(end).map(|rel| s + rel + end.len()) else { break };
            out.replace_range(s..e, "");
        }
    }
    out
}

/// Turn raw path bytes into an OS path component. Unix filenames are
/// arbitrary bytes; a lossy round-trip would point at a DIFFERENT file.
#[cfg(unix)]
fn os_path_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes).to_os_string()
}
#[cfg(not(unix))]
fn os_path_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

/// Fingerprint the SOURCE as the audit actually saw it — the WORKTREE, not
/// HEAD: an uncommitted edit to a tracked file must flag drift (the audit
/// read the worktree), and committing unchanged content must NOT (records
/// are content-derived, so add/commit is a no-op on the print). A SHA-256
/// over, per non-excluded path (tracked ∪ untracked-non-ignored):
///   - regular file → `<f|x> <sha256(content)> <path>` (exec bit counts),
///   - symlink → `l <sha256(target)> <path>`,
///   - directory entry (submodule) → `gitlink <path>`.
///
/// Plus a normalized entry for each root agent file (Kronn regions
/// stripped; empty remainder contributes nothing — the audit CREATING a
/// pure-redirector file doesn't move the print, user rules added to it do).
///
/// Subsumes the old `__GIT_HEAD__` (drifted on EVERY commit, incl.
/// docs-only) and `__GIT_LS_FILES__` (drifted when a generated doc became
/// tracked) — the two F27 culprits. All path handling is raw bytes
/// (newlines/non-UTF-8 legal on Unix); records are NUL-joined into the
/// hasher (NUL cannot appear in a path). Returns None when git is
/// unavailable (no repo): the section simply isn't tracked.
fn manifest_record_path(record: &[u8]) -> &[u8] {
    if let Some(path) = record.strip_prefix(b"deleted ") {
        return path;
    }
    if let Some(path) = record.strip_prefix(b"gitlink ") {
        return path;
    }
    if record.starts_with(b"normalized:") {
        return record.splitn(2, |byte| *byte == b' ').nth(1).unwrap_or_default();
    }
    record.splitn(3, |byte| *byte == b' ').nth(2).unwrap_or_default()
}

fn trace_source_tree_manifest(observation: &str, records: &[Vec<u8>], fingerprint: &str) {
    if std::env::var("KRONN_F27_MANIFEST").ok().as_deref() != Some("1") {
        return;
    }
    // Opaque, per-process salt: a logged per-file digest must not become an
    // offline oracle for low-entropy secrets accidentally left unignored.
    // The salt is deliberately never logged; tags remain comparable only
    // inside this backend process, which is all the diagnostic needs.
    static SALT: OnceLock<[u8; 16]> = OnceLock::new();
    let salt = SALT.get_or_init(|| *uuid::Uuid::new_v4().as_bytes());
    tracing::info!(
        target: "kronn::f27_manifest",
        observation,
        record_count = records.len(),
        fingerprint,
        "F27 source-tree manifest"
    );
    for record in records {
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(record);
        let opaque_tag: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        tracing::info!(
            target: "kronn::f27_manifest",
            observation,
            path = %String::from_utf8_lossy(manifest_record_path(record)),
            opaque_tag,
            "F27 source-tree record"
        );
    }
}

fn git_source_tree_fingerprint_observed(project_path: &Path, observation: Option<&str>) -> Option<String> {
    let docs_dir_rel = crate::core::scanner::detect_docs_dir(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tracked = run_git_bytes(project_path, &["ls-files", "-z"])?;
    let untracked = run_git_bytes(
        project_path,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    ).unwrap_or_default();

    let mut paths: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
    for listing in [&tracked, &untracked] {
        for p in listing.split(|&b| b == 0).filter(|p| !p.is_empty()) {
            paths.insert(p.to_vec());
        }
    }

    let mut kept: Vec<Vec<u8>> = Vec::new();
    for path_bytes in &paths {
        // Exclusions are defined on ASCII names — a non-UTF-8 path can never
        // match one, so it is always source and stays counted verbatim.
        if let Ok(path) = std::str::from_utf8(path_bytes) {
            if is_kronn_generated_path(path, &docs_dir_rel)
                || ROOT_AGENT_CONTEXT_FILES.contains(&path)
            {
                continue;
            }
        }
        let full = project_path.join(os_path_from_bytes(path_bytes));
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            // Tracked but deleted in the worktree: the deletion IS a source
            // change — record it so the print moves.
            let mut record = b"deleted ".to_vec();
            record.extend_from_slice(path_bytes);
            kept.push(record);
            continue;
        };
        let record_head: String = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full)
                .map(|t| t.into_os_string())
                .unwrap_or_default();
            #[cfg(unix)]
            let target_bytes = {
                use std::os::unix::ffi::OsStrExt;
                target.as_os_str().as_bytes().to_vec()
            };
            #[cfg(not(unix))]
            let target_bytes = target.to_string_lossy().into_owned().into_bytes();
            format!("l {}", sha256_of_bytes(&target_bytes))
        } else if meta.is_dir() {
            // A directory in ls-files output is a gitlink (submodule) —
            // presence tracked, inner content out of scope.
            "gitlink".to_string()
        } else {
            let Ok(content) = std::fs::read(&full) else { continue };
            #[cfg(unix)]
            let exec = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let exec = false;
            format!("{} {}", if exec { "x" } else { "f" }, sha256_of_bytes(&content))
        };
        let mut record = record_head.into_bytes();
        record.push(b' ');
        record.extend_from_slice(path_bytes);
        kept.push(record);
    }
    for &agent_file in ROOT_AGENT_CONTEXT_FILES {
        let Ok(content) = std::fs::read_to_string(project_path.join(agent_file)) else { continue };
        let user_part = strip_kronn_regions(&content);
        if user_part.trim().is_empty() {
            continue;
        }
        kept.push(
            format!("normalized:{} {agent_file}", sha256_of_bytes(user_part.as_bytes()))
                .into_bytes(),
        );
    }
    kept.sort();
    let mut hasher = Sha256::new();
    for (i, record) in kept.iter().enumerate() {
        if i > 0 {
            hasher.update([0u8]);
        }
        hasher.update(record);
    }
    let fingerprint: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    if let Some(label) = observation {
        trace_source_tree_manifest(label, &kept, &fingerprint);
    }
    Some(fingerprint)
}

fn git_source_tree_fingerprint(project_path: &Path) -> Option<String> {
    // The env gate inside the tracer keeps production silent. When enabled
    // for an investigative run, ordinary drift checks must emit a comparable
    // manifest too — baseline-only instrumentation cannot identify the later
    // divergent record.
    git_source_tree_fingerprint_observed(project_path, Some("runtime-check"))
}

/// Run a git command returning RAW stdout bytes. Needed where output can
/// carry arbitrary path bytes (`ls-tree -z`): a lossy UTF-8 pass would
/// silently alter non-UTF-8 names. Returns None on failure.
fn run_git_bytes(project_path: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = sync_cmd("git");
    cmd.arg("-C").arg(project_path);
    for arg in args {
        cmd.arg(arg);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

/// Run a git command in the given directory, returning stdout trimmed. Returns None on failure.
fn run_git(project_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = sync_cmd("git");
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

const SOURCE_TREE_QUIET_PERIOD: Duration = Duration::from_millis(250);

fn stable_source_tree_fingerprint_with_between<F>(
    project_path: &Path,
    between_reads: F,
) -> Result<Option<String>, String>
where
    F: FnOnce(),
{
    let first = git_source_tree_fingerprint_observed(project_path, Some("quiescence-first"));
    between_reads();
    let second = git_source_tree_fingerprint_observed(project_path, Some("quiescence-second"));
    match (first, second) {
        (Some(a), Some(b)) if a == b => Ok(Some(a)),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(
            "source-tree fingerprint changed during quiescence window; baseline not frozen"
                .to_string(),
        ),
        _ => Err("source-tree fingerprint availability changed during quiescence window".to_string()),
    }
}

/// Freeze a source-tree fingerprint only after a bounded quiet period. This
/// is a fail-closed guard, not a claim about the root cause of any particular
/// drift incident.
pub fn stable_source_tree_fingerprint(project_path: &Path) -> Result<Option<String>, String> {
    stable_source_tree_fingerprint_with_between(project_path, || {
        std::thread::sleep(SOURCE_TREE_QUIET_PERIOD);
    })
}

#[cfg(test)]
pub(crate) fn stable_source_tree_fingerprint_test_hook<F>(
    project_path: &Path,
    between_reads: F,
) -> Result<Option<String>, String>
where
    F: FnOnce(),
{
    stable_source_tree_fingerprint_with_between(project_path, between_reads)
}

/// Compute checksums for a set of source patterns relative to the project root.
///
/// Special patterns:
/// - `__GIT_SOURCE_TREE__`: F27 fingerprint of the current source worktree
///   (content + structure), excluding Kronn outputs — the preferred whole-
///   repo signal.
/// - `__GIT_HEAD__` / `__GIT_LS_FILES__`: legacy whole-repo sentinels, kept
///   for back-compat with pre-F27 baselines (they self-drift on Kronn's own
///   commits — new steps should use `__GIT_SOURCE_TREE__`).
/// - Patterns containing `*`: expanded as simple directory globs.
/// - Everything else: treated as a plain file path.
pub fn compute_step_checksums(project_path: &Path, patterns: &[&str]) -> BTreeMap<String, String> {
    compute_step_checksums_impl(project_path, patterns, None)
}

/// Compute a mapping while reusing the already-quiesced whole-tree snapshot.
/// `None` means the project is not a Git repository and the sentinel is
/// intentionally omitted; it never triggers another repository scan.
pub fn compute_step_checksums_from_snapshot(
    project_path: &Path,
    patterns: &[&str],
    source_tree_fingerprint: Option<&str>,
) -> BTreeMap<String, String> {
    compute_step_checksums_impl(project_path, patterns, Some(source_tree_fingerprint))
}

fn compute_step_checksums_impl(
    project_path: &Path,
    patterns: &[&str],
    source_tree_override: Option<Option<&str>>,
) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    for &pattern in patterns {
        match pattern {
            "__GIT_SOURCE_TREE__" => {
                // F27 — content+structure fingerprint of the source tree,
                // excluding Kronn outputs. Preferred over the two sentinels
                // below (kept only for back-compat with pre-F27 baselines).
                let fingerprint = match source_tree_override {
                    Some(fixed) => fixed.map(str::to_string),
                    None => git_source_tree_fingerprint(project_path),
                };
                if let Some(fp) = fingerprint {
                    map.insert("__GIT_SOURCE_TREE__".to_string(), fp);
                }
            }
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
                    // sha2 0.11 — see compute_sha256 above for context.
                    let digest: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
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

/// Write a `checksums.json` to the project's docs folder (post-pivot
/// `docs/`, legacy `ai/`). Path-agnostic via `detect_docs_dir`.
pub fn write_checksums_file(
    project_path: &Path,
    mappings: &[ChecksumMapping],
) -> Result<(), String> {
    publish_checksums_file(project_path, mappings).map(|_| ())
}

fn publish_checksums_file(
    project_path: &Path,
    mappings: &[ChecksumMapping],
) -> Result<(PathBuf, Vec<u8>), String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    std::fs::create_dir_all(&docs_dir)
        .map_err(|e| format!("Failed to create {} dir: {e}", docs_dir.display()))?;

    let file = ChecksumsFile {
        audited_at: chrono::Utc::now().to_rfc3339(),
        mappings: mappings.to_vec(),
    };

    let json = serde_json::to_vec_pretty(&file)
        .map_err(|e| format!("JSON serialize error: {e}"))?;

    // Atomic sibling-temp + rename (Codex lot-2 #4): a direct fs::write
    // that fails mid-stream truncates the manifest — read_checksums_file
    // then returns None and the whole drift scope silently DISAPPEARS
    // instead of staying stale. On any failure here the previous valid
    // manifest stays byte-intact.
    let path = docs_dir.join("checksums.json");
    crate::core::mcp_scanner::atomic_write_bytes(&path, &json)?;

    Ok((path, json))
}

fn restore_checksums_bytes(
    path: &Path,
    previous: Option<&[u8]>,
    published: &[u8],
) -> Result<(), String> {
    let current = std::fs::read(path)
        .map_err(|e| format!("Failed to verify published checksum baseline {}: {e}", path.display()))?;
    if current != published {
        return Err(format!(
            "checksum baseline {} changed concurrently after publication; refusing to overwrite or delete it",
            path.display()
        ));
    }
    match previous {
        Some(bytes) => crate::core::mcp_scanner::atomic_write_bytes(path, bytes)
            .map_err(|e| format!("Failed to restore previous checksum baseline {}: {e}", path.display())),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to remove newly-published checksum baseline {}: {e}", path.display())),
        },
    }
}

fn write_checksums_file_fail_closed_with_after<F>(
    project_path: &Path,
    mappings: &[ChecksumMapping],
    expected_source_tree: Option<&str>,
    after_publish: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let before_publish = git_source_tree_fingerprint_observed(project_path, Some("pre-publish"));
    if before_publish.as_deref() != expected_source_tree {
        return Err("source tree changed before checksum baseline publication".to_string());
    }

    let path = crate::core::scanner::detect_docs_dir(project_path).join("checksums.json");
    let previous = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Could not preserve previous checksum baseline {}: {e}", path.display())),
    };

    let (published_path, published) = publish_checksums_file(project_path, mappings)?;
    debug_assert_eq!(published_path, path);
    after_publish();
    let after = git_source_tree_fingerprint_observed(project_path, Some("post-publish"));
    if after.as_deref() != expected_source_tree {
        let rollback = restore_checksums_bytes(&path, previous.as_deref(), &published);
        return match rollback {
            Ok(()) => Err("source tree changed during checksum baseline publication; previous baseline restored".to_string()),
            Err(rollback_error) => Err(format!(
                "source tree changed during checksum baseline publication AND rollback failed: {rollback_error}"
            )),
        };
    }
    Ok(())
}

/// Publish a baseline only while the source tree still matches the stable
/// snapshot, then wait through one more quiet window and roll back on drift.
pub fn write_checksums_file_fail_closed(
    project_path: &Path,
    mappings: &[ChecksumMapping],
    expected_source_tree: Option<&str>,
) -> Result<(), String> {
    write_checksums_file_fail_closed_with_after(
        project_path,
        mappings,
        expected_source_tree,
        || std::thread::sleep(SOURCE_TREE_QUIET_PERIOD),
    )
}

#[cfg(test)]
pub(crate) fn write_checksums_file_fail_closed_test_hook<F>(
    project_path: &Path,
    mappings: &[ChecksumMapping],
    expected_source_tree: Option<&str>,
    after_publish: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    write_checksums_file_fail_closed_with_after(
        project_path,
        mappings,
        expected_source_tree,
        after_publish,
    )
}

/// Read and parse the checksums file. Returns None if missing or malformed.
/// Path-agnostic via `detect_docs_dir` (handles `docs/`, `doc/`, `ai/`).
pub fn read_checksums_file(project_path: &Path) -> Option<ChecksumsFile> {
    let path = crate::core::scanner::detect_docs_dir(project_path).join("checksums.json");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Strict read: distinguishes a legitimately-missing manifest (`Ok(None)`)
/// from an unreadable/malformed one (`Err`). The partial pipeline must
/// refuse to run over a manifest it cannot read — treating a corrupt file
/// as "no baseline" would merge into an empty list and silently drop every
/// mapping outside the refreshed scope.
pub fn read_checksums_file_strict(project_path: &Path) -> Result<Option<ChecksumsFile>, String> {
    let path = crate::core::scanner::detect_docs_dir(project_path).join("checksums.json");
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("checksums manifest unreadable ({}): {e}", path.display())),
    };
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|e| format!("checksums manifest malformed ({}): {e}", path.display()))
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
    let source_tree_snapshot = checksums_file.mappings.iter()
        .any(|mapping| mapping.sources.iter().any(|source| source == "__GIT_SOURCE_TREE__"))
        .then(|| git_source_tree_fingerprint(project_path));

    for mapping in &checksums_file.mappings {
        let patterns: Vec<&str> = mapping.sources.iter().map(|s| s.as_str()).collect();
        let current = match &source_tree_snapshot {
            Some(snapshot) => compute_step_checksums_from_snapshot(
                project_path,
                &patterns,
                snapshot.as_deref(),
            ),
            None => compute_step_checksums(project_path, &patterns),
        };

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
