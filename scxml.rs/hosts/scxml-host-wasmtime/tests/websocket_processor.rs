//! WebSocket connections under the Wasmtime session host: the WebSocket
//! Event I/O Processor (`components/scxml-ioprocessor-websocket`, SCXML 6.2)
//! bound by its package, and the example invoked service
//! (`components/scxml-service-example`, SCXML 6.4), each sending to and
//! receiving from a local echo server through the host's WebSocket client,
//! recorded, and replayed without the network.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies every component
//! into this suite's own artifact directory (or the shared release); the
//! processor component is bound by SHA-256.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "http")]

mod prebuilt;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument, EnvVariable,
    ExtensionEnvironment, InputRecord, LiveClock, LiveHttp, LiveSession, LiveStatus,
    MachineScripts, ProcessorComponents, ProcessorPackage, ScriptedEvent, ServiceComponents,
    ServicePackage, SessionEmbedder, SessionHost, SessionHostComponents, SessionOpen,
    SessionOutput, TelemetryProviders, TrustedComponentDigest, TrustedManifestDigest,
    WasmtimeHostConfig, WebSocketClient, WebSocketClientConfig, authenticate_component,
    authenticate_processor_component,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const COMPONENTS: [(ArtifactKind, &str); 9] = [
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
    (ArtifactKind::ServiceExample, "scxml_service_example"),
];

const PROCESSOR: &str = "scxml_ioprocessor_websocket";

struct Release {
    components: Vec<AuthenticatedComponent>,
    processor: AuthenticatedComponent,
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
}

fn artifacts() -> PathBuf {
    prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace().join("target/session-host-proof");
        let artifacts = target.join("wasm-artifacts/websocket-processor");
        let status = Command::new("sh")
            .arg(workspace().join("scripts/verify-wasm-capabilities.sh"))
            .arg("--artifacts-dir")
            .arg(&artifacts)
            .args(COMPONENTS.iter().map(|(_, name)| *name))
            .arg(PROCESSOR)
            .current_dir(workspace())
            .env("CARGO_TARGET_DIR", &target)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "the release capability proof must pass");
        artifacts
    })
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let artifacts = artifacts();
        let manifest = std::fs::read(artifacts.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        let components = COMPONENTS
            .iter()
            .map(|(kind, name)| {
                let path = format!("{name}.virtualized.wasm");
                let bytes = std::fs::read(artifacts.join(&path)).expect("artifact is readable");
                authenticate_component(&manifest, trusted, *kind, &path, &bytes)
                    .expect("artifact authenticates")
            })
            .collect();
        let bytes = std::fs::read(artifacts.join(format!("{PROCESSOR}.virtualized.wasm")))
            .expect("the processor artifact is readable");
        let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(&bytes)))
            .expect("digest is canonical");
        let processor =
            authenticate_processor_component(&bytes, digest).expect("the processor authenticates");
        Release {
            components,
            processor,
        }
    })
}

fn component(kind: ArtifactKind) -> &'static AuthenticatedComponent {
    let index = COMPONENTS
        .iter()
        .position(|(listed, _)| *listed == kind)
        .expect("the suite builds this kind");
    &release().components[index]
}

fn session_components() -> SessionHostComponents<'static> {
    SessionHostComponents {
        machine: component(ArtifactKind::Machine),
        compiler: component(ArtifactKind::Compiler),
        data_model_ecmascript: component(ArtifactKind::DataModelEcmascript),
        data_model_xpath: component(ArtifactKind::DataModelXpath),
        data_model_null: component(ArtifactKind::DataModelNull),
        io_scxml: component(ArtifactKind::IoProcessorScxml),
        io_basichttp: component(ArtifactKind::IoProcessorBasicHttp),
        clock: component(ArtifactKind::Clock),
    }
}

/// The checked-in package, admitting `origin`.
fn package(origin: &str) -> ProcessorPackage {
    let manifest = scxml_validator_core::ProcessorPackageManifest::from_json(include_bytes!(
        "../../../components/scxml-ioprocessor-websocket/package/websocket.processor.json"
    ))
    .expect("the manifest parses");
    manifest.verify().expect("the manifest verifies");
    ProcessorPackage {
        processor_uri: manifest.processor_uri,
        aliases: manifest.aliases,
        package_version: manifest.package_version,
        processor_artifact: manifest.processor_artifact,
        wit_world: manifest.wit_world,
        wit_version: manifest.wit_version,
        origins: vec![origin.to_owned()],
    }
}

/// A host binding the processor for `origin`, with the WebSocket client
/// when `network` is true.
fn host(origin: &str, network: bool) -> SessionHost {
    let mut processors = ProcessorComponents::new();
    if network {
        processors = processors.with_outbound_websocket(Arc::new(
            WebSocketClient::new(&WebSocketClientConfig::default()).expect("the client builds"),
        ));
    }
    let processors = processors
        .bind(package(origin), &release().processor)
        .expect("the processor binds");
    SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| host.with_processor_components(processors))
    .expect("the verified components link")
}

/// An echo server on loopback: each connection sends back every message it
/// receives until the client closes it. Returns its `ws://` base URL and
/// `http://` origin.
fn echo_server() -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
    let port = listener.local_addr().expect("bound address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            std::thread::spawn(move || {
                let Ok(mut socket) = tungstenite::accept(stream) else {
                    return;
                };
                while let Ok(message) = socket.read() {
                    if (message.is_text() || message.is_binary()) && socket.send(message).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (
        format!("ws://127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}"),
    )
}

/// No documents, no scripts.
struct NoResources;

impl SessionEmbedder for NoResources {
    fn document(&self, _kind: DocumentKind, _uri: &str) -> Option<EmbeddedDocument> {
        None
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        MachineScripts::none()
    }
}

/// Greets the echo server with a named event, answers the echoed event on
/// its origin with plain text, closes the connection when the text comes
/// back, and finishes on the close.
fn document(base: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <onentry>
      <send type="websocket" target="{base}/echo" event="hello"><param name="n" expr="1"/></send>
    </onentry>
    <transition event="hello">
      <log label="hello" expr="_event.origin + ' ' + _event.origintype + ' ' + _event.data.n"/>
      <send type="websocket" targetexpr="_event.origin"><content expr="'plain'"/></send>
    </transition>
    <transition event="websocket.message">
      <log label="message" expr="_event.data"/>
      <send type="websocket" targetexpr="_event.origin" event="websocket.close"/>
    </transition>
    <transition event="websocket.close" target="done">
      <log label="close" expr="_event.data.code"/>
    </transition>
    <transition event="error.*" target="done">
      <log label="error" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"#
    )
}

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "websocket.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn open(
    host: &SessionHost,
    source: &str,
    record: bool,
    replay: Option<&[InputRecord]>,
) -> LiveSession {
    let (ir, data_model) = lower(source);
    host.open(SessionOpen {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record,
        replay,
        http: LiveHttp::default(),
    })
    .expect("the session opens")
}

/// Every log record of the session so far as `label: text`.
fn logs(session: &mut LiveSession) -> Vec<String> {
    session
        .take_outputs()
        .into_iter()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => Some(format!(
                "{}: {}",
                entry.label.unwrap_or_default(),
                entry.text
            )),
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

#[test]
fn a_websocket_processor_talks_to_its_peer_closes_and_replays_without_the_network() {
    let (base, origin) = echo_server();
    let source = document(&base);
    let host = host(&origin, true);
    let mut session = open(&host, &source, true, None);
    let mut seen = logs(&mut session);
    let deadline = Instant::now() + Duration::from_secs(30);
    while session.status() == LiveStatus::Running {
        assert!(Instant::now() < deadline, "the exchange finishes: {seen:?}");
        if session
            .deliver_http_responses()
            .expect("an answer is delivered")
            .is_none()
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        seen.extend(logs(&mut session));
    }
    let expected = [
        format!(
            "hello: {base}/echo https://github.com/stateforward/scxml.rs/processors/websocket 1"
        ),
        "message: plain".to_owned(),
        // The echo server answers the close without a status code (1005).
        "close: 1005".to_owned(),
    ];
    assert_eq!(seen, expected);
    let records = session.take_recorded();
    assert!(
        records
            .iter()
            .any(|record| matches!(record, InputRecord::WebSocket { .. })),
        "each answer is a recorded input: {records:?}"
    );

    // The replay has no WebSocket client: every answer comes from the
    // record.
    let offline = host_offline(&origin);
    let mut replay = open(&offline, &source, false, Some(&records));
    let mut replayed = logs(&mut replay);
    for record in &records {
        replay.apply(record).expect("the record replays");
        replayed.extend(logs(&mut replay));
    }
    assert_eq!(replayed, expected);
    assert_eq!(replay.status(), LiveStatus::Finished);
}

fn host_offline(origin: &str) -> SessionHost {
    host(origin, false)
}

#[test]
fn a_send_type_no_processor_answers_raises_error_execution() {
    let (base, origin) = echo_server();
    let host = host(&origin, true);
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <transition event="go"><send type="https://example.com/no-such-processor" target="{base}/echo" event="x"/></transition>
    <transition event="refused"><send type="websocket" target="https://example.com/" event="x"/></transition>
    <transition event="outside"><send type="websocket" target="ws://192.0.2.1:9/" event="x"/></transition>
    <transition event="error.execution"><log label="execution" expr="_event.name"/></transition>
    <transition event="error.communication"><log label="communication" expr="_event.sendid === undefined ? 'none' : 'id'"/></transition>
  </state>
</scxml>"#
    );
    let mut session = open(&host, &source, false, None);
    session
        .send(&ScriptedEvent::named("go"))
        .expect("go is processed");
    assert_eq!(logs(&mut session), ["execution: error.execution"]);
    // A target that is no ws:// or wss:// URL is the processor's
    // invalid-input.
    session
        .send(&ScriptedEvent::named("refused"))
        .expect("refused is processed");
    assert_eq!(logs(&mut session), ["execution: error.execution"]);
    // An origin the package does not declare is refused by the host when
    // the instance connects: the send is accepted, then fails.
    session
        .send(&ScriptedEvent::named("outside"))
        .expect("outside is processed");
    let mut seen = logs(&mut session);
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.is_empty() {
        assert!(Instant::now() < deadline, "the refusal arrives");
        if session
            .deliver_http_responses()
            .expect("an answer is delivered")
            .is_none()
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        seen.extend(logs(&mut session));
    }
    assert_eq!(seen, ["communication: id"]);
}

const ECHO: &str = "https://stateforward.org/scxml/services/echo";

/// The checked-in example service package, admitting `origin`.
fn service_package(origin: &str) -> ServicePackage {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../../../components/scxml-service-example/package/echo.service.json"
    ))
    .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    ServicePackage {
        service_uri: manifest.service_uri,
        package_version: manifest.package_version,
        service_artifact: manifest.service_artifact,
        wit_world: manifest.wit_world,
        wit_version: manifest.wit_version,
        http_origins: vec![origin.to_owned()],
        env_slots: manifest.env_slots,
    }
}

/// A host binding the example service for `origin`, with `TOKEN` granted to
/// it, and the WebSocket client when `network` is true.
fn service_host(origin: &str, network: bool) -> SessionHost {
    let mut services = ServiceComponents::new().with_values(
        ExtensionEnvironment::new([EnvVariable::new("TOKEN", "s3cret", [ECHO.to_owned()])])
            .expect("valid values"),
    );
    if network {
        services = services.with_outbound_websocket(Arc::new(
            WebSocketClient::new(&WebSocketClientConfig::default()).expect("the client builds"),
        ));
    }
    let services = services
        .bind(
            service_package(origin),
            component(ArtifactKind::ServiceExample),
        )
        .expect("the service binds");
    SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| host.with_service_components(services))
    .expect("the verified components link")
}

/// An echo server on loopback that also keeps each handshake's
/// `authorization` header. Returns its `ws://` base URL, `http://` origin,
/// and the headers seen.
fn authorized_echo_server() -> (String, String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
    let port = listener.local_addr().expect("bound address").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let headers = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let headers = Arc::clone(&headers);
            std::thread::spawn(move || {
                #[allow(
                    clippy::result_large_err,
                    reason = "tungstenite's handshake callback signature"
                )]
                let keep =
                    |request: &tungstenite::handshake::server::Request,
                     response: tungstenite::handshake::server::Response| {
                        if let Some(value) = request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                        {
                            headers
                                .lock()
                                .expect("the header log is not poisoned")
                                .push(value.to_owned());
                        }
                        Ok(response)
                    };
                let Ok(mut socket) = tungstenite::accept_hdr(stream, keep) else {
                    return;
                };
                while let Ok(message) = socket.read() {
                    if (message.is_text() || message.is_binary()) && socket.send(message).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (
        format!("ws://127.0.0.1:{port}"),
        format!("http://127.0.0.1:{port}"),
        seen,
    )
}

/// Invokes the example service with the `token` slot bound; `go` has it
/// connect to the echo server, it says `hi` once open, closes once the echo
/// comes back, and stops once closed.
fn service_document(base: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{env}" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke type="{ECHO}" id="echo" env:token="TOKEN"/>
    <transition event="go"><send event="connect" target="#_echo"><content expr="'{base}/stream'"/></send></transition>
    <transition event="echo.opened">
      <log label="opened" expr="_event.invokeid"/>
      <send event="message" target="#_echo"><content expr="'hi'"/></send>
    </transition>
    <transition event="echo.message">
      <log label="message" expr="_event.data"/>
      <send event="close" target="#_echo"/>
    </transition>
    <transition event="echo.closed">
      <log label="closed" expr="typeof _event.data === 'number' ? _event.data : 'none'"/>
      <send event="stop" target="#_echo"/>
    </transition>
    <transition event="done.invoke.echo" target="done"/>
    <transition event="error.*" target="done">
      <log label="error" expr="_event.name"/>
    </transition>
  </state>
  <final id="done"/>
</scxml>"##,
        env = scxml_types::ENV_NAMESPACE_URI
    )
}

#[test]
fn an_invoked_service_talks_over_a_websocket_with_a_referenced_token_and_replays() {
    let (base, origin, authorizations) = authorized_echo_server();
    let source = service_document(&base);
    let host = service_host(&origin, true);
    let mut session = open(&host, &source, true, None);
    session
        .send(&ScriptedEvent::named("go"))
        .expect("go is processed");
    let mut seen = logs(&mut session);
    let deadline = Instant::now() + Duration::from_secs(30);
    while session.status() == LiveStatus::Running {
        assert!(Instant::now() < deadline, "the exchange finishes: {seen:?}");
        if session
            .deliver_http_responses()
            .expect("an answer is delivered")
            .is_none()
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        seen.extend(logs(&mut session));
    }
    // The echo server answers the service's close without a status code
    // (1005).
    let expected = ["opened: echo", "message: hi", "closed: 1005"];
    assert_eq!(seen, expected);
    assert_eq!(
        *authorizations
            .lock()
            .expect("the header log is not poisoned"),
        ["Bearer s3cret"],
        "the referenced token reaches the peer, never the component"
    );
    let records = session.take_recorded();
    assert!(
        records.iter().any(|record| matches!(
            record,
            InputRecord::WebSocket {
                processor: None,
                ..
            }
        )),
        "each answer is a recorded input of the invocation: {records:?}"
    );

    // The replay has no WebSocket client: every answer comes from the
    // record.
    let offline = service_host(&origin, false);
    let mut replay = open(&offline, &source, false, Some(&records));
    let mut replayed = logs(&mut replay);
    for record in &records {
        replay.apply(record).expect("the record replays");
        replayed.extend(logs(&mut replay));
    }
    assert_eq!(replayed, expected);
    assert_eq!(replay.status(), LiveStatus::Finished);
}

#[test]
fn a_service_connection_outside_its_package_or_without_a_transport_fails_the_invocation() {
    let (base, origin, _) = authorized_echo_server();
    let source = service_document(&base);
    // An origin the package does not declare, then no transport at all.
    for host in [
        service_host("http://127.0.0.1:9", true),
        service_host(&origin, false),
    ] {
        let mut session = open(&host, &source, false, None);
        session
            .send(&ScriptedEvent::named("go"))
            .expect("go is processed");
        let mut seen = logs(&mut session);
        let deadline = Instant::now() + Duration::from_secs(10);
        while session.status() == LiveStatus::Running {
            assert!(Instant::now() < deadline, "the refusal arrives: {seen:?}");
            if session
                .deliver_http_responses()
                .expect("an answer is delivered")
                .is_none()
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            seen.extend(logs(&mut session));
        }
        assert_eq!(seen, ["error: error.communication.echo"]);
    }
}
