//! nemobot-host integration test of package binding and WASI grants (decision
//! D4): service packages bind with their grants, a declared permission the
//! config does not grant or a grant naming nothing declared fails closed in
//! `Packages::load` (before any session can open, since a session takes the
//! loaded packages), an extension package that declares a WASI permission is
//! refused, and the consent listing names every declared permission with
//! what the run grants it.
//!
//! The subtree's `scxml.rs/scripts/verify-wasm-capabilities.sh` builds and
//! verifies scxml.rs's example components (the echo service, which declares
//! no WASI permission; the files service, which declares `wasi:filesystem`;
//! and the example extension) into `target/test-service-packages/scxml`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use nemobot_host::FailureKind;
use nemobot_host::config::{CONFIG_FORMAT, Config};
use nemobot_host::env_file::Values;
use nemobot_host::packages::Packages;
use serde_json::{Value as Json, json};
use sha2::{Digest as _, Sha256};

const ECHO_URI: &str = "https://stateforward.org/scxml/services/echo";
const FILES_URI: &str = "https://stateforward.org/scxml/services/files";

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the host crate sits in the nemobot workspace")
}

fn sha256_file(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("the artifact is readable");
    format!("{:x}", Sha256::digest(&bytes))
}

/// The directory of the verified example components.
fn components() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = workspace().join("target/test-service-packages/scxml");
        let status = Command::new("sh")
            .arg("scripts/verify-wasm-capabilities.sh")
            .arg("--artifacts-dir")
            .arg(&dir)
            .args([
                "scxml_service_example",
                "scxml_service_files",
                "scxml_extension_example",
            ])
            .current_dir(workspace().join("scxml.rs"))
            .env("CARGO_TARGET_DIR", workspace().join("target/scxml.rs"))
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("CARGO_TARGET_WASM32_WASIP2_RUSTFLAGS")
            .status()
            .expect("the subtree's component script runs");
        assert!(status.success(), "the example component proof must pass");
        dir
    })
}

/// One package reference: the manifest and the built component with its
/// digest.
fn package(manifest: &Path, component: &str) -> Json {
    let component = components().join(component);
    json!({
        "manifest": manifest.display().to_string(),
        "component": component.display().to_string(),
        "component-sha256": sha256_file(&component),
    })
}

fn echo() -> Json {
    package(
        &workspace().join("scxml.rs/components/scxml-service-example/package/echo.service.json"),
        "scxml_service_example.virtualized.wasm",
    )
}

fn files() -> Json {
    package(
        &workspace().join("scxml.rs/components/scxml-service-files/package/files.service.json"),
        "scxml_service_files.virtualized.wasm",
    )
}

/// The example extension package, copied with a declared WASI permission
/// (its schema beside it).
fn extension_declaring_wasi() -> Json {
    let source = workspace().join("scxml.rs/components/scxml-extension-example/package");
    let dir = workspace().join("target/test-service-packages/wasi-extension");
    std::fs::create_dir_all(&dir).expect("the fixture directory is creatable");
    std::fs::copy(source.join("example.xsd"), dir.join("example.xsd")).expect("the schema copies");
    let mut manifest: Json = serde_json::from_slice(
        &std::fs::read(source.join("example.extension.json")).expect("readable"),
    )
    .expect("the manifest is JSON");
    manifest["wasi-permissions"] =
        json!([{"interface": "wasi:filesystem", "mount": "workspace", "access": "read-write"}]);
    let path = dir.join("example.extension.json");
    std::fs::write(&path, manifest.to_string()).expect("the manifest is writable");
    package(&path, "scxml_extension_example.virtualized.wasm")
}

/// A filesystem grant of the files service's `workspace` mount.
fn workspace_grant(uri: &str) -> Json {
    let directory = workspace().join("target/test-service-packages/workspace");
    std::fs::create_dir_all(&directory).expect("the granted directory is creatable");
    json!({"uri": uri, "permission": "wasi:filesystem", "mount": "workspace",
           "directory": directory.display().to_string()})
}

/// A config binding `services` and `extensions` with `grants`. Package
/// binding reads no session component, so the components directory is a
/// placeholder.
fn config(extensions: &[Json], services: &[Json], grants: &[Json]) -> Config {
    let text = json!({
        "format": CONFIG_FORMAT,
        "components": {"dir": "unused", "manifest-sha256": "0".repeat(64)},
        "extensions": extensions,
        "services": services,
        "grants": grants,
        "outbound-http": false,
    })
    .to_string();
    Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the test config is valid")
}

fn grant_failure(config: &Config) -> String {
    let failure = Packages::load(config).expect_err("the binding must fail closed");
    assert_eq!(failure.kind(), FailureKind::Grant, "{}", failure.message());
    failure.message().to_owned()
}

#[test]
fn a_granted_service_binds_and_the_consent_lists_its_permission_and_grant() {
    let config = config(&[], &[echo(), files()], &[workspace_grant(FILES_URI)]);
    let packages = Packages::load(&config).expect("the services bind with their grant");
    assert!(packages.declares_wasi(), "files declares wasi:filesystem");
    packages
        .service_components(&Values::none(), None)
        .expect("the session host accepts the services and the grant");
    let consent = packages.consent(&Values::none());
    let files_line = consent
        .lines()
        .find(|line| line.starts_with("  wasi: "))
        .expect("the files service's permission is listed");
    assert!(
        files_line.contains("workspace")
            && files_line.contains("granted directory")
            && files_line.contains("test-service-packages/workspace"),
        "{consent}"
    );
    assert!(
        consent.contains(&format!("service {ECHO_URI} "))
            && consent.contains(&format!("service {FILES_URI} ")),
        "{consent}"
    );
    assert_eq!(
        consent.matches("  wasi: ").count(),
        1,
        "only files declares a permission: {consent}"
    );
}

#[test]
fn an_ungranted_declared_permission_fails_closed() {
    let message = grant_failure(&config(&[], &[files()], &[]));
    assert!(
        message.contains(FILES_URI) && message.contains("does not grant"),
        "{message}"
    );
}

#[test]
fn a_grant_naming_an_unbound_uri_fails_closed() {
    let message = grant_failure(&config(
        &[],
        &[files()],
        &[
            workspace_grant(FILES_URI),
            workspace_grant("https://stateforward.org/scxml/services/none"),
        ],
    ));
    assert!(message.contains("no bound package declares"), "{message}");
}

#[test]
fn a_grant_of_an_undeclared_permission_fails_closed() {
    let message = grant_failure(&config(&[], &[echo()], &[workspace_grant(ECHO_URI)]));
    assert!(
        message.contains(ECHO_URI) && message.contains("does not declare"),
        "{message}"
    );
}

#[test]
fn an_extension_declaring_a_wasi_permission_is_refused() {
    let message = grant_failure(&config(&[extension_declaring_wasi()], &[], &[]));
    assert!(
        message.contains("https://stateforward.org/scxml/example")
            && message.contains("links no WASI interface into extension components"),
        "{message}"
    );
}
