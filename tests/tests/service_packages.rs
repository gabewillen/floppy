//! Integration test of package binding and WASI grants through
//! `scxml-cli run` (decision D4): service packages bind with their grants;
//! a declared permission the run does not grant, a grant naming nothing
//! declared, and an extension package that declares a WASI permission fail
//! closed before anything runs; and the consent listing names every
//! declared permission with what the run grants it. Live sessions run
//! `<invoke type="<service URI>">` of the echo service bound with a
//! declared event contract (a conforming emit is delivered, and an emit the
//! contract refuses retires the service with `error.communication`) and of
//! the files service through its granted directory.
//!
//! The subtree's `scxml.rs/scripts/verify-wasm-capabilities.sh` builds and
//! verifies scxml.rs's example components (the echo service, which declares
//! no WASI permission; the files service, which declares `wasi:filesystem`;
//! and the example extension) into `target/test-service-packages/scxml`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{Value as Json, json};
use sha2::{Digest as _, Sha256};
use support::{Components, Run, components, logs, output, scratch, workspace, write};

const ECHO_URI: &str = "https://stateforward.org/scxml/services/echo";
const FILES_URI: &str = "https://stateforward.org/scxml/services/files";
const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";

/// The directory of the verified example components.
fn examples() -> &'static Path {
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

/// The example extension package, copied with a declared WASI permission
/// (its schema beside it).
fn extension_declaring_wasi() -> Vec<String> {
    let source = workspace().join("scxml.rs/components/scxml-extension-example/package");
    let dir = scratch("wasi-extension");
    std::fs::copy(source.join("example.xsd"), dir.join("example.xsd")).expect("the schema copies");
    let mut manifest: Json = serde_json::from_slice(
        &std::fs::read(source.join("example.extension.json")).expect("readable"),
    )
    .expect("the manifest is JSON");
    manifest["wasi-permissions"] =
        json!([{"interface": "wasi:filesystem", "mount": "workspace", "access": "read-write"}]);
    let path = write(&dir, "example.extension.json", &manifest.to_string());
    Components::package_args(
        "extension",
        &path,
        EXAMPLE_URI,
        &examples().join("scxml_extension_example.virtualized.wasm"),
    )
}

/// The granted directory of the files service's `workspace` mount.
fn workspace_directory() -> PathBuf {
    let directory = workspace().join("target/test-service-packages/workspace");
    std::fs::create_dir_all(&directory).expect("the granted directory is creatable");
    directory
}

/// A filesystem grant of the `workspace` mount to `uri`.
fn workspace_grant(uri: &str) -> Vec<String> {
    vec![
        "--grant".into(),
        format!(
            "{uri}=wasi:filesystem=workspace={}",
            workspace_directory().display()
        ),
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
fn a_granted_service_binds_and_the_consent_lists_its_permission_and_grant() {
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
    let files_line = consent
        .lines()
        .find(|line| line.starts_with("  = WASI: wasi:filesystem"))
        .expect("the files service's permission is listed");
    assert!(
        files_line.contains("workspace") && files_line.contains("test-service-packages/workspace"),
        "the listing names the declared mount and the granted directory: {consent}"
    );
    assert_eq!(
        consent.matches("  = WASI: none").count(),
        1,
        "only files declares a permission: {consent}"
    );
}

#[test]
fn an_ungranted_declared_permission_fails_closed() {
    let message = refused(&[files()]);
    assert!(
        message.contains("files.service.json")
            && message.contains("declares wasi:filesystem")
            && message.contains("the run does not grant"),
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
            && message.contains("not the service URI of any --service-package"),
        "{message}"
    );
}

#[test]
fn a_grant_of_an_undeclared_permission_fails_closed() {
    let message = refused(&[echo(), workspace_grant(ECHO_URI)]);
    assert!(
        message.contains(ECHO_URI) && message.contains("does not declare"),
        "{message}"
    );
}

#[test]
fn an_extension_declaring_a_wasi_permission_is_refused() {
    let message = refused(&[extension_declaring_wasi()]);
    assert!(
        message.contains(EXAMPLE_URI) && message.contains("WASI belongs to invoked services"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------
// Live sessions.

/// The echo package with a declared contract: it accepts `say` carrying an
/// object with `text`, and emits `echo.say` whose `text` is a string.
fn contracted_echo() -> Vec<String> {
    let source = workspace().join("scxml.rs/components/scxml-service-example/package");
    let dir = scratch("contracted-echo");
    let schemas = br#"{"$defs":{"Echo":{"properties":{"text":{"type":"string"}},"type":"object"},"Say":{"required":["text"],"type":"object"}}}"#;
    std::fs::write(dir.join("events.json"), schemas).expect("the schemas are writable");
    let digest = format!("{:x}", Sha256::digest(schemas));
    let mut manifest: Json =
        serde_json::from_slice(&std::fs::read(source.join("echo.service.json")).expect("readable"))
            .expect("the manifest is JSON");
    manifest["events"] = json!({
        "emits": [{"name": "echo.say", "schema-artifact": "events.json#/$defs/Echo", "schema-sha256": digest}],
        "accepts": [{"name": "say", "schema-artifact": "events.json#/$defs/Say", "schema-sha256": digest}],
    });
    let path = write(&dir, "echo.service.json", &manifest.to_string());
    service(&path, ECHO_URI, "scxml_service_example.virtualized.wasm")
}

/// `s` invokes the echo service and relays `say` to it with the event's
/// data; it logs each `echo.say` and finishes in `failed` when the service
/// fails.
const CONTRACT_DOCUMENT: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke type="https://stateforward.org/scxml/services/echo" id="echo"/>
    <transition event="say"><send event="say" target="#_echo"><content expr="_event.data"/></send></transition>
    <transition event="echo.say"><log label="echo.say" expr="JSON.stringify(_event.data)"/></transition>
    <transition event="error.communication" target="failed"><log label="failed" expr="_event.invokeid"/></transition>
  </state>
  <final id="failed"/>
</scxml>"##;

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

fn is_log(record: &Json) -> bool {
    record["type"] == "log"
}

#[test]
fn a_contracted_service_delivers_a_conforming_emit_and_fails_closed_on_another() {
    let mut run = start(&[contracted_echo()], CONTRACT_DOCUMENT);
    run.event("say", &json!({"text": "hi"}));
    let records = run.until(Duration::from_secs(60), is_log);
    assert_eq!(
        logs(&records),
        [("echo.say".to_owned(), r#"{"text":"hi"}"#.to_owned())],
        "a conforming emit is delivered"
    );

    // `say` is accepted (`text` is present), but the echo's `text` is no
    // string: the emit breaks the contract, the host retires the service,
    // and the session gets error.communication with the invoke id.
    run.event("say", &json!({"text": 5}));
    run.end_input();
    let finished = run.finish(Duration::from_secs(60));
    assert_eq!(
        logs(&finished.records),
        [("failed".to_owned(), "echo".to_owned())],
        "the refused emit is never delivered"
    );
    assert!(support::finished(&finished.records), "{}", finished.stderr);
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

/// The run's filesystem grant reaches the files service at run time: the
/// host links `wasi:filesystem` for the granted directory.
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
