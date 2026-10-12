//! Fixtures shared by tests across modules.

pub(crate) mod hill;

/// Settings for the crate's property tests: few enough cases to keep the
/// suite fast, and no regression files written into the source tree.
pub(crate) fn proptest_config() -> proptest::test_runner::Config {
    proptest::test_runner::Config {
        cases: 256,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    }
}
