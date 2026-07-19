//! Create-only filesystem primitives for install-type routes (Codex A2).
//!
//! Guarantees, stated honestly:
//! - PRE-EXISTING symlinks anywhere on the path (dangling included) are
//!   refused by an lstat walk before any write;
//! - the FINAL component is race-free: content is written to a temp
//!   sibling and published atomically no-clobber (`hard_link`), so a file
//!   or symlink appearing between check and publish is never overwritten
//!   nor followed — the call quietly reports `Ok(false)`;
//! - a failure before publish leaves NO destination at all (the temp is
//!   removed by best-effort RAII); durability is fs::write parity (no
//!   fsync);
//! - OUT OF SCOPE until the A1 rooted primitive (openat2/openat walk,
//!   Windows handle-rooted — fs_guard migrates onto it in A1.2): a
//!   concurrent replacement of a PARENT directory by a symlink after the
//!   lstat walk. A crashed process may leave an orphan `.kronn-tmp-*`
//!   sibling; it is inert and never auto-deleted here.

use std::path::{Component, Path, PathBuf};

/// Verify `path` is lexically under `root` and that no component from
/// `root` (exclusive) down to `path` (inclusive) is a symlink. Missing
/// components are fine — they're what we're about to create.
pub fn assert_contained_no_symlink(root: &Path, path: &Path) -> Result<(), String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| format!("{} escapes the root {}", path.display(), root.display()))?;
    if rel.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{} contains a parent traversal", path.display()));
    }
    let mut cur = PathBuf::from(root);
    for comp in rel.components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink — refusing to write through it",
                    cur.display()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// `create_dir_all` behind the lstat walk. Limitation (module header):
/// a parent swapped to a symlink AFTER the walk is followed by the
/// underlying `create_dir_all` — closed by the A1 rooted primitive.
pub fn guarded_create_dir_all(root: &Path, dir: &Path) -> Result<(), String> {
    assert_contained_no_symlink(root, dir)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))
}

/// Best-effort RAII cleanup for the temp sibling on non-publish exit
/// paths (a successful publish disarms it and unlinks explicitly). An
/// unlink failure cannot be compensated here — it is logged, and the
/// destination-never-published guarantee is unaffected.
struct TempGuard {
    path: PathBuf,
    armed: bool,
}
impl Drop for TempGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(e) = std::fs::remove_file(&self.path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("temp cleanup failed {}: {e}", self.path.display());
                }
            }
        }
    }
}

/// Short, basename-independent temp sibling (a target near NAME_MAX must
/// not make its temp impossible).
fn temp_sibling(parent: &Path) -> PathBuf {
    parent.join(format!(".kronn-tmp-{}", uuid::Uuid::new_v4()))
}

/// Two-phase create-only publication. `write` fills the temp handle
/// (payload + any handle-level permissions); the temp is then published
/// atomically no-clobber via `hard_link`:
/// - anything occupying `dst` at publish time (file, symlink even
///   dangling — never followed) ⇒ `Ok(false)`, best-effort temp cleanup,
///   dst intact;
/// - any error before publish ⇒ `Err`, best-effort temp cleanup, dst
///   NEVER existed;
/// - `Unsupported`/`PermissionDenied` on the link are explicit `Err`s —
///   no rename fallback (rename overwrites);
/// - after a successful link the destination is live: a failed unlink of
///   the temp downgrades to a warning, never to a false `Err`.
///
/// `pre_publish` runs between "temp fully written" and the publish —
/// production passes a no-op, tests inject deterministic races: one code
/// path, no hidden state.
fn create_new_via_temp(
    dst: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), String>,
    pre_publish: impl FnOnce(),
) -> Result<bool, String> {
    let parent = dst
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", dst.display()))?;
    let tmp_path = temp_sibling(parent);
    // Default OpenOptions mode (0666 & umask) — fs::write parity for
    // guarded_write_new; guarded_copy_new overrides via the handle.
    let mut tmp = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| format!("create temp {}: {e}", tmp_path.display()))?;
    let mut guard = TempGuard {
        path: tmp_path,
        armed: true,
    };
    write(&mut tmp)?;
    {
        use std::io::Write;
        tmp.flush()
            .map_err(|e| format!("flush {}: {e}", guard.path.display()))?;
    }
    drop(tmp); // close before linking (Windows requirement)
    pre_publish();
    match std::fs::hard_link(&guard.path, dst) {
        Ok(()) => {
            guard.armed = false;
            if let Err(e) = std::fs::remove_file(&guard.path) {
                tracing::warn!(
                    "temp cleanup failed after publish {}: {e}",
                    guard.path.display()
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!(
            "publish {} -> {}: {e}",
            guard.path.display(),
            dst.display()
        )),
    }
}

/// Write `bytes` at `path` ONLY if nothing exists there (lstat — a
/// dangling symlink is "something"). Returns whether a file was written.
/// Two-phase publish: see `create_new_via_temp` for the race contract.
pub fn guarded_write_new(root: &Path, path: &Path, bytes: &[u8]) -> Result<bool, String> {
    guarded_write_new_hooked(root, path, bytes, || {}, || {})
}

/// Full wrapper with test seams: `after_parents` runs after every
/// path-based check AND the parent creation — the exact window where a
/// parent swapped to a symlink is no longer re-checked (the documented
/// A1 hole); `pre_publish` runs between the written temp and the publish
/// (final-collision injection). Production goes through this exact path
/// with no-ops.
fn guarded_write_new_hooked(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    after_parents: impl FnOnce(),
    pre_publish: impl FnOnce(),
) -> Result<bool, String> {
    assert_contained_no_symlink(root, path)?;
    if std::fs::symlink_metadata(path).is_ok() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        guarded_create_dir_all(root, parent)?;
    }
    after_parents();
    create_new_via_temp(
        path,
        |tmp| {
            use std::io::Write;
            tmp.write_all(bytes)
                .map_err(|e| format!("write {}: {e}", path.display()))
        },
        pre_publish,
    )
}

/// Copy `src` to `dst` ONLY if nothing exists at `dst` (lstat). Returns
/// whether a file was created. Two-phase publish; the src mode (fstat on
/// the open src handle) is applied to the temp handle BEFORE publication
/// — what `fs::copy` guaranteed before the streamed rewrite.
pub fn guarded_copy_new(root: &Path, src: &Path, dst: &Path) -> Result<bool, String> {
    assert_contained_no_symlink(root, dst)?;
    if std::fs::symlink_metadata(dst).is_ok() {
        return Ok(false);
    }
    // Input first (msg 213 pipeline): no destination mutation happens for
    // an unreadable source.
    let mut input = std::fs::File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    if let Some(parent) = dst.parent() {
        guarded_create_dir_all(root, parent)?;
    }
    create_new_via_temp(
        dst,
        |tmp| {
            use std::io::Write;
            std::io::copy(&mut input, tmp)
                .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
            // Flush BEFORE the mode lands on the handle (msg 213 order); the
            // caller's post-write flush is then a benign second flush.
            tmp.flush()
                .map_err(|e| format!("flush temp for {}: {e}", dst.display()))?;
            let perms = input
                .metadata()
                .map_err(|e| format!("metadata {}: {e}", src.display()))?
                .permissions();
            tmp.set_permissions(perms)
                .map_err(|e| format!("chmod temp for {}: {e}", dst.display()))
        },
        || {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_temp_left(dir: &Path) -> bool {
        // An unreadable dir or entry must FAIL the check, never pass it.
        std::fs::read_dir(dir)
            .expect("test dir must be readable")
            .map(|e| e.expect("test dir entry must be readable"))
            .all(|e| !e.file_name().to_string_lossy().starts_with(".kronn-tmp-"))
    }

    #[test]
    fn concurrent_regular_file_at_publish_is_never_clobbered() {
        // The Copilot round-3 finding, made deterministic: a rival file
        // lands EXACTLY between "temp written" and the publish.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let dst = proj.join("docs/target.md");
        let rival = dst.clone();
        let created = guarded_write_new_hooked(
            &proj,
            &dst,
            b"kronn content",
            || {},
            move || std::fs::write(&rival, "SENTINEL").unwrap(),
        )
        .unwrap();
        assert!(!created, "losing the race must report false, not clobber");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "SENTINEL");
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_appearing_at_publish_is_never_followed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let dst = proj.join("docs/target.md");
        let link = dst.clone();
        let nowhere = tmp.path().join("nowhere");
        let nowhere_hook = nowhere.clone();
        let created = guarded_write_new_hooked(
            &proj,
            &dst,
            b"kronn content",
            || {},
            move || std::os::unix::fs::symlink(&nowhere_hook, &link).unwrap(),
        )
        .unwrap();
        assert!(!created);
        let meta = std::fs::symlink_metadata(&dst).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the late link stays byte-intact"
        );
        assert!(!nowhere.exists(), "the link target was never created");
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[test]
    fn name_max_basename_still_publishes() {
        // The temp name is basename-independent — a destination close to
        // NAME_MAX must not make the temp impossible.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let long = format!("{}.md", "a".repeat(251));
        let dst = proj.join("docs").join(&long);
        assert!(guarded_write_new(&proj, &dst, b"content").unwrap());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "content");
    }

    #[test]
    fn injected_write_failure_leaves_no_destination_and_no_temp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        let dst = proj.join("docs/target.md");
        let err = create_new_via_temp(
            &dst,
            |f| {
                use std::io::Write;
                f.write_all(b"partial prefix").unwrap();
                Err("injected failure after a partial write".into())
            },
            || {},
        )
        .unwrap_err();
        assert!(err.contains("injected"), "{err}");
        assert!(
            std::fs::symlink_metadata(&dst).is_err(),
            "a pre-publish failure must never leave a destination"
        );
        assert!(no_temp_left(&proj.join("docs")));
    }

    #[cfg(unix)]
    #[test]
    fn write_new_mode_matches_fs_write_parity() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let dst = proj.join("docs/a.md");
        assert!(guarded_write_new(&proj, &dst, b"x").unwrap());
        let control = proj.join("control.md");
        std::fs::write(&control, "x").unwrap();
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            std::fs::metadata(&control).unwrap().permissions().mode() & 0o777,
            "guarded_write_new keeps fs::write mode parity (0666 & umask)"
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "KNOWN HOLE until the A1 rooted primitive (openat walk, fs_guard migrates in A1.2): run with --ignored, fails today for the documented reason"]
    fn parent_swapped_to_symlink_after_walk_is_refused() {
        // Real regression harness, not a placeholder: every path-based
        // check (walk + parent creation, which re-walks) passes on a clean
        // `docs`, THEN the directory is swapped for a symlink to an
        // EXTERNAL dir — the temp open and the publish both traverse the
        // swapped parent. The security assertion below is what A1 must
        // make true — today the write escapes the root and this test
        // fails under --ignored.
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        let docs = proj.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let docs_hook = docs.clone();
        let external_hook = external.clone();
        let res = guarded_write_new_hooked(
            &proj,
            &proj.join("docs/escape.md"),
            b"x",
            move || {
                std::fs::remove_dir_all(&docs_hook).unwrap();
                std::os::unix::fs::symlink(&external_hook, &docs_hook).unwrap();
            },
            || {},
        );
        assert!(
            res.is_err() && !external.join("escape.md").exists(),
            "a parent swapped to a symlink after the walk must be refused, \
             and nothing may be created outside the root (A1 contract)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_component_and_dangling_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let external = root.join("external");
        std::fs::create_dir_all(&external).unwrap();
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join("docs")).unwrap();
        std::os::unix::fs::symlink(&external, proj.join("docs/conventions")).unwrap();
        // Through a symlinked dir component -> refusal.
        let err =
            guarded_write_new(&proj, &proj.join("docs/conventions/spec.md"), b"x").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
        // Dangling symlink target -> no write, link intact.
        std::os::unix::fs::symlink(root.join("nowhere"), proj.join("docs/index.md")).unwrap();
        let err = guarded_write_new(&proj, &proj.join("docs/index.md"), b"x").unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::symlink_metadata(proj.join("docs/index.md"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn copy_preserves_src_mode_and_stays_create_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let src = tmp.path().join("template.sh");
        std::fs::write(&src, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dst = proj.join("docs/run.sh");
        assert!(guarded_copy_new(&proj, &src, &dst).unwrap());
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o755,
            "the streamed create-new must re-apply the src mode like fs::copy did"
        );
        // Second copy: dst exists — untouched (create-only holds end to end).
        std::fs::write(&dst, "user edit").unwrap();
        assert!(!guarded_copy_new(&proj, &src, &dst).unwrap());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "user edit");
    }

    #[test]
    fn refuses_escape_and_creates_cleanly_inside() {
        let tmp = tempfile::TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        assert!(assert_contained_no_symlink(&proj, &tmp.path().join("outside.md")).is_err());
        assert!(guarded_write_new(&proj, &proj.join("docs/new.md"), b"hello").unwrap());
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/new.md")).unwrap(),
            "hello"
        );
        // Second write: target exists -> untouched.
        assert!(!guarded_write_new(&proj, &proj.join("docs/new.md"), b"clobber").unwrap());
        assert_eq!(
            std::fs::read_to_string(proj.join("docs/new.md")).unwrap(),
            "hello"
        );
    }
}
