//! nemobot-host integration test of package binding and WASI grants (decision
//! D4): service packages bind with their grants, a declared permission the
//! config does not grant or a grant naming nothing declared fails closed in
//! `Packages::load` (before any session can open, since a session takes the
//! loaded packages), an extension package that declares a WASI permission is
//! refused, and the consent listing names every declared permission with
//! what the run grants it. A live session through `SessionHandle` runs
//! `<invoke type="<service URI>">` of the echo service bound with a declared
//! event contract: a conforming emit is delivered, and an emit the contract
//! refuses retires the service (`error.communication`).
//!
//! The subtree's `scxml.rs/scripts/verify-wasm-capabilities.sh` builds and
//! verifies scxml.rs's example components (the echo service, which declares
//! no WASI permission; the files service, which declares `wasi:filesystem`;
//! and the example extension) into `target/test-service-packages/scxml`;
//! `scripts/build-components.sh --session-only` builds the session
//! components into `target/test-service-packages/components`.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use nemobot_host::FailureKind;
use nemobot_host::compile::compile_source;
use nemobot_host::config::{CONFIG_FORMAT, Config};
use nemobot_host::env_file::Values;
use nemobot_host::packages::Packages;
use nemobot_host::runtime::{ClockMode, Egress, SessionHandle, SessionSpec, SessionStatus};
use scxml_host_wasmtime::{ScriptedEvent, SessionOutput};
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
    let consent = packages.consent();
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
            && message.contains("WASI belongs to invoked services"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------
// A live session invoking a contracted service.

/// The verified session components directory and its manifest digest.
fn session_components() -> &'static (PathBuf, String) {
    static COMPONENTS: OnceLock<(PathBuf, String)> = OnceLock::new();
    COMPONENTS.get_or_init(|| {
        let artifacts = workspace().join("target/test-service-packages/components");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/build-components.sh"))
            .arg("--session-only")
            .arg("--artifacts-dir")
            .arg(&artifacts)
            .current_dir(workspace())
            .status()
            .expect("the build script runs");
        assert!(status.success(), "the session component proof must pass");
        let components = artifacts.join("scxml");
        let digest = sha256_file(&components.join("manifest.tsv"));
        (components, digest)
    })
}

/// The echo package with a declared contract: it accepts `say` carrying an
/// object with `text`, and emits `echo.say` whose `text` is a string.
fn contracted_echo() -> Json {
    let source = workspace().join("scxml.rs/components/scxml-service-example/package");
    let dir = workspace().join("target/test-service-packages/contracted-echo");
    std::fs::create_dir_all(&dir).expect("the fixture directory is creatable");
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
    let path = dir.join("echo.service.json");
    std::fs::write(&path, manifest.to_string()).expect("the manifest is writable");
    package(&path, "scxml_service_example.virtualized.wasm")
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

fn say(text: &Json) -> ScriptedEvent {
    ScriptedEvent {
        name: "say".into(),
        data: scxml_types::DataValue::JsonObject(json!({"text": text}).to_string()),
        trace_context: None,
    }
}

/// The logs of egress records until `done` holds for one, at most 30 s.
fn logs_until(handle: &SessionHandle, done: impl Fn(&Egress) -> bool) -> (Vec<String>, Egress) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logs = Vec::new();
    while Instant::now() < deadline {
        if let Some(record) = handle.recv_timeout(Duration::from_millis(100)) {
            if let Egress::Outputs(outputs) = &record {
                logs.extend(outputs.iter().filter_map(|output| match output {
                    SessionOutput::Log { entry, .. } => Some(format!(
                        "{}: {}",
                        entry.label.clone().unwrap_or_default(),
                        entry.text
                    )),
                    SessionOutput::Done { .. } => None,
                }));
            }
            if done(&record) {
                return (logs, record);
            }
        }
    }
    panic!("no expected egress record within 30 s; logs {logs:?}");
}

/// Opens a live session of `document` through `nemobot-host` with the real
/// session components, `services` bound with `grants`, and returns it with
/// the logs up to its opening.
fn open_session(
    services: &[Json],
    grants: &[Json],
    document: &str,
) -> (SessionHandle, Vec<String>) {
    let (components, digest) = session_components();
    let text = json!({
        "format": CONFIG_FORMAT,
        "components": {"dir": components.display().to_string(), "manifest-sha256": digest},
        "services": services,
        "grants": grants,
        "outbound-http": false,
    })
    .to_string();
    let config = Config::parse(text.as_bytes(), &PathBuf::from("/")).expect("the config is valid");
    let packages = Packages::load(&config).expect("the services bind");
    let compiled = compile_source(
        "service.scxml",
        document.to_owned(),
        scxml_event_schema::Resources::new(),
        &packages,
    )
    .expect("the document compiles");
    let handle = SessionHandle::open(
        &config,
        packages,
        Values::none(),
        SessionSpec {
            compiled,
            session_id: "service".into(),
            clock: ClockMode::Logical,
            documents: Vec::new(),
        },
    )
    .expect("the session opens with the services bound");
    let (logs, _) = logs_until(&handle, |record| matches!(record, Egress::Opened { .. }));
    (handle, logs)
}

#[test]
fn a_contracted_service_delivers_a_conforming_emit_and_fails_closed_on_another() {
    let (handle, _) = open_session(&[contracted_echo()], &[], CONTRACT_DOCUMENT);

    handle
        .send(say(&json!("hi")))
        .expect("the session takes say");
    let (logs, _) = logs_until(&handle, |record| matches!(record, Egress::Outputs(_)));
    assert_eq!(
        logs,
        [r#"echo.say: {"text":"hi"}"#],
        "a conforming emit is delivered"
    );

    // `say` is accepted (`text` is present), but the echo's `text` is no
    // string: the emit breaks the contract, the host retires the service,
    // and the session gets error.communication with the invoke id.
    handle.send(say(&json!(5))).expect("the session takes say");
    let (logs, closed) = logs_until(&handle, |record| matches!(record, Egress::Closed { .. }));
    assert_eq!(
        logs,
        ["failed: echo"],
        "the refused emit is never delivered"
    );
    assert!(
        matches!(
            closed,
            Egress::Closed {
                status: SessionStatus::Finished,
                ..
            }
        ),
        "{closed:?}"
    );
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

/// The config's filesystem grant reaches the files service at run time:
/// `nemobot-host` links `wasi:filesystem` for the granted directory.
#[test]
fn a_granted_files_service_reads_and_writes_its_directory_through_nemobot_host() {
    let grant = workspace_grant(FILES_URI);
    let directory = PathBuf::from(grant["directory"].as_str().expect("the grant's directory"));
    std::fs::write(directory.join("seed.txt"), "from the host").expect("seed is written");
    let _ = std::fs::remove_file(directory.join("out.txt"));
    let (handle, mut logs) = open_session(&[files()], &[grant], FILES_DOCUMENT);
    let (rest, closed) = logs_until(&handle, |record| matches!(record, Egress::Closed { .. }));
    logs.extend(rest);
    assert_eq!(
        logs,
        [r#"done: {"seed":"from the host","wrote":22}"#],
        "the service read the seed through its granted mount"
    );
    assert!(
        matches!(
            closed,
            Egress::Closed {
                status: SessionStatus::Finished,
                ..
            }
        ),
        "{closed:?}"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("out.txt")).expect("the service wrote out.txt"),
        "written by the service"
    );
}
