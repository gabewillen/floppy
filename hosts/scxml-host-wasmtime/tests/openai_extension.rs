//! The shipped `openai` namespace (`namespaces/openai`) under the Wasmtime
//! session host, against an OpenAI-compatible mock server this suite runs on
//! 127.0.0.1: the extension component reaches the network only through the
//! host-granted, allowlisted `outbound-http` capability
//! (`ExtensionHttpClient`), every request carries the executing session's
//! snapshot and the events it accepts, a function call the model returns is
//! sent as the original event, and every failure raises `error.execution`
//! in the session.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies every component
//! into this suite's own artifact directory.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "http")]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument, EnvVariable,
    ExtensionComponents, ExtensionEnvironment, ExtensionHttpClient, ExtensionHttpConfig,
    ExtensionPackage, MachineScripts, ScriptedEvent, SessionEmbedder, SessionHost,
    SessionHostComponents, SessionOutcome, SessionRun, SnapshotRequest, TelemetryProviders,
    TrustedComponentDigest, TrustedManifestDigest, WasmtimeHostConfig, authenticate_component,
    authenticate_extension_component,
};
use sha2::{Digest as _, Sha256};

const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);
const OPENAI_URI: &str = "https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0";

const COMPONENTS: [(ArtifactKind, &str); 8] = [
    (ArtifactKind::Machine, "scxml_machine"),
    (ArtifactKind::Compiler, "scxml_compiler"),
    (
        ArtifactKind::DataModelEcmascript,
        "scxml_data_model_ecmascript",
    ),
    (ArtifactKind::DataModelXpath, "scxml_data_model_xpath"),
    (ArtifactKind::DataModelNull, "scxml_data_model_null"),
    (ArtifactKind::IoProcessorScxml, "scxml_ioprocessor_scxml"),
    (
        ArtifactKind::IoProcessorBasicHttp,
        "scxml_ioprocessor_basichttp",
    ),
    (ArtifactKind::Clock, "scxml_clock"),
];

/// The openai component file: a repository namespace, so the release
/// manifest does not list it; it is authenticated by its own SHA-256 like
/// every caller-supplied extension component.
const OPENAI_COMPONENT: &str = "scxml_extension_openai.virtualized.wasm";

/// The Responses API result the mock answers on `/call/...`: one call of
/// the function for `answer.yes`, with data.
const CALL_BODY: &str = r#"{"id":"resp_2","object":"response","status":"completed","output":[{"type":"function_call","id":"fc_1","call_id":"call_1","name":"send_answer_yes","arguments":"{\"data\":{\"confidence\":0.9}}","status":"completed"}]}"#;
/// The Responses API result the mock answers on `/unknown/...`: a call of a
/// function the tools never declared.
const UNKNOWN_BODY: &str = r#"{"id":"resp_3","object":"response","status":"completed","output":[{"type":"function_call","id":"fc_1","call_id":"call_1","name":"send_answer_maybe","arguments":"{}","status":"completed"}]}"#;
/// The Responses API result the mock answers on `/ok/...`.
const OK_BODY: &str = r#"{"id":"resp_1","object":"response","status":"completed","output":[{"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello from the mock","annotations":[]}]}]}"#;

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
}

fn artifacts() -> std::path::PathBuf {
    workspace().join("target/session-host-proof/wasm-artifacts/openai-namespace")
}

fn release() -> &'static [AuthenticatedComponent] {
    static RELEASE: OnceLock<Vec<AuthenticatedComponent>> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let target = workspace().join("target/session-host-proof");
        let artifacts = artifacts();
        let status = Command::new("sh")
            .arg(workspace().join("scripts/verify-wasm-capabilities.sh"))
            .args(COMPONENTS.iter().map(|(_, name)| *name))
            .arg("scxml_extension_openai")
            .current_dir(workspace())
            .env("CARGO_TARGET_DIR", &target)
            .env("SCXML_WASM_ARTIFACTS_DIR", &artifacts)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        let manifest = std::fs::read(artifacts.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        COMPONENTS
            .iter()
            .map(|(kind, name)| {
                let path = format!("{name}.virtualized.wasm");
                let bytes = std::fs::read(artifacts.join(&path)).expect("artifact is readable");
                authenticate_component(&manifest, trusted, *kind, &path, &bytes)
                    .expect("artifact authenticates")
            })
            .collect()
    })
}

/// The openai component, authenticated by the SHA-256 of the bytes the
/// capability script just built and verified.
fn openai_component() -> &'static AuthenticatedComponent {
    static OPENAI: OnceLock<AuthenticatedComponent> = OnceLock::new();
    OPENAI.get_or_init(|| {
        release();
        let bytes = std::fs::read(artifacts().join(OPENAI_COMPONENT))
            .expect("the openai component is readable");
        authenticate_extension_component(
            &bytes,
            TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(&bytes)))
                .expect("a canonical digest"),
        )
        .expect("the openai component authenticates")
    })
}

fn component(kind: ArtifactKind) -> &'static AuthenticatedComponent {
    let index = COMPONENTS
        .iter()
        .position(|(listed, _)| *listed == kind)
        .expect("the suite builds this kind");
    &release()[index]
}

fn package(http_origins: &[&str]) -> ExtensionPackage {
    let schema = std::fs::read(workspace().join("namespaces/openai/package/openai.xsd"))
        .expect("the openai schema is readable");
    ExtensionPackage {
        namespace_uri: OPENAI_URI.into(),
        package_version: "0.2.0".into(),
        extension_artifact: OPENAI_COMPONENT.into(),
        schema_artifact: "openai.xsd".into(),
        schema_sha256: format!("{:x}", Sha256::digest(&schema)),
        wit_world: "stateforward:scxml/executable-content-extension".into(),
        wit_version: "0.2.0".into(),
        env_slots: vec!["apikey".into(), "baseurl".into()],
        http_origins: http_origins
            .iter()
            .map(|origin| (*origin).to_owned())
            .collect(),
        wasi_permissions: Vec::new(),
        schema,
    }
}

/// One request the mock received.
#[derive(Clone, Debug)]
struct Received {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// An OpenAI-compatible mock on 127.0.0.1: `POST /ok/v1/responses`
/// answers [`OK_BODY`], `POST /fail/v1/responses` answers 500, anything
/// else 404. Every request is recorded.
struct Mock {
    origin: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("the mock has an address")
        );
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        headers.push((name.trim().to_owned(), value.trim().to_owned()));
                    }
                }
                let length = headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let target = request_line.split(' ').nth(1).unwrap_or("").to_owned();
                log.lock().expect("the log lock").push(Received {
                    request_line: request_line.trim_end().to_owned(),
                    headers,
                    body,
                });
                let (status, answer) = match target.as_str() {
                    "/ok/v1/responses" => ("200 OK", OK_BODY.to_owned()),
                    "/call/v1/responses" => ("200 OK", CALL_BODY.to_owned()),
                    "/unknown/v1/responses" => ("200 OK", UNKNOWN_BODY.to_owned()),
                    "/fail/v1/responses" => (
                        "500 Internal Server Error",
                        r#"{"error":{"message":"boom"}}"#.to_owned(),
                    ),
                    _ => ("404 Not Found", "{}".to_owned()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self { origin, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log lock").clone()
    }
}

/// No documents, no scripts.
struct NoResources;

impl SessionEmbedder for NoResources {
    fn document(&self, _kind: DocumentKind, _uri: &str) -> Option<EmbeddedDocument> {
        None
    }

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

/// A session host binding the openai namespace (its package declaring
/// `declared`) with `variables` as the run's values, granted to the openai
/// namespace, and, when `outbound`, an outbound transport.
fn host(variables: &[(&str, String)], outbound: bool, declared: &[&str]) -> SessionHost {
    granted_host(variables, OPENAI_URI, outbound, declared)
}

/// [`host`] with `variables` granted to `granted_to` only.
fn granted_host(
    variables: &[(&str, String)],
    granted_to: &str,
    outbound: bool,
    declared: &[&str],
) -> SessionHost {
    let values = ExtensionEnvironment::new(
        variables
            .iter()
            .map(|(name, value)| EnvVariable::new(*name, value.clone(), [granted_to.to_owned()])),
    )
    .expect("valid variables");
    let mut extensions = ExtensionComponents::new()
        .with_values(values)
        .bind(package(declared), openai_component())
        .expect("the openai namespace binds");
    if outbound {
        let client = ExtensionHttpClient::new(ExtensionHttpConfig::new()).expect("a valid client");
        extensions = extensions.with_outbound_http(Arc::new(client));
    }
    SessionHost::new(
        &SessionHostComponents {
            machine: component(ArtifactKind::Machine),
            compiler: component(ArtifactKind::Compiler),
            data_model_ecmascript: component(ArtifactKind::DataModelEcmascript),
            data_model_xpath: component(ArtifactKind::DataModelXpath),
            data_model_null: component(ArtifactKind::DataModelNull),
            io_scxml: component(ArtifactKind::IoProcessorScxml),
            io_basichttp: component(ArtifactKind::IoProcessorBasicHttp),
            clock: component(ArtifactKind::Clock),
        },
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| host.with_extension_components(extensions))
    .expect("the verified components link")
}

/// The machine under test: it generates into `reply`, moves to `answered`
/// when the reply is the mock's text, and to `failed` on `error.execution`.
const MACHINE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:openai="https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0" datamodel="ecmascript" initial="ask">
  <datamodel><data id="reply" expr="''"/><data id="topic" expr="'greetings'"/></datamodel>
  <state id="ask">
    <onentry>
      <openai:generate env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL" model="test-model" prompt="Be brief." promptexpr="'Topic: ' + topic" location="reply" max-output-tokens="32">
        <openai:prompt>Say hello.</openai:prompt>
      </openai:generate>
      <log label="reply" expr="reply"/>
    </onentry>
    <transition cond="reply === 'hello from the mock'" target="answered"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="answered"/>
  <final id="failed"/>
</scxml>"#;

/// Function-calling mode: no `location`, so the model must pick one of the
/// events `ask` accepts; the chosen event moves the machine.
const CALLING_MACHINE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:openai="https://github.com/stateforward/scxml.rs/namespaces/openai@v0.2.0" xmlns:env="https://github.com/stateforward/scxml.rs/namespaces/env" version="1.0" datamodel="ecmascript" initial="ask">
  <datamodel><data id="secret" expr="'do-not-send'"/></datamodel>
  <state id="ask">
    <onentry>
      <openai:generate env:apikey="OPENAI_API_KEY" env:baseurl="OPENAI_BASE_URL" model="test-model" prompt="Is the sky blue?"/>
    </onentry>
    <transition event="answer.yes" target="yes"><log label="confidence" expr="_event.data.confidence"/></transition>
    <transition event="answer.no" target="no"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="yes"/>
  <final id="no"/>
  <final id="failed"/>
</scxml>"#;

fn run(host: &SessionHost) -> SessionOutcome {
    run_machine(host, MACHINE)
}

fn run_machine(host: &SessionHost, machine: &str) -> SessionOutcome {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "openai.scxml",
        machine,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let bytes = ir.to_json().expect("fixture IR encodes");
    let events: [ScriptedEvent; 0] = [];
    host.run(&SessionRun {
        machine_ir: &bytes,
        data_model: ir.data_model.as_deref(),
        session_id: "root",
        events: &events,
        external_client: None,
        embedder: Rc::new(NoResources),
        max_commands: 256,
        snapshots: SnapshotRequest::none(),
    })
    .expect("the run completes")
}

fn key_and_base(base: String) -> Vec<(&'static str, String)> {
    vec![
        ("OPENAI_API_KEY", "sk-test-key".to_owned()),
        ("OPENAI_BASE_URL", base),
    ]
}

#[test]
fn a_generated_reply_is_stored_at_location_and_drives_the_transition() {
    let mock = Mock::start();
    let host = host(&key_and_base(format!("{}/ok/v1", mock.origin)), true, &[]);
    let outcome = run(&host);
    assert_eq!(outcome.configuration, ["answered"]);
    let texts: Vec<_> = outcome
        .root_logs
        .iter()
        .map(|log| log.text.clone())
        .collect();
    assert_eq!(texts, ["hello from the mock"]);
    let [request] = mock.received().try_into().expect("exactly one request");
    assert_eq!(request.request_line, "POST /ok/v1/responses HTTP/1.1");
    assert_eq!(request.header("authorization"), Some("Bearer sk-test-key"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    let body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("the request body is JSON");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["max_output_tokens"], 32);
    assert_eq!(
        body["input"][1],
        serde_json::json!({ "role": "user", "content": "Be brief.\nTopic: greetings\nSay hello." })
    );
    assert_eq!(body["input"][0]["role"], "system");
    assert_eq!(
        tool_names(&body),
        ["send_error_execution"],
        "with a location the accepted events are offered, not required"
    );
    assert_eq!(body["tool_choice"], "auto");
}

fn tool_names(body: &serde_json::Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name").to_owned())
        .collect()
}

/// (a) the system message is the executing session's pruned snapshot and
/// the tools are the events its active transitions accept; (b) the model's
/// function call is sent as the original event and drives the machine.
#[test]
fn a_function_call_sends_the_chosen_event_and_drives_the_machine() {
    let mock = Mock::start();
    let host = host(&key_and_base(format!("{}/call/v1", mock.origin)), true, &[]);
    let outcome = run_machine(&host, CALLING_MACHINE);
    assert_eq!(outcome.configuration, ["yes"], "answer.yes was sent");
    let texts: Vec<_> = outcome
        .root_logs
        .iter()
        .map(|log| log.text.clone())
        .collect();
    assert_eq!(texts, ["0.9"], "the call's data is the event's data");
    let [request] = mock.received().try_into().expect("exactly one request");
    let body: serde_json::Value =
        serde_json::from_slice(&request.body).expect("the request body is JSON");
    assert_eq!(body["tool_choice"], "required");
    assert_eq!(
        tool_names(&body),
        ["send_answer_yes", "send_answer_no", "send_error_execution"]
    );
    assert_eq!(body["input"][0]["role"], "system");
    let system = body["input"][0]["content"].as_str().expect("system text");
    assert!(
        scxml_snapshot::check(system.as_bytes())
            .expect("a structurally valid snapshot")
            .is_empty(),
        "the system message is a canonical scxml.session-snapshot/v2 document"
    );
    let snapshot: serde_json::Value = serde_json::from_str(system).expect("JSON");
    assert_eq!(
        snapshot["options"]["exclude"],
        serde_json::json!(["data_model", "transitions", "previews", "invocations"])
    );
    assert_eq!(snapshot["options"]["point"], "executable-content");
    let root = &snapshot["snapshot"]["root"];
    assert_eq!(
        root["configuration"],
        serde_json::json!({ "atomic": ["ask"], "full": ["ask"] })
    );
    assert!(root.get("data_model").is_none() && root.get("transitions").is_none());
    assert!(
        !system.contains("do-not-send"),
        "the data model never leaves the session"
    );
    assert_eq!(body["input"][1]["content"], "Is the sky blue?");
}

/// A call of a function the tools never declared raises `error.execution`
/// and sends nothing.
#[test]
fn a_call_of_an_undeclared_function_raises_error_execution() {
    let mock = Mock::start();
    let host = host(
        &key_and_base(format!("{}/unknown/v1", mock.origin)),
        true,
        &[],
    );
    assert_eq!(
        run_machine(&host, CALLING_MACHINE).configuration,
        ["failed"]
    );
}

#[test]
fn a_non_success_status_raises_error_execution() {
    let mock = Mock::start();
    let host = host(&key_and_base(format!("{}/fail/v1", mock.origin)), true, &[]);
    let outcome = run(&host);
    assert_eq!(outcome.configuration, ["failed"]);
    assert!(outcome.root_logs.is_empty(), "the failure aborts the block");
    assert_eq!(mock.received().len(), 1, "the request was sent");
}

#[test]
fn a_target_neither_declared_nor_run_supplied_raises_error_execution_and_sends_nothing() {
    let mock = Mock::start();
    // No base URL value: the default origin, which this package variant does
    // not declare, is refused before the transport sees the request (so the
    // key never leaves the host).
    let undeclared = host(&[("OPENAI_API_KEY", "sk-test-key".to_owned())], true, &[]);
    assert_eq!(run(&undeclared).configuration, ["failed"]);
    // No transport at all: every request is refused.
    let offline = host(&key_and_base(format!("{}/ok/v1", mock.origin)), false, &[]);
    assert_eq!(run(&offline).configuration, ["failed"]);
    assert!(mock.received().is_empty(), "nothing reached the network");
}

#[test]
fn a_missing_api_key_raises_error_execution_and_sends_nothing() {
    let mock = Mock::start();
    let host = host(
        &[("OPENAI_BASE_URL", format!("{}/ok/v1", mock.origin))],
        true,
        &[],
    );
    assert_eq!(run(&host).configuration, ["failed"]);
    assert!(mock.received().is_empty());
}

#[test]
fn values_granted_to_another_package_are_absent_and_send_nothing() {
    let mock = Mock::start();
    let host = granted_host(
        &key_and_base(format!("{}/ok/v1", mock.origin)),
        "https://github.com/stateforward/scxml.rs/namespaces/other@v0.2.0",
        true,
        &[],
    );
    assert_eq!(run(&host).configuration, ["failed"]);
    assert!(mock.received().is_empty(), "nothing reached the network");
}

#[test]
fn an_unreachable_service_raises_error_execution() {
    // Bind and drop a listener: nothing answers on that port.
    let origin = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        format!("http://{}", listener.local_addr().expect("address"))
    };
    let host = host(&key_and_base(format!("{origin}/ok/v1")), true, &[]);
    assert_eq!(run(&host).configuration, ["failed"]);
}
