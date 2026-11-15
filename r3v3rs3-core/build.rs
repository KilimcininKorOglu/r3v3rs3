fn main() {
    // Without a rerun-if line Cargo keys this script on the newest file time of the package, so
    // the year-2000 file times that .github/scripts/workspace-cache.sh sets reran it and rebuilt
    // every library crate on a tag. Everything the generated file holds (target, profile,
    // features, rustc version) is already part of the fingerprint of the script run.
    println!("cargo:rerun-if-changed=build.rs");
    built::write_built_file().expect("Failed to acquire build-time information");
}
