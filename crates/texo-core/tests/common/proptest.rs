//! Shared proptest configuration mirroring BatPak persistence patterns.

use proptest::test_runner::FileFailurePersistence;

/// Build proptest config with optional `PROPTEST_CASES` floor and regression files.
pub fn config() -> proptest::prelude::ProptestConfig {
    let mut cases = 32;
    if let Ok(env_cases) = std::env::var("PROPTEST_CASES") {
        if let Ok(n) = env_cases.parse::<u32>() {
            cases = cases.max(n);
        }
    }
    proptest::prelude::ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(FileFailurePersistence::SourceParallel(
            "proptest-regressions",
        ))),
        ..proptest::prelude::ProptestConfig::default()
    }
}
