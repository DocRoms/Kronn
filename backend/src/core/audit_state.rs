//! Kronn's audit working files (documentary report, citation originals).
//! They live under the project's `.kronn/audit/`, excluded locally from Git,
//! never inside the audited `docs/` tree where they would be committed.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const STATE_DIR: &str = ".kronn/audit";
pub const REPORT_FILE: &str = ".kronn/audit/document-optimization.json";
pub const CITATION_ORIGINALS_DIR: &str = ".kronn/audit/citation-originals";
const TEMPLATE_BACKUPS: &str = "template-backups";
const LEGACY_REPORT_FILE: &str = "docs/.kronn-document-optimization.json";
const LEGACY_CITATION_ORIGINALS_DIR: &str = "docs/.kronn-citation-originals";

fn sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The project root the state folder is rooted at: canonical, so every path
/// below it is lexically under it for the rooted primitive (KT-1595).
fn state_root(project: &Path) -> Result<PathBuf, String> {
    project
        .canonicalize()
        .map_err(|e| format!("resolve project root: {e}"))
}

/// Create `dir` (under `root`) if missing; refuse a symlink or a
/// non-directory. The lstat is the readable refusal; the creation goes through
/// the rooted primitive, so a parent swapped for a link in between never takes
/// the folder outside the project.
fn ensure_real_dir(root: &Path, dir: &Path) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            Err(format!("{} is not a regular directory", dir.display()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::core::fs_guard::guarded_create_dir_all(root, dir)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Writes `name` in the audit state folder (created on demand), replacing it.
/// A link at the name is refused; the write goes through the rooted primitive
/// under the project, never by path (KT-1595).
pub fn write_state_file(project: &Path, name: &str, text: &str) -> Result<(), String> {
    let root = state_root(project)?;
    let target = state_dir(project)?.join(name);
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "{} is a symlink; refusing the write",
            target.display()
        ));
    }
    crate::core::fs_guard::guarded_replace(&root, &target, text.as_bytes())
}

/// The audit state directory, created on demand. Files an older version left
/// in `docs/` are moved here on the first call.
pub fn state_dir(project: &Path) -> Result<PathBuf, String> {
    let root = state_root(project)?;
    let mut dir = root.clone();
    for part in STATE_DIR.split('/') {
        dir.push(part);
        ensure_real_dir(&root, &dir)?;
    }
    // The local exclude, not the operator's `.gitignore`; a project outside
    // Git has nothing to commit by mistake.
    if let Err(e) = crate::core::worktree::ensure_local_git_exclude(&root, ".kronn/") {
        tracing::debug!("no local Git exclude for {}: {e}", root.display());
    }
    if let Err(e) = migrate_legacy(&root, &dir) {
        tracing::warn!("legacy audit files left in {}/docs: {e}", root.display());
    }
    Ok(dir)
}

/// A new, empty `template-backups/<stamp>` directory under the audit state:
/// the originals a template reinstall is about to replace. A stamp already
/// taken gets a numeric suffix, so one run never writes into another's folder.
pub fn new_template_backup_dir(project: &Path, stamp: &str) -> Result<PathBuf, String> {
    if stamp.is_empty() || !stamp.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("invalid backup stamp {stamp:?}"));
    }
    let root = state_root(project)?;
    let parent = state_dir(project)?.join(TEMPLATE_BACKUPS);
    ensure_real_dir(&root, &parent)?;
    for attempt in 0..100u32 {
        let name = if attempt == 0 {
            stamp.to_string()
        } else {
            format!("{stamp}-{attempt}")
        };
        let dir = parent.join(name);
        // Atomic create (`mkdirat` fails on EEXIST, a link included): a folder
        // another run made, or one that appears between a check and the
        // create, is never adopted. The suffix advances instead.
        if crate::core::fs_guard::guarded_create_new_dir(&root, &dir)? {
            return Ok(dir);
        }
    }
    Err("no free template backup folder".into())
}

/// Content-addressed, create-new store under `root`: each distinct original is
/// kept once. Written and compared through the rooted primitive (KT-1595).
fn store_original(root: &Path, dir: &Path, content: &[u8]) -> Result<(), String> {
    let path = dir.join(format!("{}.bak", sha256_hex(content)));
    if crate::core::fs_guard::guarded_write_new(root, &path, content)? {
        return Ok(());
    }
    match crate::core::fs_guard::guarded_read(root, &path) {
        Ok(Some(existing)) if existing == content => Ok(()),
        _ => Err("citation backup collision or symlink".into()),
    }
}

/// Save a document's original before a citation correction rewrites it.
pub fn preserve_citation_original(project: &Path, content: &str) -> Result<(), String> {
    let root = state_root(project)?;
    let dir = state_dir(project)?.join("citation-originals");
    ensure_real_dir(&root, &dir)?;
    store_original(&root, &dir, content.as_bytes())
}

const VALIDATED_ORIGINALS: &str = "validated-originals";

/// Keep the text of a doc as a human validated it: the "before" of its diff.
pub fn preserve_validated_original(project: &Path, content: &str) -> Result<(), String> {
    let root = state_root(project)?;
    let dir = state_dir(project)?.join(VALIDATED_ORIGINALS);
    ensure_real_dir(&root, &dir)?;
    store_original(&root, &dir, content.as_bytes())
}

fn validated_originals_dir(project: &Path) -> Option<PathBuf> {
    let mut dir = project.to_path_buf();
    for part in STATE_DIR.split('/').chain([VALIDATED_ORIGINALS]) {
        dir.push(part);
        let meta = fs::symlink_metadata(&dir).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
    }
    Some(dir)
}

/// The validated text with this digest, if this machine kept it intact.
pub fn read_validated_original(project: &Path, sha256: &str) -> Option<String> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let path = validated_originals_dir(project)?.join(format!("{sha256}.bak"));
    if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    (sha256_hex(&bytes) == sha256)
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

/// Drop the validated texts no record points to any more.
pub fn prune_validated_originals(project: &Path, keep: &std::collections::BTreeSet<String>) {
    let Some(dir) = validated_originals_dir(project) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(digest) = name.strip_suffix(".bak") else {
            continue;
        };
        if !keep.contains(digest) && entry.file_type().is_ok_and(|t| t.is_file()) {
            // `dir` was walked under `project` as given; the removal goes
            // through the rooted primitive under it.
            let _ = crate::core::fs_guard::guarded_remove_file(project, &entry.path());
        }
    }
}

fn migrate_legacy(root: &Path, state: &Path) -> Result<(), String> {
    // The report is regenerated by every documentary pass; nothing reads it.
    let report = root.join(LEGACY_REPORT_FILE);
    if fs::symlink_metadata(&report).is_ok_and(|m| m.is_file()) {
        crate::core::fs_guard::guarded_remove_file(root, &report)
            .map_err(|e| format!("remove {LEGACY_REPORT_FILE}: {e}"))?;
    }
    let legacy = root.join(LEGACY_CITATION_ORIGINALS_DIR);
    if !fs::symlink_metadata(&legacy).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    let target = state.join("citation-originals");
    ensure_real_dir(root, &target)?;
    for entry in fs::read_dir(&legacy).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(digest) = name.strip_suffix(".bak") else {
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Some(content) =
            crate::core::fs_guard::guarded_read(root, &entry.path()).map_err(|e| e.to_string())?
        else {
            continue;
        };
        // Only a file whose name is its own digest is Kronn's; anything else stays.
        if sha256_hex(&content) != digest {
            continue;
        }
        store_original(root, &target, &content)?;
        crate::core::fs_guard::guarded_remove_file(root, &entry.path())
            .map_err(|e| e.to_string())?;
    }
    // Succeeds only once nothing foreign is left in it.
    let _ = crate::core::fs_guard::guarded_remove_dir(root, &legacy);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, args: &[&str]) -> String {
        let out = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn originals_land_outside_docs_and_git_never_offers_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git(root, &["init", "-q"]);
        fs::create_dir(root.join("docs")).unwrap();
        preserve_citation_original(root, "# Before\n").unwrap();
        // Idempotent across a resume.
        preserve_citation_original(root, "# Before\n").unwrap();
        let backup = root
            .join(CITATION_ORIGINALS_DIR)
            .join(format!("{}.bak", sha256_hex(b"# Before\n")));
        assert_eq!(fs::read_to_string(backup).unwrap(), "# Before\n");
        assert!(fs::read_dir(root.join("docs")).unwrap().next().is_none());
        let status = git(root, &["status", "--porcelain", "--untracked-files=all"]);
        assert!(status.is_empty(), "{status}");
        let gitignore = root.join(".gitignore");
        assert!(
            !gitignore.exists(),
            "the operator's .gitignore is untouched"
        );
    }

    #[test]
    fn legacy_files_move_once_and_foreign_entries_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let legacy = root.join(LEGACY_CITATION_ORIGINALS_DIR);
        fs::create_dir_all(&legacy).unwrap();
        let original = b"# Legacy original\n";
        let name = format!("{}.bak", sha256_hex(original));
        fs::write(legacy.join(&name), original).unwrap();
        fs::write(legacy.join("not-a-digest.bak"), "foreign").unwrap();
        fs::write(root.join(LEGACY_REPORT_FILE), "{}\n").unwrap();

        state_dir(root).unwrap();
        assert_eq!(
            fs::read(root.join(CITATION_ORIGINALS_DIR).join(&name)).unwrap(),
            original
        );
        assert!(!legacy.join(&name).exists());
        assert!(legacy.join("not-a-digest.bak").exists());
        assert!(!root.join(LEGACY_REPORT_FILE).exists());

        // Second pass: nothing left to read, the foreign file is still kept.
        fs::remove_file(legacy.join("not-a-digest.bak")).unwrap();
        state_dir(root).unwrap();
        assert!(!legacy.exists());
    }

    #[test]
    fn a_tampered_legacy_backup_is_not_imported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let legacy = root.join(LEGACY_CITATION_ORIGINALS_DIR);
        fs::create_dir_all(&legacy).unwrap();
        let name = format!("{}.bak", sha256_hex(b"genuine"));
        fs::write(legacy.join(&name), "tampered").unwrap();
        state_dir(root).unwrap();
        assert!(legacy.join(&name).exists());
        assert!(!root.join(CITATION_ORIGINALS_DIR).join(&name).exists());
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn a_symlinked_state_dir_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join(".kronn")).unwrap();
        assert!(preserve_citation_original(tmp.path(), "x").is_err());
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    /// KT-1595: the state folder is written right after an agent ran; a folder
    /// swapped for a link between the walk and the write never takes a file or
    /// a folder out of the project.
    #[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
    mod swapped_folder {
        use super::*;
        use crate::core::fs_guard::swap_probe;

        /// A project whose state folder exists, and an empty external folder.
        /// The root is canonical: that is what the writers walk from.
        fn project() -> (tempfile::TempDir, PathBuf, tempfile::TempDir) {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().canonicalize().unwrap();
            fs::create_dir_all(root.join(STATE_DIR)).unwrap();
            (tmp, root, tempfile::tempdir().unwrap())
        }

        #[test]
        fn a_state_file_is_never_written_through_a_swapped_state_folder() {
            let (_tmp, root, external) = project();
            swap_probe::arm(&root.join(STATE_DIR), external.path());
            let result = write_state_file(&root, "document-blame.json", "[]");
            assert!(swap_probe::fired(), "the swap really happened");
            // The write goes to the folder it held (moved, still in the project).
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
            let held = root.join(".kronn/audit.moved/document-blame.json");
            assert_eq!(fs::read_to_string(held).unwrap(), "[]");
        }

        #[test]
        fn an_original_is_never_stored_through_a_swapped_folder() {
            let (_tmp, root, external) = project();
            let dir = root.join(CITATION_ORIGINALS_DIR);
            fs::create_dir_all(&dir).unwrap();
            swap_probe::arm(&dir, external.path());
            let result = preserve_citation_original(&root, "# Before\n");
            assert!(swap_probe::fired(), "the swap really happened");
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
            let name = format!("{}.bak", sha256_hex(b"# Before\n"));
            let held = root
                .join(".kronn/audit/citation-originals.moved")
                .join(name);
            assert_eq!(fs::read_to_string(held).unwrap(), "# Before\n");
        }

        #[test]
        fn a_backup_folder_is_never_created_through_a_swapped_folder() {
            let (_tmp, root, external) = project();
            let parent = root.join(STATE_DIR).join(TEMPLATE_BACKUPS);
            fs::create_dir_all(&parent).unwrap();
            swap_probe::arm(&parent, external.path());
            let result = new_template_backup_dir(&root, "20261011");
            assert!(swap_probe::fired(), "the swap really happened");
            // The create goes to the folder it held (moved, still in the
            // project); nothing is created behind the link.
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
            assert!(root
                .join(STATE_DIR)
                .join("template-backups.moved/20261011")
                .is_dir());
        }

        /// A folder another run (or an agent) creates under the stamp between
        /// the name being picked and the create is never adopted: the create is
        /// atomic and the suffix advances.
        #[test]
        fn a_backup_folder_that_appears_meanwhile_is_not_adopted() {
            let (_tmp, root, _external) = project();
            let parent = root.join(STATE_DIR).join(TEMPLATE_BACKUPS);
            fs::create_dir_all(&parent).unwrap();
            swap_probe::arm_hook(&parent, 0, |dir| {
                fs::create_dir(dir.join("20261011")).unwrap();
                fs::write(dir.join("20261011/theirs.txt"), "other run").unwrap();
            });
            let result = new_template_backup_dir(&root, "20261011").unwrap();
            assert!(swap_probe::fired(), "the race really happened");
            assert_eq!(result, parent.join("20261011-1"));
            assert!(result.is_dir());
            assert_eq!(fs::read_dir(&result).unwrap().count(), 0);
            assert_eq!(
                fs::read_to_string(parent.join("20261011/theirs.txt")).unwrap(),
                "other run"
            );
        }

        #[test]
        fn a_link_at_the_stamp_name_is_skipped_not_adopted() {
            let (_tmp, root, external) = project();
            let parent = root.join(STATE_DIR).join(TEMPLATE_BACKUPS);
            fs::create_dir_all(&parent).unwrap();
            std::os::unix::fs::symlink(external.path(), parent.join("20261011")).unwrap();
            let result = new_template_backup_dir(&root, "20261011").unwrap();
            assert_eq!(result, parent.join("20261011-1"));
            assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
        }

        #[test]
        fn a_symlinked_state_file_is_refused() {
            let (_tmp, root, external) = project();
            let target = external.path().join("elsewhere.json");
            std::os::unix::fs::symlink(&target, root.join(REPORT_FILE)).unwrap();
            assert!(write_state_file(&root, "document-optimization.json", "{}").is_err());
            assert!(!target.exists());
        }
    }
}
