use r3v3rs3_api::app::AppInfo;
use std::path::Path;

pub mod account;
pub mod file;
pub mod storage;

mod build_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub fn new_appinfo(config_path: &Path, log_path: &Path) -> AppInfo {
    AppInfo {
        version: build_info::PKG_VERSION,
        target: build_info::TARGET,
        profile: build_info::PROFILE,
        features: &build_info::FEATURES[..],
        rustc: build_info::RUSTC_VERSION,
        config_path: config_path.to_owned(),
        log_path: log_path.to_owned(),
    }
}
