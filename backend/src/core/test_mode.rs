//! Whether this process is a unit-test binary of the backend.
//!
//! A crate's own unit tests are built with `cfg(test)`; the crates below it
//! are then plain libraries built with the `test-support` feature, and so are
//! they in an integration binary or a server built beside the tests. What a
//! unit test must see (a scratch data directory, short waits, per-thread
//! settings) is therefore decided when the process runs, not when it is built.

/// Submitted by the test build of every crate above this one.
#[cfg(feature = "test-support")]
pub struct UnitTestBinary;
#[cfg(feature = "test-support")]
inventory::collect!(UnitTestBinary);

/// True in a unit-test binary of any backend crate; false in the server and
/// in an integration binary, whatever they were built beside.
pub fn on() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(all(not(test), feature = "test-support"))]
    {
        inventory::iter::<UnitTestBinary>
            .into_iter()
            .next()
            .is_some()
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        false
    }
}
