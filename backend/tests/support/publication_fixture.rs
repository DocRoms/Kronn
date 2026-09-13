//! Owned process environment for the serial KT-619 router tests.
//! Keep this guard alive until all requests/background work have completed.

pub struct PublicationFixture {
    _root: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl PublicationFixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let previous = ["KRONN_DATA_DIR", "KRONN_HOST_HOME"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let host = root.path().join("host-home");
        std::fs::create_dir(&host).unwrap();
        std::env::set_var("KRONN_DATA_DIR", root.path());
        std::env::set_var("KRONN_HOST_HOME", host);
        Self {
            _root: root,
            previous,
        }
    }
}

impl Drop for PublicationFixture {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn fixture_owns_both_roots_and_restores_the_inherited_environment() {
    let keys = ["KRONN_DATA_DIR", "KRONN_HOST_HOME"];
    let before = keys.map(std::env::var_os);
    {
        let _fixture = PublicationFixture::new();
        let data = std::path::PathBuf::from(std::env::var_os(keys[0]).unwrap());
        let host = std::path::PathBuf::from(std::env::var_os(keys[1]).unwrap());
        assert!(data.is_dir());
        assert!(host.is_dir());
        assert_eq!(host.parent(), Some(data.as_path()));
        assert_eq!(
            keys.map(std::env::var_os),
            [Some(data.into()), Some(host.into())]
        );
    }
    assert_eq!(keys.map(std::env::var_os), before);
}
