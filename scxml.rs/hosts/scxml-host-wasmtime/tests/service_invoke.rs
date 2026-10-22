//! Invoked services (SCXML 6.4) under the Wasmtime session host: the echo
//! and timer example (`components/scxml-service-example`, a WASI 0.3
//! `async fn invoke`) invoked by live sessions of the shipped machine, data
//! model, and I/O processors.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies every component
//! into this suite's own artifact directory; the example service is bound
//! through the release manifest (and, for the caller path, by SHA-256). The
//! files example (`components/scxml-service-files`), whose component
//! imports `wasi:filesystem`, proves the WASI resource path: it is bound by
//! SHA-256 with the directory the run preopens for it.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use scxml_host_wasmtime::scxml_snapshot::Options;
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument,
    EnvVariable, ExtensionEnvironment, ExtensionHttp, ExtensionHttpFailure, ExtensionHttpPart,
    ExtensionHttpRequest, ExtensionHttpResponse, HostError, InputRecord, LiveClock, LiveHttp,
    LiveSession, LiveStatus, MachineScripts, RunFailure, ScriptedEvent, ServiceComponents,
    ServicePackage, SessionEmbedder, SessionHost, SessionHostComponents, SessionOpen,
    SessionOutput, SnapshotFailure, TelemetryProviders, TrustedComponentDigest,
    TrustedManifestDigest, WasiOptions, WasmtimeHostConfig, authenticate_component,
    authenticate_service_component,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const ECHO: &str = "https://stateforward.org/scxml/services/echo";
const FILES: &str = "https://stateforward.org/scxml/services/files";

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

struct Release {
    components: Vec<AuthenticatedComponent>,
    example_bytes: Vec<u8>,
    files_bytes: Vec<u8>,
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent")
}

fn release() -> &'static Release {
    static RELEASE: OnceLock<Release> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let artifacts = prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace().join("target/session-host-proof");
            let artifacts = target.join("wasm-artifacts/service-invoke");
            let status = Command::new("sh")
                .arg(workspace().join("scripts/verify-wasm-capabilities.sh"))
                .arg("--artifacts-dir")
                .arg(&artifacts)
                .args(COMPONENTS.iter().map(|(_, name)| *name))
                .arg("scxml_service_files")
                .current_dir(workspace())
                .env("CARGO_TARGET_DIR", &target)
                .status()
                .expect("the capability script runs");
            assert!(status.success(), "the release capability proof must pass");
            artifacts
        });
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
        let example_bytes = std::fs::read(artifacts.join("scxml_service_example.virtualized.wasm"))
            .expect("example artifact is readable");
        let files_bytes = std::fs::read(artifacts.join("scxml_service_files.virtualized.wasm"))
            .expect("files artifact is readable");
        Release {
            components,
            example_bytes,
            files_bytes,
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

/// A checked-in service manifest as the host's package, bound to
/// `service_uri`.
fn package_from(bytes: &[u8], service_uri: &str) -> ServicePackage {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(bytes)
        .expect("the manifest parses");
    manifest.verify(None).expect("the manifest verifies");
    ServicePackage {
        service_uri: service_uri.into(),
        package_version: manifest.package_version,
        service_artifact: manifest.service_artifact,
        wit_world: manifest.wit_world,
        wit_version: manifest.wit_version,
        http_origins: manifest.http_origins,
        env_slots: manifest.env_slots,
    }
}

/// The checked-in example manifest as the host's package.
fn package(service_uri: &str) -> ServicePackage {
    package_from(
        include_bytes!("../../../components/scxml-service-example/package/echo.service.json"),
        service_uri,
    )
}

fn host_with(services: ServiceComponents) -> SessionHost {
    try_host(services).expect("the verified components link")
}

fn try_host(services: ServiceComponents) -> Result<SessionHost, HostError> {
    SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| host.with_service_components(services))
}

fn host() -> SessionHost {
    host_with(
        ServiceComponents::new()
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
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

/// The parent document: `s` invokes the echo service with a 250 ms timer
/// and arms a 100 ms and a 300 ms delayed send around it; its `<finalize>`
/// counts the messages it saw.
fn document() -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <datamodel><data id="finalized" expr="0"/></datamodel>
  <state id="s">
    <onentry>
      <send event="before" delay="100ms"/>
      <send event="after" delay="300ms"/>
    </onentry>
    <invoke type="{ECHO}" id="echo">
      <param name="delay" expr="250"/>
      <finalize><assign location="finalized" expr="finalized + 1"/></finalize>
    </invoke>
    <transition event="before"><log label="before" expr="finalized"/></transition>
    <transition event="after"><log label="after" expr="finalized"/></transition>
    <transition event="echo.tick">
      <log label="tick" expr="_event.invokeid + ' ' + _event.data + ' ' + finalized"/>
    </transition>
    <transition event="ping"><send event="ping" target="#_echo"/></transition>
    <transition event="long"><send eventexpr="'x'.repeat(1020)" target="#_echo"/></transition>
    <transition event="echo.ping">
      <log label="echo.ping" expr="_event.invokeid + ' ' + _event.origintype + ' ' + finalized"/>
    </transition>
    <transition event="stop"><send event="stop" target="#_echo"/></transition>
    <transition event="done.invoke.echo" target="done">
      <log label="done" expr="JSON.stringify(_event.data) + ' ' + finalized"/>
    </transition>
    <transition event="error.communication" target="failed">
      <log label="failed" expr="_event.name + ' ' + _event.invokeid + ' ' + _event.type"/>
    </transition>
    <transition event="leave" target="away"/>
  </state>
  <state id="done">
    <transition event="ping"><send event="ping" target="#_echo"/></transition>
    <transition event="error.communication">
      <log label="unreachable" expr="_event.name"/>
    </transition>
  </state>
  <state id="failed"><transition event="ping"><send event="ping" target="#_echo"/></transition></state>
  <state id="away"/>
</scxml>"##
    )
}

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "service.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn open_with(
    host: &SessionHost,
    source: &str,
    record: bool,
    replay: Option<&[InputRecord]>,
    http: LiveHttp,
) -> Result<LiveSession, scxml_host_wasmtime::SessionHostError> {
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
        http,
    })
}

fn open(host: &SessionHost) -> LiveSession {
    open_with(host, &document(), false, None, LiveHttp::default()).expect("the session opens")
}

/// Every log record of the call as `label: text`.
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
fn an_echo_service_ticks_on_the_logical_clock_echoes_and_completes_with_data() {
    let host = host();
    let mut session = open(&host);
    // The start ran `invoke`: the service waits on its 250 ms timer, which
    // the session schedules on its logical clock with its delayed sends.
    assert_eq!(session.configuration().expect("configuration"), ["s"]);
    assert_eq!(session.next_deadline().expect("deadline"), Some(100));
    assert!(logs(&mut session).is_empty());

    // The timer falls due in order with the delayed sends; `echo.tick` is
    // stamped with the invoke id and runs the invocation's `<finalize>`.
    session.advance_to(300).expect("the deadlines fire");
    assert_eq!(
        logs(&mut session),
        ["before: 0", "tick: echo 1 1", "after: 1"]
    );
    assert_eq!(session.next_deadline().expect("deadline"), None);

    // `<send target="#_echo">` reaches the service's `events` stream, and
    // the event it returns carries the invoke id and its type as origin.
    session
        .send(&ScriptedEvent::named("ping"))
        .expect("ping is processed");
    assert_eq!(logs(&mut session), [format!("echo.ping: echo {ECHO} 2")]);

    // `stop` resolves the service's future: `done.invoke.echo` carries it.
    session
        .send(&ScriptedEvent::named("stop"))
        .expect("stop is processed");
    assert_eq!(logs(&mut session), [r#"done: {"echoed":1,"ticks":1} 3"#]);
    assert_eq!(session.configuration().expect("configuration"), ["done"]);

    // After it finished, `#_echo` answers nobody: error.communication.
    session
        .send(&ScriptedEvent::named("ping"))
        .expect("the late ping is processed");
    assert_eq!(logs(&mut session), ["unreachable: error.communication"]);
    session.close().expect("the session closes");
}

#[test]
fn leaving_the_invoking_state_cancels_the_service() {
    let host = host();
    let mut session = open(&host);
    // A session with a live service cannot be snapshotted.
    match session.snapshot(&Options::default()) {
        Err(error) => assert!(
            matches!(
                error.failure,
                RunFailure::Snapshot(SnapshotFailure::ServiceInvocation)
            ),
            "{error}"
        ),
        Ok(_) => panic!("a live service invocation refuses the snapshot"),
    }
    session
        .send(&ScriptedEvent::named("leave"))
        .expect("leave is processed");
    assert_eq!(session.configuration().expect("configuration"), ["away"]);
    // SCXML 6.4: the cancelled invocation's timer is gone, no done.invoke
    // follows, and the session snapshots again.
    session
        .snapshot(&Options::default())
        .expect("no invoked service remains");
    let step = session.run_until_idle().expect("idle");
    assert_eq!(step.status, LiveStatus::Running);
    assert!(logs(&mut session).is_empty(), "no tick after the cancel");
    session.close().expect("the session closes");
}

#[test]
fn a_service_whose_future_resolves_err_raises_error_communication() {
    let host = host();
    let mut session = open(&host);
    // The service cannot echo a name `echo.` would push past the event-name
    // bound: its future resolves `err`.
    session
        .send(&ScriptedEvent::named("long"))
        .expect("the long event is processed");
    assert_eq!(
        logs(&mut session),
        ["failed: error.communication.echo echo platform"]
    );
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    // No done.invoke follows, its timer is gone, and `#_echo` is now
    // unreachable.
    session.run_until_idle().expect("idle");
    assert!(
        logs(&mut session)
            .iter()
            .all(|line| !line.starts_with("tick"))
    );
    session.close().expect("the session closes");
}

/// A document whose `go` enters a state that invokes `invoke`.
fn runtime_document(invoke: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="idle">
  <state id="idle"><transition event="go" target="s"/></state>
  <state id="s">
    {invoke}
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="failed"/>
</scxml>"#
    )
}

#[test]
fn an_unbound_or_platform_specific_invoke_type_raises_error_execution_when_it_runs() {
    // SCXML 6.4: invocation is a runtime action. The session starts although
    // no service is bound to the type; only entering the invoking state
    // fails that `<invoke>` with error.execution. A platform-specific type
    // that is not a URI (SCXML 6.4.1) and an unbound `typeexpr` fail the
    // same way.
    for (services, invoke) in [
        (
            ServiceComponents::new(),
            format!(r#"<invoke type="{ECHO}"/>"#),
        ),
        (
            ServiceComponents::new()
                .bind(package(ECHO), component(ArtifactKind::ServiceExample))
                .expect("binds"),
            r#"<invoke type="echo"/>"#.to_owned(),
        ),
        (
            ServiceComponents::new()
                .bind(package(ECHO), component(ArtifactKind::ServiceExample))
                .expect("binds"),
            r#"<invoke typeexpr="'urn:example:missing'"/>"#.to_owned(),
        ),
    ] {
        let host = host_with(services);
        let mut session = open_with(
            &host,
            &runtime_document(&invoke),
            false,
            None,
            LiveHttp::default(),
        )
        .expect("an unbound type does not fail the session start");
        assert_eq!(session.configuration().expect("configuration"), ["idle"]);
        session
            .send(&ScriptedEvent::named("go"))
            .expect("go is processed");
        assert_eq!(session.status(), LiveStatus::Finished, "{invoke}");
        session.close().expect("the session closes");
    }
}

#[test]
fn a_caller_service_component_binds_by_digest_and_kind_is_checked() {
    let bytes = &release().example_bytes;
    let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("digest is canonical");
    let caller = authenticate_service_component(bytes, digest).expect("the bytes authenticate");
    assert_eq!(caller.kind(), ArtifactKind::ServiceComponent);
    let host = host_with(
        ServiceComponents::new()
            .bind(package(ECHO), &caller)
            .expect("a caller service binds"),
    );
    let mut session = open(&host);
    assert_eq!(session.next_deadline().expect("deadline"), Some(100));
    session.close().expect("the session closes");
    // A component bound to a URI it does not implement rejects its start:
    // the `<invoke>` raises error.execution.
    let other = "https://stateforward.org/scxml/services/other";
    let host = host_with(
        ServiceComponents::new()
            .bind(package(other), &caller)
            .expect("binding checks no component identity"),
    );
    let mut session = open_with(
        &host,
        &runtime_document(&format!(r#"<invoke type="{other}"/>"#)),
        false,
        None,
        LiveHttp::default(),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("go"))
        .expect("go is processed");
    assert_eq!(session.status(), LiveStatus::Finished);
    session.close().expect("the session closes");
    // Only service artifact kinds bind.
    assert!(
        ServiceComponents::new()
            .bind(package(ECHO), component(ArtifactKind::Clock))
            .is_err()
    );
    assert!(
        ServiceComponents::new()
            .bind(package("scxml"), &caller)
            .is_err(),
        "the SCXML type is no service type"
    );
}

/// The files component, authenticated by its SHA-256 like any caller
/// service component.
fn files_component() -> AuthenticatedComponent {
    let bytes = &release().files_bytes;
    let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("digest is canonical");
    authenticate_service_component(bytes, digest).expect("the files component authenticates")
}

fn files_package() -> ServicePackage {
    package_from(
        include_bytes!("../../../components/scxml-service-files/package/files.service.json"),
        FILES,
    )
}

/// A fresh directory of this suite's own for one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let directory = workspace()
        .join("target/session-host-proof/wasi-files")
        .join(name);
    if directory.exists() {
        std::fs::remove_dir_all(&directory).expect("the old scratch directory is removed");
    }
    std::fs::create_dir_all(&directory).expect("the scratch directory is created");
    directory
}

/// `--dir FILES=DIRECTORY::/workspace`.
fn workspace_dir(directory: &Path) -> WasiOptions {
    WasiOptions {
        dirs: vec![(directory.to_path_buf(), "/workspace".into())],
        ..WasiOptions::default()
    }
}

/// Reads `seed.txt` and writes `out.txt` in the service's workspace.
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
    <transition event="error.execution" target="failed">
      <log label="failed" expr="_event.name"/>
    </transition>
  </state>
  <state id="done"/>
  <state id="failed"/>
</scxml>"#;

fn files_host(wasi: Option<WasiOptions>) -> Result<SessionHost, HostError> {
    let services = ServiceComponents::new()
        .bind(files_package(), &files_component())
        .expect("the files package binds");
    try_host(match wasi {
        Some(options) => services.with_wasi(FILES, options),
        None => services,
    })
}

#[test]
fn a_preopened_directory_is_what_the_filesystem_import_reads_and_writes() {
    let directory = scratch("granted");
    std::fs::write(directory.join("seed.txt"), "from the host").expect("seed is written");
    let host = files_host(Some(workspace_dir(&directory))).expect("the grant links");
    let mut session = open_with(&host, FILES_DOCUMENT, false, None, LiveHttp::default())
        .expect("the session opens");
    assert_eq!(
        logs(&mut session),
        [r#"done: {"seed":"from the host","wrote":22}"#],
        "the service read the host's seed through its preopened directory"
    );
    assert_eq!(session.configuration().expect("configuration"), ["done"]);
    session.close().expect("the session closes");
    assert_eq!(
        std::fs::read_to_string(directory.join("out.txt")).expect("the service wrote out.txt"),
        "written by the service"
    );
}

#[test]
fn an_ungranted_or_unused_wasi_resource_fails_the_binding_closed() {
    let refused = |result: Result<SessionHost, HostError>| match result {
        Err(HostError::Artifact(error)) => error,
        Err(other) => panic!("unexpected failure: {other}"),
        Ok(_) => panic!("the binding must fail closed"),
    };
    // The component imports wasi:filesystem: without a directory it fails
    // at bind time, before any session starts.
    assert_eq!(
        refused(files_host(None)),
        ArtifactError::WasiResourceUngranted
    );
    // A grant whose interface the component does not import, or to a URI
    // no package binds, fails the same way.
    let directory = scratch("unused");
    assert_eq!(
        refused(files_host(Some(WasiOptions {
            allow_ip_name_lookup: true,
            ..workspace_dir(&directory)
        }))),
        ArtifactError::WasiGrantUndeclared
    );
    assert_eq!(
        refused(try_host(
            ServiceComponents::new()
                .bind(package(ECHO), component(ArtifactKind::ServiceExample))
                .expect("binds")
                .with_wasi(ECHO, workspace_dir(&directory))
        )),
        ArtifactError::WasiGrantUndeclared,
        "the echo service imports no filesystem"
    );
    assert_eq!(
        refused(try_host(
            ServiceComponents::new()
                .bind(package(ECHO), component(ArtifactKind::ServiceExample))
                .expect("binds")
                .with_wasi(FILES, workspace_dir(&directory))
        )),
        ArtifactError::WasiGrantUndeclared,
        "no package binds the granted URI"
    );
    // A preopened directory that does not exist fails closed when the
    // invocation starts (`--dir` is opened for each invocation's context).
    let host = files_host(Some(workspace_dir(&directory.join("missing"))))
        .expect("the plan does not open the directory");
    assert!(
        open_with(&host, FILES_DOCUMENT, false, None, LiveHttp::default()).is_err(),
        "a directory that cannot be opened fails closed"
    );
}

/// A transport that answers 204, records each admitted request's URL and
/// `authorization` header, and,
/// when `hold` is set, holds every request until the test releases it: what
/// a slow model call looks like to the session.
#[derive(Debug, Default)]
struct TestHttp {
    hold: bool,
    urls: Mutex<Vec<String>>,
    authorizations: Mutex<Vec<String>>,
    released: Mutex<usize>,
    wake: Condvar,
}

impl TestHttp {
    fn held() -> Arc<Self> {
        Arc::new(Self {
            hold: true,
            ..Self::default()
        })
    }

    fn urls(&self) -> Vec<String> {
        self.urls
            .lock()
            .expect("the request log is not poisoned")
            .clone()
    }

    /// Releases one held request.
    fn release_one(&self) {
        *self
            .released
            .lock()
            .expect("the release count is not poisoned") += 1;
        self.wake.notify_all();
    }
}

impl ExtensionHttp for TestHttp {
    fn fetch(
        &self,
        request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
        let index = {
            let mut urls = self.urls.lock().expect("the request log is not poisoned");
            urls.push(request.url.clone());
            urls.len()
        };
        self.authorizations
            .lock()
            .expect("the header log is not poisoned")
            .extend(
                request
                    .headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    .map(|(_, value)| value.clone()),
            );
        if self.hold {
            let mut released = self
                .released
                .lock()
                .expect("the release count is not poisoned");
            while *released < index {
                released = self
                    .wake
                    .wait(released)
                    .expect("the release count is not poisoned");
            }
        }
        Ok(ExtensionHttpResponse {
            status: 204,
            body: Vec::new(),
        })
    }
}

/// `s` invokes the echo service binding its `endpoint` slot as `invoke`
/// says (with a 1 s timer of its own); `fetch` makes the service GET the
/// slot's URL, `send` GET it streamed (each body read is logged as
/// `received`), `ping` is echoed, and `leave` exits `s`.
fn endpoint_document(invoke: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{env}" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <onentry><send event="timer" delay="1s"/></onentry>
    {invoke}
    <transition event="fetch"><send event="fetch" target="#_echo"/></transition>
    <transition event="send"><send event="send" target="#_echo"/></transition>
    <transition event="ping"><send event="ping" target="#_echo"/></transition>
    <transition event="echo.ping"><log label="ping" expr="_event.invokeid"/></transition>
    <transition event="echo.received"><log label="received" expr="_event.data"/></transition>
    <transition event="echo.fetched"><log label="fetched" expr="_event.data"/></transition>
    <transition event="timer"><log label="timer" expr="'fired'"/></transition>
    <transition event="error.execution" target="refused"/>
    <transition event="error.communication" target="failed"/>
    <transition event="leave" target="away"/>
  </state>
  <state id="away"><transition event="timer"><log label="timer" expr="'fired'"/></transition></state>
  <state id="refused"/>
  <state id="failed"/>
</scxml>"##,
        env = scxml_types::ENV_NAMESPACE_URI
    )
}

const BOUND: &str = r#"<invoke type="https://stateforward.org/scxml/services/echo" id="echo" env:endpoint="ENDPOINT"/>"#;

/// `values` are `(name, value, granted package URI)`.
fn endpoint_host(http: &Arc<TestHttp>, values: &[(&str, &str, &str)]) -> SessionHost {
    host_with(
        ServiceComponents::new()
            .with_values(
                ExtensionEnvironment::new(values.iter().map(|(name, value, uri)| {
                    EnvVariable::new(*name, *value, [(*uri).to_owned()])
                }))
                .expect("valid values"),
            )
            .with_outbound_http(Arc::clone(http) as _)
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    )
}

/// A live HTTP network that counts the responses that arrive.
fn counted(notified: &Arc<AtomicUsize>) -> LiveHttp {
    let counter = Arc::clone(notified);
    LiveHttp {
        on_http_response: Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::AcqRel);
        })),
        ..LiveHttp::default()
    }
}

/// Waits until `notified` reaches `count` (a response arrived).
fn arrived(notified: &AtomicUsize, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while notified.load(Ordering::Acquire) < count {
        assert!(Instant::now() < deadline, "the response arrives");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn an_env_slot_reference_lives_for_the_whole_invocation() {
    let http = Arc::new(TestHttp::default());
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", ECHO)]);
    for invoke in [
        BOUND.to_owned(),
        format!(r#"<invoke type="{ECHO}" id="echo" env:endpointexpr="'END' + 'POINT'"/>"#),
    ] {
        let notified = Arc::new(AtomicUsize::new(0));
        let mut session = open_with(
            &host,
            &endpoint_document(&invoke),
            false,
            None,
            counted(&notified),
        )
        .expect("the session opens");
        // The reference bound at the start serves every request of the
        // invocation: its value reaches only the transport.
        for count in 1..=2 {
            session
                .send(&ScriptedEvent::named("fetch"))
                .expect("fetch is processed");
            arrived(&notified, count);
            session
                .deliver_http_responses()
                .expect("the response is delivered")
                .expect("one response arrived");
            assert_eq!(logs(&mut session), ["fetched: 204"], "{invoke}");
        }
        session.close().expect("the session closes");
    }
    assert_eq!(http.urls(), ["http://127.0.0.1:9/status"; 4]);
}

#[test]
fn an_unbound_out_of_scope_or_undeclared_env_slot_never_reaches_the_network() {
    let http = Arc::new(TestHttp::default());
    // No value, or a value granted only to another package: the slot is
    // absent, so the service's fetch fails and its future resolves `err`
    // (error.communication).
    for values in [
        &[][..],
        &[("ENDPOINT", "http://127.0.0.1:9/status", FILES)][..],
    ] {
        let host = endpoint_host(&http, values);
        let mut session = open_with(
            &host,
            &endpoint_document(BOUND),
            false,
            None,
            LiveHttp::default(),
        )
        .expect("the session opens");
        session
            .send(&ScriptedEvent::named("fetch"))
            .expect("fetch is processed");
        assert_eq!(session.configuration().expect("configuration"), ["failed"]);
        session.close().expect("the session closes");
    }
    // A slot the package does not declare rejects the start: the `<invoke>`
    // raises error.execution and the service never runs.
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", ECHO)]);
    let mut session = open_with(
        &host,
        &endpoint_document(&format!(
            r#"<invoke type="{ECHO}" id="echo" env:other="ENDPOINT"/>"#
        )),
        false,
        None,
        LiveHttp::default(),
    )
    .expect("the session opens");
    assert_eq!(session.configuration().expect("configuration"), ["refused"]);
    session.close().expect("the session closes");
    assert!(http.urls().is_empty());
}

/// `s` invokes the echo service as `invoke` says; `fetch` makes the service
/// GET its `endpoint` slot's URL followed by `path`.
fn path_document(invoke: &str, path: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{env}" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    {invoke}
    <transition event="fetch"><send event="fetch" target="#_echo"><content expr="'{path}'"/></send></transition>
    <transition event="echo.fetched"><log label="fetched" expr="_event.data"/></transition>
    <transition event="error.communication" target="failed"/>
  </state>
  <state id="failed"/>
</scxml>"##,
        env = scxml_types::ENV_NAMESPACE_URI
    )
}

/// The `token` slot's reference becomes a header value and the request
/// path follows the `endpoint` slot's reference, substituted by the host
/// only into a request whose whole origin a value supplied (the package
/// declares none): a value that is no origin, or a path that extends the
/// authority a value starts, is refused before the transport sees it, and
/// the invocation fails (`error.communication`).
#[test]
fn a_token_and_a_path_reach_only_a_request_whose_origin_a_value_supplies() {
    let invoke =
        format!(r#"<invoke type="{ECHO}" id="echo" env:endpoint="ENDPOINT" env:token="TOKEN"/>"#);
    for (endpoint, path, admitted) in [
        (
            "http://127.0.0.1:9/v1",
            "/status?q=1",
            Some("http://127.0.0.1:9/v1/status?q=1"),
        ),
        ("http://127.0.0.1", ":9/status", None),
        ("127.0.0.1:9", "/status", None),
    ] {
        let http = Arc::new(TestHttp::default());
        let host = endpoint_host(
            &http,
            &[("ENDPOINT", endpoint, ECHO), ("TOKEN", "key-test", ECHO)],
        );
        let notified = Arc::new(AtomicUsize::new(0));
        let mut session = open_with(
            &host,
            &path_document(&invoke, path),
            false,
            None,
            counted(&notified),
        )
        .expect("the session opens");
        session
            .send(&ScriptedEvent::named("fetch"))
            .expect("fetch is processed");
        if let Some(url) = admitted {
            arrived(&notified, 1);
            session
                .deliver_http_responses()
                .expect("the response is delivered")
                .expect("one response arrived");
            assert_eq!(logs(&mut session), ["fetched: 204"]);
            assert_eq!(http.urls(), [url]);
            assert_eq!(
                *http.authorizations.lock().expect("the header log"),
                ["Bearer key-test"]
            );
        } else {
            assert_eq!(
                session.configuration().expect("configuration"),
                ["failed"],
                "{endpoint} {path}"
            );
            assert!(http.urls().is_empty(), "nothing is sent");
        }
        session.close().expect("the session closes");
    }
}

#[test]
fn a_fetch_suspends_only_its_task_and_its_response_arrives_as_an_external_input() {
    let http = TestHttp::held();
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/slow", ECHO)]);
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    assert!(session.awaiting_http_responses());
    // Neither the session nor the service waits for the request: the
    // service keeps reading its events while its fetch is pending.
    session
        .send(&ScriptedEvent::named("ping"))
        .expect("ping is processed while the request is in flight");
    assert_eq!(logs(&mut session), ["ping: echo"]);
    // A logical session does not move its clock past the pending request.
    session.run_until_idle().expect("idle");
    assert!(logs(&mut session).is_empty(), "the 1 s timer waits");
    assert_eq!(
        session.deliver_http_responses().expect("nothing arrived"),
        None
    );
    // The response arrives off the session thread; the embedder is woken
    // and delivers it, and the service returns the status.
    http.release_one();
    arrived(&notified, 1);
    assert!(session.awaiting_http_responses(), "arrived, not delivered");
    session
        .deliver_http_responses()
        .expect("the response is delivered")
        .expect("one response arrived");
    assert_eq!(logs(&mut session), ["fetched: 204"]);
    assert!(!session.awaiting_http_responses());
    session.run_until_idle().expect("idle");
    assert_eq!(logs(&mut session), ["timer: fired"]);
    session.close().expect("the session closes");
}

#[test]
fn leaving_the_invoking_state_drops_a_request_in_flight() {
    let http = TestHttp::held();
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/slow", ECHO)]);
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    assert!(session.awaiting_http_responses());
    // The invoking state exits: the invocation is cancelled while its
    // request is in flight, and the session no longer waits for it.
    session
        .send(&ScriptedEvent::named("leave"))
        .expect("leave is processed");
    assert_eq!(session.configuration().expect("configuration"), ["away"]);
    assert!(!session.awaiting_http_responses());
    session.run_until_idle().expect("idle");
    assert_eq!(logs(&mut session), ["timer: fired"]);
    // The late response reaches no one.
    http.release_one();
    arrived(&notified, 1);
    session
        .deliver_http_responses()
        .expect("the late response is dropped");
    assert!(logs(&mut session).is_empty());
    assert_eq!(session.configuration().expect("configuration"), ["away"]);
    session.close().expect("the session closes");
}

#[test]
fn a_recorded_http_response_replays_without_the_network() {
    let http = Arc::new(TestHttp::default());
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", ECHO)]);
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        true,
        None,
        counted(&notified),
    )
    .expect("the recording session opens");
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    arrived(&notified, 1);
    session
        .deliver_http_responses()
        .expect("the response is delivered")
        .expect("one response arrived");
    session.run_until_idle().expect("idle");
    let recorded_logs = logs(&mut session);
    assert_eq!(recorded_logs, ["fetched: 204", "timer: fired"]);
    let records = session.take_recorded();
    session.close().expect("the session closes");
    assert!(
        records.iter().any(|record| matches!(
            record,
            InputRecord::HttpResponse { path, request: 0, response: Ok(response), .. }
                if path == &["echo".to_owned()] && response.status == 204
        )),
        "the response is a recorded input: {records:?}"
    );
    // The replay delivers the recorded response; the network is not used.
    let sent = http.urls().len();
    let mut replay = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        Some(&records),
        LiveHttp::default(),
    )
    .expect("the replay opens");
    let mut replayed = Vec::new();
    for record in &records {
        replay.apply(record).expect("the record applies");
        replayed.extend(logs(&mut replay));
    }
    assert_eq!(replayed, recorded_logs);
    assert_eq!(http.urls().len(), sent, "a replay sends no request");
    replay.close().expect("the replay closes");
}

/// A transport that answers every `send` with `parts`, each held until the
/// test releases it: a response that streams in as the test says.
#[derive(Debug)]
struct Stepped {
    parts: Vec<Result<ExtensionHttpPart, ExtensionHttpFailure>>,
    released: Mutex<usize>,
    wake: Condvar,
    sent: AtomicUsize,
}

impl Stepped {
    fn new(parts: Vec<Result<ExtensionHttpPart, ExtensionHttpFailure>>) -> Arc<Self> {
        Arc::new(Self {
            parts,
            released: Mutex::new(0),
            wake: Condvar::new(),
            sent: AtomicUsize::new(0),
        })
    }

    /// Releases the next part.
    fn release_one(&self) {
        *self
            .released
            .lock()
            .expect("the release count is not poisoned") += 1;
        self.wake.notify_all();
    }
}

impl ExtensionHttp for Stepped {
    fn fetch(
        &self,
        _request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
        Err(ExtensionHttpFailure::Transport)
    }

    fn send(
        &self,
        _request: &ExtensionHttpRequest,
        part: &mut dyn FnMut(Result<ExtensionHttpPart, ExtensionHttpFailure>),
    ) {
        self.sent.fetch_add(1, Ordering::AcqRel);
        for (index, next) in self.parts.iter().enumerate() {
            let mut released = self
                .released
                .lock()
                .expect("the release count is not poisoned");
            while *released <= index {
                released = self
                    .wake
                    .wait(released)
                    .expect("the release count is not poisoned");
            }
            drop(released);
            part(next.clone());
        }
    }
}

fn stepped_host(http: &Arc<Stepped>) -> SessionHost {
    host_with(
        ServiceComponents::new()
            .with_values(
                ExtensionEnvironment::new([EnvVariable::new(
                    "ENDPOINT",
                    "http://127.0.0.1:9/stream",
                    [ECHO.to_owned()],
                )])
                .expect("valid values"),
            )
            .with_outbound_http(Arc::clone(http) as _)
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    )
}

/// `outbound-http.send`: the status, each body part, and the end reach the
/// service as inputs of its invocation as they arrive, so it reports
/// progress while the response is still streaming; a recording session
/// records each part, and a replay delivers them without the network.
#[test]
fn a_streamed_response_reaches_the_service_part_by_part_and_replays() {
    let http = Stepped::new(vec![
        Ok(ExtensionHttpPart::Status(200)),
        Ok(ExtensionHttpPart::Body(b"ab".to_vec())),
        Ok(ExtensionHttpPart::Body(b"cde".to_vec())),
        Ok(ExtensionHttpPart::End),
    ]);
    let host = stepped_host(&http);
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        true,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("send"))
        .expect("send is processed");
    let mut recorded_logs = Vec::new();
    for (step, expected) in [
        (1, Vec::<&str>::new()),
        (2, vec!["received: 2"]),
        (3, vec!["received: 3"]),
        (4, vec!["fetched: 200"]),
    ] {
        assert!(session.awaiting_http_responses(), "step {step}");
        http.release_one();
        arrived(&notified, step);
        session
            .deliver_http_responses()
            .expect("the part is delivered")
            .expect("one part arrived");
        let logged = logs(&mut session);
        assert_eq!(logged, expected, "step {step}");
        recorded_logs.extend(logged);
    }
    assert!(!session.awaiting_http_responses(), "the body ended");
    let records = session.take_recorded();
    session.close().expect("the session closes");
    let parts: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            InputRecord::HttpResponsePart {
                path,
                request: 0,
                part,
                ..
            } if path == &["echo".to_owned()] => Some(part.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(parts, http.parts, "each part is a recorded input");
    let mut replay = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        Some(&records),
        LiveHttp::default(),
    )
    .expect("the replay opens");
    let mut replayed = Vec::new();
    for record in &records {
        replay.apply(record).expect("the record applies");
        replayed.extend(logs(&mut replay));
    }
    assert_eq!(replayed, recorded_logs);
    assert_eq!(
        http.sent.load(Ordering::Acquire),
        1,
        "a replay sends no request"
    );
    replay.close().expect("the replay closes");
}

/// A streamed response that fails after its status fails the invocation
/// (its body's future resolves `err`, which the service returns), after the
/// parts that arrived.
#[test]
fn a_streamed_response_that_fails_midway_fails_the_invocation() {
    let http = Stepped::new(vec![
        Ok(ExtensionHttpPart::Status(200)),
        Ok(ExtensionHttpPart::Body(b"ab".to_vec())),
        Err(ExtensionHttpFailure::Transport),
    ]);
    let host = stepped_host(&http);
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("send"))
        .expect("send is processed");
    for step in 1..=3 {
        http.release_one();
        arrived(&notified, step);
        session
            .deliver_http_responses()
            .expect("the part is delivered")
            .expect("one part arrived");
    }
    assert_eq!(logs(&mut session), ["received: 2"]);
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    assert!(!session.awaiting_http_responses());
    session.close().expect("the session closes");
}

/// A loopback model server that streams its answer slowly: it reads one
/// request head, answers a chunked `text/event-stream` head, writes one SSE
/// chunk every 20 ms until a write fails, and reports when it saw the client
/// close the connection.
#[cfg(feature = "http")]
struct SlowStream {
    port: u16,
    requested: std::sync::mpsc::Receiver<()>,
    disconnected: std::sync::mpsc::Receiver<Instant>,
    server: std::thread::JoinHandle<()>,
}

#[cfg(feature = "http")]
impl SlowStream {
    fn start() -> Self {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
        let port = listener
            .local_addr()
            .expect("the listener has an address")
            .port();
        let (requested_tx, requested) = std::sync::mpsc::channel();
        let (disconnected_tx, disconnected) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("the client connects");
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                assert_eq!(stream.read(&mut byte).expect("the head is read"), 1);
                head.push(byte[0]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .expect("the response head is written");
            requested_tx.send(()).expect("the test listens");
            let mut writer = stream.try_clone().expect("the stream clones");
            let streaming = std::thread::spawn(move || {
                let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"token\"}}]}\n\n";
                let framed = [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
                while writer.write_all(&framed).is_ok() {
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            // The client sends nothing more: the next read ends when it
            // closes the connection.
            let mut rest = [0_u8; 64];
            loop {
                match stream.read(&mut rest) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            disconnected_tx
                .send(Instant::now())
                .expect("the test listens");
            // A write fails soon after the close, which ends the stream.
            streaming.join().expect("the streaming thread ends");
        });
        Self {
            port,
            requested,
            disconnected,
            server,
        }
    }
}

#[cfg(feature = "http")]
#[test]
fn cancelling_the_invoking_state_closes_the_streaming_connection_and_ends_its_thread() {
    let server = SlowStream::start();
    let client = scxml_host_wasmtime::ExtensionHttpClient::new();
    let host = host_with(
        ServiceComponents::new()
            .with_values(
                ExtensionEnvironment::new([EnvVariable::new(
                    "ENDPOINT",
                    format!("http://127.0.0.1:{}/slow", server.port),
                    [ECHO.to_owned()],
                )])
                .expect("valid values"),
            )
            .with_outbound_http(Arc::new(client))
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    );
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    server
        .requested
        .recv_timeout(Duration::from_secs(10))
        .expect("the request reaches the server");
    // The answer is streaming and will never end on its own.
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        notified.load(Ordering::Acquire),
        0,
        "the request is waiting"
    );
    assert!(session.awaiting_http_responses());
    // Leaving the invoking state cancels the invocation (SCXML 6.4), which
    // closes the request's connection.
    let cancelled = Instant::now();
    session
        .send(&ScriptedEvent::named("leave"))
        .expect("leave is processed");
    assert_eq!(session.configuration().expect("configuration"), ["away"]);
    let disconnected = server
        .disconnected
        .recv_timeout(Duration::from_secs(2))
        .expect("the server sees the connection close within 2 s");
    assert!(disconnected.duration_since(cancelled) < Duration::from_secs(2));
    // The request's transport thread ends at once: it reports its (failed)
    // outcome within 2 s of the cancel.
    let deadline = Instant::now() + Duration::from_secs(2);
    while notified.load(Ordering::Acquire) == 0 {
        assert!(
            Instant::now() < deadline,
            "the transport thread ends within 2 s of the cancel"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Its outcome reaches no one.
    session
        .deliver_http_responses()
        .expect("the late outcome is dropped");
    assert!(logs(&mut session).is_empty());
    assert_eq!(session.configuration().expect("configuration"), ["away"]);
    server.server.join().expect("the server ends");
    session.close().expect("the session closes");
}

/// A service request has no time limit of its own (the invocation bounds
/// it, SCXML 6.4): a loopback server that says nothing for a while and then
/// streams its answer slowly is read to its end.
#[cfg(feature = "http")]
#[test]
fn a_slow_streaming_response_is_read_to_its_end() {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the client connects");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).expect("the head is read"), 1);
            head.push(byte[0]);
        }
        // Nothing at all, then the head and one chunk every 100 ms.
        std::thread::sleep(Duration::from_millis(1_500));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .expect("the response head is written");
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"token\"}}]}\n\n";
        let framed = [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
        for _ in 0..15 {
            stream.write_all(&framed).expect("a chunk is written");
            std::thread::sleep(Duration::from_millis(100));
        }
        stream.write_all(b"0\r\n\r\n").expect("the stream ends");
    });
    let host = host_with(
        ServiceComponents::new()
            .with_values(
                ExtensionEnvironment::new([EnvVariable::new(
                    "ENDPOINT",
                    format!("http://127.0.0.1:{port}/slow"),
                    [ECHO.to_owned()],
                )])
                .expect("valid values"),
            )
            .with_outbound_http(Arc::new(scxml_host_wasmtime::ExtensionHttpClient::new()))
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    );
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    let started = Instant::now();
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    arrived(&notified, 1);
    assert!(
        started.elapsed() >= Duration::from_millis(3_000),
        "the whole stream was read"
    );
    session
        .deliver_http_responses()
        .expect("the response is delivered")
        .expect("one response arrived");
    assert_eq!(logs(&mut session), ["fetched: 200"]);
    server.join().expect("the server ends");
    session.close().expect("the session closes");
}

/// `ExtensionHttpClient`'s `send` streams a response as it is read: a
/// loopback server that answers one chunk every 100 ms reaches the service
/// read by read, before the server has finished, and then its end.
#[cfg(feature = "http")]
#[test]
fn a_streamed_response_arrives_while_the_server_still_writes() {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();
    let (finished_tx, finished) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the client connects");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).expect("the head is read"), 1);
            head.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .expect("the response head is written");
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"token\"}}]}\n\n";
        let framed = [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
        for _ in 0..10 {
            stream.write_all(&framed).expect("a chunk is written");
            stream.flush().expect("the chunk is flushed");
            std::thread::sleep(Duration::from_millis(100));
        }
        stream.write_all(b"0\r\n\r\n").expect("the stream ends");
        finished_tx.send(()).expect("the test listens");
        chunk.len() * 10
    });
    let host = host_with(
        ServiceComponents::new()
            .with_values(
                ExtensionEnvironment::new([EnvVariable::new(
                    "ENDPOINT",
                    format!("http://127.0.0.1:{port}/stream"),
                    [ECHO.to_owned()],
                )])
                .expect("valid values"),
            )
            .with_outbound_http(Arc::new(scxml_host_wasmtime::ExtensionHttpClient::new()))
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    );
    let notified = Arc::new(AtomicUsize::new(0));
    let mut session = open_with(
        &host,
        &endpoint_document(BOUND),
        false,
        None,
        counted(&notified),
    )
    .expect("the session opens");
    session
        .send(&ScriptedEvent::named("send"))
        .expect("send is processed");
    // The status and the first reads arrive while the server still writes.
    arrived(&notified, 2);
    session
        .deliver_http_responses()
        .expect("the parts are delivered")
        .expect("parts arrived");
    let early = logs(&mut session);
    assert!(
        finished.try_recv().is_err(),
        "delivered before the server finished: {early:?}"
    );
    let mut logged = early;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !logged.iter().any(|line| line.starts_with("fetched")) {
        assert!(Instant::now() < deadline, "the body ends: {logged:?}");
        std::thread::sleep(Duration::from_millis(20));
        if session
            .deliver_http_responses()
            .expect("the parts are delivered")
            .is_some()
        {
            logged.extend(logs(&mut session));
        }
    }
    let total = server.join().expect("the server ends");
    let received: usize = logged
        .iter()
        .filter_map(|line| line.strip_prefix("received: "))
        .map(|count| count.parse::<usize>().expect("a byte count"))
        .sum();
    assert_eq!(received, total, "every byte arrived: {logged:?}");
    assert_eq!(logged.last().map(String::as_str), Some("fetched: 200"));
    assert!(
        logged
            .iter()
            .filter(|line| line.starts_with("received"))
            .count()
            > 1,
        "the body arrived in more than one read: {logged:?}"
    );
    session.close().expect("the session closes");
}
