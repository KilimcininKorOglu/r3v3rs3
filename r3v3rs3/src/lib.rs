//! The r3v3rs3 server. The modules live in the `r3v3rs3-*` crates, which compile in parallel;
//! this crate re-exports all of them for the `r3-server` binary and the integration tests.
//!
//! This crate alone carries the release version and the embedded WebUI: the library crates have
//! a fixed package version and can come from a build cache, so a release recompiles only this
//! crate.

pub use r3v3rs3_admin::*;

/// The WebUI that trunk builds into `dist/webui`.
static WEBUI: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/dist");

/// Sets the release version that the library crates read through [`release::version`] and the
/// WebUI that the admin API serves.
pub fn init() {
    release::set(env!("CARGO_PKG_VERSION"));
    admin::static_file::set_webui(&WEBUI);
}

/// The configuration module, with the app info of this binary.
pub mod config {
    pub use r3v3rs3_admin::config::*;
    use r3v3rs3_api::app::AppInfo;
    use std::path::Path;

    /// Sets the release version and returns the app info of this binary.
    pub fn new_appinfo(config_path: &Path, log_path: &Path) -> AppInfo {
        crate::init();
        app_info(env!("CARGO_PKG_VERSION"), config_path, log_path)
    }
}
