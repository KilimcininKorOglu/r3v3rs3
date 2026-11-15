//! The release version of the running binary.
//!
//! The library crates have the fixed package version `0.0.0`, so a release bump recompiles only
//! the `r3v3rs3` crate and the crates above stay cached. That crate sets the release version with
//! [`set`] before anything else runs, and every place that reports or compares the version reads
//! it with [`version`].

use std::sync::OnceLock;

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// Sets the release version. A second call keeps the first value.
pub fn set(version: &'static str) {
    VERSION.get_or_init(|| version);
}

/// The release version. It fails when the binary did not set it, so no code compares or reports
/// a version that does not exist. Test builds, which run without the binary, read
/// [`TEST_VERSION`] instead; the `test-util` feature is only a dev-dependency feature, so a
/// release build never has it.
pub fn version() -> anyhow::Result<&'static str> {
    if let Some(version) = VERSION.get() {
        return Ok(version);
    }
    unset()
}

#[cfg(not(any(test, feature = "test-util")))]
fn unset() -> anyhow::Result<&'static str> {
    Err(anyhow::anyhow!(
        "the release version of the binary is not set"
    ))
}

#[cfg(any(test, feature = "test-util"))]
fn unset() -> anyhow::Result<&'static str> {
    Ok(TEST_VERSION)
}

/// The `User-Agent` value `<product>/<release version>`.
pub fn user_agent(product: &str) -> anyhow::Result<String> {
    Ok(format!("{product}/{}", version()?))
}

/// The version that test builds read when no binary set one.
#[cfg(any(test, feature = "test-util"))]
pub const TEST_VERSION: &str = "0.0.0-test";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_version_stays() {
        set(TEST_VERSION);
        set("9.9.9");
        assert_eq!(version().unwrap(), TEST_VERSION);
        assert_eq!(user_agent("r3v3rs3").unwrap(), "r3v3rs3/0.0.0-test");
    }
}
