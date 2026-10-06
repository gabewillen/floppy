//! Integration test of service binding and WASI resources through
//! `scxml-cli run` (decision D15): a service may use exactly the `wasi:*`
//! interfaces its component imports, and the run grants their runtime
//! resources with wasmtime CLI's option names scoped to the service URI
//! (`--dir URI=HOST::GUEST`). An imported resource the run does not grant,
//! a grant naming an unbound URI, and a grant for an interface the
//! component does not import fail closed before anything runs; the consent
//! listing names every service's WASI imports and the resources the run
//! grants it. A live session runs the files service through its granted
//! directory.
//!
//! The subtree's `scxml.rs/scripts/verify-wasm-capabilities.sh` builds and
//! verifies scxml.rs's example components (the echo service, which imports
//! no WASI interface but the logical clock, and the files service, which
//! imports `wasi:filesystem`) into `target/test-service-packages/scxml`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use support::{Components, Run, components, logs, output, workspace, write};

const ECHO_URI: &str = "https://stateforward.org/scxml/services/echo";
const FILES_URI: &str = "https://stateforward.org/scxml/services/files";

/// The directory of the verified example components.
fn examples() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = workspace().join("target/test-service-packages/scxml");
        let status = Command::new("sh")
            .arg("scripts/verify-wasm-capabilities.sh")
            .arg("--artifacts-dir")
            .arg(&dir)
            .args(["scxml_service_example", "scxml_service_files"])
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

fn echo_manifest() -> PathBuf {
    workspace().join("scxml.rs/components/scxml-service-example/package/echo.service.json")
}

fn service(manifest: &Path, uri: &str, component: &str) -> Vec<String> {
    Components::package_args("service", manifest, uri, &examples().join(component))
}

fn echo() -> Vec<String> {
    service(
        &echo_manifest(),
        ECHO_URI,
        "scxml_service_example.virtualized.wasm",
    )
}

fn files() -> Vec<String> {
    service(
        &workspace().join("scxml.rs/components/scxml-service-files/package/files.service.json"),
        FILES_URI,
        "scxml_service_files.virtualized.wasm",
    )
}

/// The directory the run preopens for the files service.
fn workspace_directory() -> PathBuf {
    let directory = workspace().join("target/test-service-packages/workspace");
    std::fs::create_dir_all(&directory).expect("the granted directory is creatable");
    directory
}

/// `--dir` preopening that directory at `/workspace` for `uri`.
fn workspace_grant(uri: &str) -> Vec<String> {
    vec![
        "--dir".into(),
        format!("{uri}={}::/workspace", workspace_directory().display()),
    ]
}

/// A document that uses no package.
const IDLE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="s"><state id="s"/></scxml>"#;

/// `scxml-cli run` of `document` with the session components and `bindings`,
/// no input.
fn run_with(bindings: &[Vec<String>], document: &str) -> support::Output {
    let path = write(&support::unique_scratch("packages"), "doc.scxml", document);
    let mut args = vec!["run".to_owned()];
    args.extend(components().session_args());
    args.extend(bindings.iter().flatten().cloned());
    args.extend([
        "--clock".to_owned(),
        "logical".to_owned(),
        "--on-eof".to_owned(),
        "close".to_owned(),
        path.display().to_string(),
    ]);
    output(&args)
}

/// The binding fails closed before anything runs: exit 2, no record.
fn refused(bindings: &[Vec<String>]) -> String {
    let output = run_with(bindings, IDLE);
    assert_eq!(output.status.code(), Some(2), "{}", output.stderr);
    assert!(output.stdout.is_empty(), "nothing ran: {}", output.stdout);
    output.stderr
}

#[test]
fn a_granted_service_binds_and_the_consent_lists_its_imports_and_resources() {
    let output = run_with(&[echo(), files(), workspace_grant(FILES_URI)], IDLE);
    assert_eq!(
        output.status.code(),
        Some(3),
        "the session ran and closed at the end of input: {}",
        output.stderr
    );
    let consent = &output.stderr;
    assert!(
        consent.contains(&format!("service `{ECHO_URI}`"))
            && consent.contains(&format!("service `{FILES_URI}`")),
        "{consent}"
    );
    let files_imports = consent
        .lines()
        .find(|line| line.starts_with("  = WASI imports: ") && line.contains("wasi:filesystem"))
        .expect("the files service's imports are listed");
    assert!(
        files_imports.contains("wasi:filesystem/preopens"),
        "{consent}"
    );
    let resources = consent
        .lines()
        .find(|line| line.starts_with("  = WASI resources: --dir "))
        .expect("the files service's directory is listed");
    assert!(
        resources.contains("test-service-packages/workspace::/workspace"),
        "the listing names the granted directory and its guest path: {consent}"
    );
    assert_eq!(
        consent.matches("  = WASI resources: none").count(),
        1,
        "only files is granted a resource: {consent}"
    );
}

#[test]
fn an_ungranted_imported_resource_fails_closed() {
    let message = refused(&[files()]);
    assert!(
        message.contains("imports a WASI interface whose resource the run does not grant"),
        "{message}"
    );
}

#[test]
fn a_grant_naming_an_unbound_uri_fails_closed() {
    let message = refused(&[
        files(),
        workspace_grant(FILES_URI),
        workspace_grant("https://stateforward.org/scxml/services/none"),
    ]);
    assert!(
        message.contains("https://stateforward.org/scxml/services/none")
            && message.contains("not the service URI of any bound service"),
        "{message}"
    );
}

#[test]
fn a_grant_for_an_interface_the_component_does_not_import_fails_closed() {
    let message = refused(&[echo(), workspace_grant(ECHO_URI)]);
    assert!(
        message.contains("grants a WASI resource no bound service component imports"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------
// Live sessions.

/// Starts a live session of `document` with `bindings` on the logical
/// clock.
fn start(bindings: &[Vec<String>], document: &str) -> Run {
    let path = write(&support::unique_scratch("packages"), "doc.scxml", document);
    let mut args = components().session_args();
    args.extend(bindings.iter().flatten().cloned());
    args.extend([
        "--clock".to_owned(),
        "logical".to_owned(),
        path.display().to_string(),
    ]);
    Run::start(&args, &[])
}

/// Reads `seed.txt` and writes `out.txt` in the files service's workspace,
/// then finishes.
const FILES_DOCUMENT: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke type="https://stateforward.org/scxml/services/files" id="files">
      <param name="seed" expr="'seed.txt'"/>
      <param name="name" expr="'out.txt'"/>
      <param name="text" expr="'written by the service'"/>
    </invoke>
    <transition event="done.invoke.files" target="done">
      <log label="done" expr="JSON.stringify(_event.data)"/>
    </transition>
    <transition event="error.*" target="done">
      <log label="failed" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#;

/// The run's `--dir` reaches the files service at run time: the host links
/// `wasi:filesystem` with that directory preopened.
#[test]
fn a_granted_files_service_reads_and_writes_its_directory() {
    let directory = workspace_directory();
    std::fs::write(directory.join("seed.txt"), "from the host").expect("seed is written");
    let _ = std::fs::remove_file(directory.join("out.txt"));
    let mut run = start(&[files(), workspace_grant(FILES_URI)], FILES_DOCUMENT);
    run.end_input();
    let finished = run.finish(Duration::from_secs(60));
    assert_eq!(
        logs(&finished.records),
        [(
            "done".to_owned(),
            r#"{"seed":"from the host","wrote":22}"#.to_owned()
        )],
        "the service read the seed through its granted mount: {}",
        finished.stderr
    );
    assert!(support::finished(&finished.records));
    assert_eq!(
        std::fs::read_to_string(directory.join("out.txt")).expect("the service wrote out.txt"),
        "written by the service"
    );
}
