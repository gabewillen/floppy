//! The one verified release a whole test run may share.
//!
//! `SCXML_COMPONENTS_DIR` names a release directory that
//! `scripts/verify-wasm-capabilities.sh` wrote with no component arguments
//! (so it holds every component, the packaged machine, and the scripted
//! machine), and `SCXML_COMPONENTS_MANIFEST_SHA256` is the trusted SHA-256 of
//! its `manifest.tsv`. With both set, a suite reads its components from that
//! directory instead of running the script into its own; with neither, each
//! suite builds and verifies its own release as before. The directory is
//! only read, so concurrently running suites can share it.
//!
//! The host's integration tests include this file as `mod prebuilt;`; the
//! host's unit tests (`src/lib.rs`) and the CLI's integration tests
//! (`crates/scxml-cli/tests`) include it through `#[path]`.

use std::path::PathBuf;

use sha2::{Digest as _, Sha256};

/// The shared release directory, or `None` when the environment names none.
///
/// # Panics
///
/// When only one of the two variables is set, when the manifest cannot be
/// read, or when its SHA-256 is not the trusted one: a misconfigured shared
/// release fails the suite instead of falling back to another release.
pub fn release_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("SCXML_COMPONENTS_DIR");
    let trusted = std::env::var("SCXML_COMPONENTS_MANIFEST_SHA256").ok();
    match (dir, trusted) {
        (None, None) => None,
        (Some(dir), Some(trusted)) => {
            let dir = PathBuf::from(dir);
            let manifest = dir.join("manifest.tsv");
            let bytes = std::fs::read(&manifest).unwrap_or_else(|error| {
                panic!(
                    "SCXML_COMPONENTS_DIR names no readable release manifest `{}`: {error}",
                    manifest.display()
                )
            });
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                trusted,
                "`{}` is not the release SCXML_COMPONENTS_MANIFEST_SHA256 trusts",
                manifest.display()
            );
            Some(dir)
        }
        _ => panic!(
            "set both SCXML_COMPONENTS_DIR and SCXML_COMPONENTS_MANIFEST_SHA256 (a shared release and its trusted manifest digest), or neither"
        ),
    }
}
