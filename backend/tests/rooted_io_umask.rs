//! KT-1038 — a file rooted_io creates follows the process umask. A binary of
//! its own: the umask is process-wide, so no other test may run alongside.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[test]
fn a_new_file_follows_the_umask_and_a_replaced_one_keeps_its_mode() {
    let root = tempfile::tempdir().unwrap();
    // SAFETY: umask has no failure mode; this binary runs a single test.
    let previous = unsafe { libc::umask(0o077) };
    let mode = |name: &str| {
        std::fs::metadata(root.path().join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    kronn::core::rooted_io::write(root.path(), Path::new("a/new.json"), b"x").unwrap();
    kronn::core::rooted_io::append(root.path(), Path::new("journal.md"), b"x").unwrap();
    std::fs::write(root.path().join("shared"), b"old").unwrap();
    std::fs::set_permissions(
        root.path().join("shared"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    kronn::core::rooted_io::write(root.path(), Path::new("shared"), b"new").unwrap();
    // SAFETY: as above.
    unsafe { libc::umask(previous) };
    assert_eq!(mode("a/new.json"), 0o600);
    assert_eq!(mode("journal.md"), 0o600);
    assert_eq!(mode("shared"), 0o644, "an existing file keeps its mode");
}
