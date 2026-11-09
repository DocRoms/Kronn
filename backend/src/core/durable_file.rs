//! Flush a finished file to disk before it is renamed into place.

use std::path::Path;

/// Syncs `path`'s content to disk. Windows' `FlushFileBuffers` refuses a
/// read-only handle (os error 5), so the file is opened for writing, without
/// truncating it.
pub(crate) fn sync_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_copy_is_synced_without_changing_it() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("kronn.db.backup.tmp");
        std::fs::write(&copy, b"SQLite format 3\0content").unwrap();

        sync_file(&copy).unwrap();

        assert_eq!(std::fs::read(&copy).unwrap(), b"SQLite format 3\0content");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sync_file(&dir.path().join("absent")).is_err());
    }
}
