//! Invoked services (SCXML 6.4) under the Wasmtime session host: the echo
//! and timer example (`components/scxml-service-example`) invoked by a live
//! session of the shipped machine, data model, and I/O processors.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies every component
//! into this suite's own artifact directory; the example service is bound
//! through the release manifest (and, for the caller path, by SHA-256). The
//! files example (`components/scxml-service-files`), whose package declares
//! `wasi:filesystem`, proves the declared-and-granted WASI path: it is bound
//! by SHA-256 with the run's grants.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;

use scxml_host_wasmtime::scxml_snapshot::Options;
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument, HostError,
    LiveClock, LiveHttp, LiveSession, LiveStatus, MachineScripts, RunFailure, ScriptedEvent,
    ServiceComponents, ServicePackage, SessionEmbedder, SessionHost, SessionHostComponents,
    SessionOpen, SessionOutput, SnapshotFailure, TelemetryProviders, TrustedComponentDigest,
    TrustedManifestDigest, WasiGrant, WasiGrants, WasmtimeHostConfig, authenticate_component,
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

/// The checked-in example manifest as the host's package.
fn package(service_uri: &str) -> ServicePackage {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../../../components/scxml-service-example/package/echo.service.json"
    ))
    .expect("the example manifest parses");
    manifest
        .verify(None)
        .expect("the example manifest verifies");
    ServicePackage {
        service_uri: service_uri.into(),
        package_version: manifest.package_version,
        service_artifact: manifest.service_artifact,
        wit_world: manifest.wit_world,
        wit_version: manifest.wit_version,
        http_origins: manifest.http_origins,
        env_slots: manifest.env_slots,
        wasi_permissions: manifest.wasi_permissions,
        events: scxml_host_wasmtime::ServiceEvents::default(),
    }
}

fn host_with(services: ServiceComponents) -> SessionHost {
    SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| host.with_service_components(services))
    .expect("the verified components link")
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

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

/// The parent document: `s` invokes the echo service with a 250 ms wakeup;
/// its `<finalize>` counts the messages it saw.
fn document() -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <datamodel><data id="finalized" expr="0"/></datamodel>
  <state id="s">
    <invoke type="{ECHO}" id="echo">
      <param name="delay" expr="250"/>
      <finalize><assign location="finalized" expr="finalized + 1"/></finalize>
    </invoke>
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
      <log label="failed" expr="_event.invokeid + ' ' + _event.type"/>
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
    assert_eq!(
        ir.invoke_types,
        [ECHO],
        "the IR lists the literal service type"
    );
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn open(host: &SessionHost) -> LiveSession {
    let (ir, data_model) = lower(&document());
    host.open(SessionOpen {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    })
    .expect("the session opens")
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
fn an_echo_service_wakes_echoes_and_completes_with_data() {
    let host = host();
    let mut session = open(&host);
    // The start ran the service: it asked for its wakeup and the session is
    // idle until then.
    assert_eq!(session.configuration().expect("configuration"), ["s"]);
    assert_eq!(session.next_deadline().expect("deadline"), Some(250));
    assert!(logs(&mut session).is_empty());

    // The wakeup delivers `echo.tick`, stamped with the invoke id, after the
    // invocation's `<finalize>`.
    let step = session.advance_to(250).expect("the wakeup fires");
    assert_eq!(step.status, LiveStatus::Running);
    assert_eq!(step.next_deadline, None, "the one wakeup was taken");
    assert_eq!(logs(&mut session), ["tick: echo 1 1"]);

    // `<send target="#_echo">` reaches the service, which echoes it.
    session
        .send(&ScriptedEvent::named("ping"))
        .expect("ping is processed");
    assert_eq!(
        logs(&mut session),
        [format!("echo.ping: echo {ECHO} 2")],
        "the echo carries the invoke id and the service type as its origin type"
    );

    // `stop` completes the service: `done.invoke.echo` carries its data.
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
    // The cancelled service's wakeup is gone and nothing it held remains:
    // the session snapshots again.
    assert_eq!(session.next_deadline().expect("deadline"), None);
    session
        .snapshot(&Options::default())
        .expect("no invoked service remains");
    let step = session.run_until_idle().expect("idle");
    assert_eq!(step.status, LiveStatus::Running);
    assert!(logs(&mut session).is_empty(), "no tick after the cancel");
    session.close().expect("the session closes");
}

#[test]
fn a_service_that_fails_is_reported_as_error_communication() {
    let host = host();
    let mut session = open(&host);
    // The service cannot echo a name `echo.` would push past the event-name
    // bound: its `send` fails and the host retires it.
    session
        .send(&ScriptedEvent::named("long"))
        .expect("the long event is processed");
    assert_eq!(logs(&mut session), ["failed: echo platform"]);
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    // No done.invoke follows, the wakeup is gone, and `#_echo` is now
    // unreachable.
    assert_eq!(session.next_deadline().expect("deadline"), None);
    session.run_until_idle().expect("idle");
    assert!(logs(&mut session).is_empty());
    session.close().expect("the session closes");
}

#[test]
fn an_unbound_literal_service_type_fails_the_session_start() {
    let host = host_with(ServiceComponents::new());
    let (ir, data_model) = lower(&document());
    let opened = host.open(SessionOpen {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    });
    match opened {
        Err(error) => assert!(
            matches!(error.failure, RunFailure::Machine { code, .. } if code == "unavailable"),
            "{error}"
        ),
        Ok(_) => panic!("an unbound literal service type fails the start closed"),
    }
}

#[test]
fn an_unbound_typeexpr_service_raises_error_execution() {
    let host = host();
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke typeexpr="'urn:example:missing'"/>
    <transition event="error.execution" target="failed"/>
  </state>
  <final id="failed"/>
</scxml>"#;
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "dynamic.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    assert!(ir.invoke_types.is_empty());
    let bytes = ir.to_json().expect("fixture IR encodes");
    let mut session = host
        .open(SessionOpen {
            machine_ir: &bytes,
            data_model: Some("ecmascript"),
            session_id: "root",
            embedder: Rc::new(NoResources),
            max_commands: 256,
            clock: LiveClock::Logical,
            record: false,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens");
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    session.close().expect("the session closes");
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
    assert_eq!(session.next_deadline().expect("deadline"), Some(250));
    session.close().expect("the session closes");
    // A service bound to a URI its component does not implement fails the
    // run when the invocation instantiates it.
    let other = "https://stateforward.org/scxml/services/other";
    let host = host_with(
        ServiceComponents::new()
            .bind(package(other), &caller)
            .expect("binding checks no component identity"),
    );
    let source = document().replace(ECHO, other);
    let (ir, data_model) = {
        let document =
            scxml_syntax::parse_xml(scxml_syntax::SourceText::new(SOURCE, "other.scxml", source))
                .expect("fixture parses");
        let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
        let data_model = ir.data_model.clone();
        (ir.to_json().expect("encodes"), data_model)
    };
    let opened = host.open(SessionOpen {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    });
    assert!(opened.is_err(), "a type-uri mismatch fails closed");
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

/// The checked-in files manifest as the host's package.
fn files_package() -> ServicePackage {
    let manifest = scxml_validator_core::ServicePackageManifest::from_json(include_bytes!(
        "../../../components/scxml-service-files/package/files.service.json"
    ))
    .expect("the files manifest parses");
    manifest.verify(None).expect("the files manifest verifies");
    assert_eq!(
        manifest.wasi_permissions,
        [scxml_host_wasmtime::WasiPermission::Filesystem {
            mount: "workspace".into(),
            access: scxml_host_wasmtime::FilesystemAccess::ReadWrite,
        }],
        "the files example declares exactly one read-write mount"
    );
    ServicePackage {
        service_uri: manifest.service_uri,
        package_version: manifest.package_version,
        service_artifact: manifest.service_artifact,
        wit_world: manifest.wit_world,
        wit_version: manifest.wit_version,
        http_origins: manifest.http_origins,
        env_slots: manifest.env_slots,
        wasi_permissions: manifest.wasi_permissions,
        events: scxml_host_wasmtime::ServiceEvents::default(),
    }
}

/// The files component, authenticated by its SHA-256 like any caller
/// service component.
fn files_component() -> AuthenticatedComponent {
    let bytes = &release().files_bytes;
    let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("digest is canonical");
    authenticate_service_component(bytes, digest).expect("the files component authenticates")
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

fn workspace_grant(directory: &Path) -> WasiGrants {
    WasiGrants::new()
        .grant(
            FILES,
            WasiGrant::Filesystem {
                mount: "workspace".into(),
                directory: directory.to_path_buf(),
            },
        )
        .expect("the grant is well formed")
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

fn open_files(host: &SessionHost) -> Result<LiveSession, scxml_host_wasmtime::SessionHostError> {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "files.scxml",
        FILES_DOCUMENT,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    assert_eq!(ir.invoke_types, [FILES]);
    let bytes = ir.to_json().expect("fixture IR encodes");
    host.open(SessionOpen {
        machine_ir: &bytes,
        data_model: Some("ecmascript"),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    })
}

fn files_host(package: ServicePackage, grants: WasiGrants) -> Result<SessionHost, HostError> {
    SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .and_then(|host| {
        host.with_service_components(
            ServiceComponents::new()
                .bind(package, &files_component())
                .expect("the files package binds")
                .with_wasi_grants(grants),
        )
    })
}

#[test]
fn a_granted_filesystem_permission_reads_and_writes_the_granted_directory() {
    let directory = scratch("granted");
    std::fs::write(directory.join("seed.txt"), "from the host").expect("seed is written");
    let host = files_host(files_package(), workspace_grant(&directory)).expect("the grant links");
    let mut session = open_files(&host).expect("the session opens");
    assert_eq!(
        logs(&mut session),
        [r#"done: {"seed":"from the host","wrote":22}"#],
        "the service read the host's seed through its granted mount"
    );
    assert_eq!(session.configuration().expect("configuration"), ["done"]);
    session.close().expect("the session closes");
    assert_eq!(
        std::fs::read_to_string(directory.join("out.txt")).expect("the service wrote out.txt"),
        "written by the service"
    );
}

#[test]
fn a_read_only_mount_refuses_the_write() {
    let directory = scratch("read-only");
    std::fs::write(directory.join("seed.txt"), "seed").expect("seed is written");
    let mut package = files_package();
    package.wasi_permissions = vec![scxml_host_wasmtime::WasiPermission::Filesystem {
        mount: "workspace".into(),
        access: scxml_host_wasmtime::FilesystemAccess::ReadOnly,
    }];
    let host = files_host(package, workspace_grant(&directory)).expect("the grant links");
    let mut session = open_files(&host).expect("the session opens");
    assert_eq!(logs(&mut session), ["failed: error.execution"]);
    session.close().expect("the session closes");
    assert!(!directory.join("out.txt").exists(), "nothing was written");
}

#[test]
fn an_ungranted_or_undeclared_wasi_permission_fails_the_binding_closed() {
    let directory = scratch("ungranted");
    let refused = |package: ServicePackage, grants: WasiGrants| match files_host(package, grants) {
        Err(HostError::Artifact(error)) => error,
        Err(other) => panic!("unexpected failure: {other}"),
        Ok(_) => panic!("the binding must fail closed"),
    };
    // Declared but not granted.
    assert_eq!(
        refused(files_package(), WasiGrants::new()),
        ArtifactError::WasiPermissionUngranted
    );
    // Granted for another mount: that grant names nothing the package
    // declares.
    let other_mount = WasiGrants::new()
        .grant(
            FILES,
            WasiGrant::Filesystem {
                mount: "other".into(),
                directory: directory.clone(),
            },
        )
        .expect("well formed");
    assert_eq!(
        refused(files_package(), other_mount),
        ArtifactError::WasiGrantUndeclared
    );
    // Granted beyond the declaration.
    let extra = workspace_grant(&directory)
        .grant(FILES, WasiGrant::Clocks)
        .expect("well formed");
    assert_eq!(
        refused(files_package(), extra),
        ArtifactError::WasiGrantUndeclared
    );
    // Granted to a URI no package binds.
    let unbound = workspace_grant(&directory)
        .grant(ECHO, WasiGrant::Random)
        .expect("well formed");
    assert_eq!(
        refused(files_package(), unbound),
        ArtifactError::WasiGrantUndeclared
    );
    // Malformed grants are refused when they are made.
    assert!(
        WasiGrants::new()
            .grant(
                FILES,
                WasiGrant::SocketsTcp {
                    connect: Vec::new()
                }
            )
            .is_err()
    );
    assert!(
        workspace_grant(&directory)
            .grant(
                FILES,
                WasiGrant::Filesystem {
                    mount: "workspace".into(),
                    directory: directory.clone(),
                },
            )
            .is_err(),
        "one mount is granted once"
    );
}

#[test]
fn an_undeclared_wasi_import_fails_the_session_start_closed() {
    // The files component imports wasi:filesystem, but this package declares
    // no WASI permission: the host links none, and the session start fails
    // before the first macrostep.
    let mut package = files_package();
    package.wasi_permissions = Vec::new();
    let host = files_host(package, WasiGrants::new()).expect("nothing to grant");
    match open_files(&host) {
        Err(error) => assert!(
            matches!(
                error.failure,
                RunFailure::Host(HostError::Artifact(ArtifactError::WasiImportUndeclared))
            ),
            "{error}"
        ),
        Ok(_) => panic!("an undeclared WASI import fails the start closed"),
    }
    // A package that declares another interface does not cover it either.
    let mut package = files_package();
    package.wasi_permissions = vec![scxml_host_wasmtime::WasiPermission::Random];
    let host = files_host(
        package,
        WasiGrants::new()
            .grant(FILES, WasiGrant::Random)
            .expect("well formed"),
    )
    .expect("the random grant links");
    assert!(
        open_files(&host).is_err(),
        "wasi:filesystem stays undeclared"
    );
}

#[test]
fn a_service_without_wasi_imports_needs_no_grant() {
    // The echo example imports no WASI interface: it binds and runs with no
    // grant, as before.
    let host = host_with(
        ServiceComponents::new()
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("binds")
            .with_wasi_grants(WasiGrants::new()),
    );
    let mut session = open(&host);
    assert_eq!(session.next_deadline().expect("deadline"), Some(250));
    session.close().expect("the session closes");
}

/// A transport that records each admitted request and answers 204.
#[derive(Debug, Default)]
struct RecordingHttp {
    urls: std::sync::Mutex<Vec<String>>,
}

impl scxml_host_wasmtime::ExtensionHttp for RecordingHttp {
    fn fetch(
        &self,
        request: &scxml_host_wasmtime::ExtensionHttpRequest,
    ) -> Result<scxml_host_wasmtime::ExtensionHttpResponse, scxml_host_wasmtime::ExtensionHttpFailure>
    {
        self.urls
            .lock()
            .expect("the request log is not poisoned")
            .push(request.url.clone());
        Ok(scxml_host_wasmtime::ExtensionHttpResponse {
            status: 204,
            body: Vec::new(),
        })
    }
}

/// `s` invokes the echo service binding its `endpoint` slot as `invoke`
/// says; `fetch` makes the service GET the slot's URL.
fn endpoint_document(invoke: &str) -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:env="{env}" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    {invoke}
    <transition event="fetch"><send event="fetch" target="#_echo"/></transition>
    <transition event="echo.fetched"><log label="fetched" expr="_event.data"/></transition>
    <transition event="error.execution" target="refused"/>
    <transition event="error.communication" target="failed"/>
  </state>
  <state id="refused"/>
  <state id="failed"/>
</scxml>"##,
        env = scxml_types::ENV_NAMESPACE_URI
    )
}

fn open_endpoint(host: &SessionHost, invoke: &str) -> LiveSession {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "endpoint.scxml",
        endpoint_document(invoke),
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    host.open(SessionOpen {
        machine_ir: &ir.to_json().expect("fixture IR encodes"),
        data_model: ir.data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    })
    .expect("the session opens")
}

/// `values` are `(name, value, granted package URI)`.
fn endpoint_host(
    http: &std::sync::Arc<RecordingHttp>,
    values: &[(&str, &str, &str)],
) -> SessionHost {
    host_with(
        ServiceComponents::new()
            .with_values(
                scxml_host_wasmtime::ExtensionEnvironment::new(values.iter().map(
                    |(name, value, uri)| {
                        scxml_host_wasmtime::EnvVariable::new(*name, *value, [(*uri).to_owned()])
                    },
                ))
                .expect("valid values"),
            )
            .with_outbound_http(std::sync::Arc::clone(http) as _)
            .bind(package(ECHO), component(ArtifactKind::ServiceExample))
            .expect("the example service binds"),
    )
}

#[test]
fn an_env_slot_reference_lives_for_the_whole_invocation() {
    let http = std::sync::Arc::new(RecordingHttp::default());
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", ECHO)]);
    for invoke in [
        format!(r#"<invoke type="{ECHO}" id="echo" env:endpoint="ENDPOINT"/>"#),
        format!(r#"<invoke type="{ECHO}" id="echo" env:endpointexpr="'END' + 'POINT'"/>"#),
    ] {
        let mut session = open_endpoint(&host, &invoke);
        // The reference bound at `start` is used by a later `send`: its
        // value reaches only the transport, at the origin it supplies.
        for _ in 0..2 {
            session
                .send(&ScriptedEvent::named("fetch"))
                .expect("fetch is processed");
            assert_eq!(logs(&mut session), ["fetched: 204"], "{invoke}");
        }
        session.close().expect("the session closes");
    }
    assert_eq!(
        *http.urls.lock().expect("the request log is not poisoned"),
        ["http://127.0.0.1:9/status"; 4]
    );
}

#[test]
fn an_unbound_out_of_scope_or_undeclared_env_slot_never_reaches_the_network() {
    let http = std::sync::Arc::new(RecordingHttp::default());
    // No value: the slot is absent, the service's fetch fails, and the host
    // retires it (error.communication).
    let host = endpoint_host(&http, &[]);
    let mut session = open_endpoint(
        &host,
        &format!(r#"<invoke type="{ECHO}" id="echo" env:endpoint="ENDPOINT"/>"#),
    );
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    session.close().expect("the session closes");
    // A value granted only to another package is absent exactly like no
    // value: the echo service's fetch fails the same way.
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", FILES)]);
    let mut session = open_endpoint(
        &host,
        &format!(r#"<invoke type="{ECHO}" id="echo" env:endpoint="ENDPOINT"/>"#),
    );
    session
        .send(&ScriptedEvent::named("fetch"))
        .expect("fetch is processed");
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    session.close().expect("the session closes");
    // A slot the package does not declare rejects the start: the `<invoke>`
    // raises error.execution and the service never runs.
    let host = endpoint_host(&http, &[("ENDPOINT", "http://127.0.0.1:9/status", ECHO)]);
    let mut session = open_endpoint(
        &host,
        &format!(r#"<invoke type="{ECHO}" id="echo" env:other="ENDPOINT"/>"#),
    );
    assert_eq!(session.configuration().expect("configuration"), ["refused"]);
    assert!(
        http.urls
            .lock()
            .expect("the request log is not poisoned")
            .is_empty()
    );
}

/// `s` invokes the echo service and relays `say` (immediately, or 10 ms
/// later for `later`) to it with the event's data.
fn contract_document() -> String {
    format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <onentry><send event="stop" target="#_echo" delay="20ms" id="early"/></onentry>
    <invoke type="{ECHO}" id="echo"/>
    <transition event="say"><send event="say" target="#_echo"><content expr="_event.data"/></send></transition>
    <transition event="later"><send event="say" target="#_echo" delay="10ms"><content expr="_event.data"/></send></transition>
    <transition event="stop"><send event="stop" target="#_echo"/></transition>
    <transition event="echo.say"><log label="echo.say" expr="JSON.stringify(_event.data)"/></transition>
    <transition event="error.execution"><log label="refused" expr="_event.name"/></transition>
    <transition event="error.communication" cond="_event.sendid == 'early'"><log label="undeliverable" expr="_event.sendid"/></transition>
    <transition event="error.communication" target="failed"><log label="failed" expr="_event.invokeid"/></transition>
  </state>
  <state id="failed"/>
</scxml>"##
    )
}

fn schema(name: &str, schema: &str) -> scxml_host_wasmtime::ServiceEventSchema {
    scxml_host_wasmtime::ServiceEventSchema {
        name: name.into(),
        schema: schema.into(),
    }
}

/// The echo package with a contract: it accepts `say` carrying an object
/// with `text`, and emits `echo.say` whose `text` is a string.
fn contract_host() -> SessionHost {
    let mut package = package(ECHO);
    package.events = scxml_host_wasmtime::ServiceEvents {
        emits: vec![schema(
            "echo.say",
            r#"{"properties":{"text":{"type":"string"}},"type":"object"}"#,
        )],
        accepts: vec![schema("say", r#"{"required":["text"],"type":"object"}"#)],
    };
    host_with(
        ServiceComponents::new()
            .bind(package, component(ArtifactKind::ServiceExample))
            .expect("the contracted example binds"),
    )
}

/// Opens the contract document and lets its early delayed send fall due:
/// scheduled before the service started, it is checked when it falls due,
/// and `stop` is not accepted, so it is undeliverable.
fn open_contract(host: &SessionHost) -> LiveSession {
    let mut session = open_contract_session(host);
    session.advance_to(20).expect("the early send falls due");
    assert_eq!(logs(&mut session), ["undeliverable: early"]);
    session
}

fn open_contract_session(host: &SessionHost) -> LiveSession {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "contract.scxml",
        contract_document(),
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    host.open(SessionOpen {
        machine_ir: &ir.to_json().expect("fixture IR encodes"),
        data_model: ir.data_model.as_deref(),
        session_id: "root",
        embedder: Rc::new(NoResources),
        max_commands: 256,
        clock: LiveClock::Logical,
        record: false,
        replay: None,
        http: LiveHttp::default(),
    })
    .expect("the session opens")
}

fn say(name: &str, data: &str) -> ScriptedEvent {
    ScriptedEvent {
        data: scxml_types::DataValue::canonical_json_object(data).expect("an object"),
        ..ScriptedEvent::named(name)
    }
}

#[test]
fn a_send_the_service_contract_refuses_raises_error_execution() {
    let host = contract_host();
    let mut session = open_contract(&host);
    session.send(&say("say", r#"{"text":"hi"}"#)).expect("say");
    assert_eq!(logs(&mut session), [r#"echo.say: {"text":"hi"}"#]);
    // Data the accepted schema refuses, and an event the contract does not
    // declare, raise error.execution at the `<send>`; the service runs on.
    session.send(&say("say", r#"{"n":1}"#)).expect("say");
    assert_eq!(logs(&mut session), ["refused: error.execution"]);
    session.send(&ScriptedEvent::named("stop")).expect("stop");
    assert_eq!(logs(&mut session), ["refused: error.execution"]);
    // A delayed send is checked when it is sent.
    session.send(&say("later", r#"{"n":1}"#)).expect("later");
    assert_eq!(logs(&mut session), ["refused: error.execution"]);
    assert_eq!(session.next_deadline().expect("deadline"), None);
    session
        .send(&say("later", r#"{"text":"late"}"#))
        .expect("later");
    session
        .run_until_idle()
        .expect("the delayed send falls due");
    assert_eq!(logs(&mut session), [r#"echo.say: {"text":"late"}"#]);
    assert_eq!(session.configuration().expect("configuration"), ["s"]);
    session.close().expect("the session closes");
}

#[test]
fn an_emit_the_service_contract_refuses_retires_the_service() {
    let host = contract_host();
    let mut session = open_contract(&host);
    // Accepted (`text` is present), but the echo's `text` is no string:
    // the emit is invalid, the host retires the service, and the invoking
    // session gets error.communication with the invoke id.
    session.send(&say("say", r#"{"text":5}"#)).expect("say");
    assert_eq!(logs(&mut session), ["failed: echo"]);
    assert_eq!(session.configuration().expect("configuration"), ["failed"]);
    session.close().expect("the session closes");
}

#[test]
fn a_malformed_contract_fails_the_binding_closed() {
    for events in [
        scxml_host_wasmtime::ServiceEvents {
            emits: vec![schema("echo.say", r#"{ "type": "string" }"#)],
            accepts: Vec::new(),
        },
        scxml_host_wasmtime::ServiceEvents {
            emits: vec![schema("echo.*", "true")],
            accepts: Vec::new(),
        },
        scxml_host_wasmtime::ServiceEvents {
            emits: vec![schema("b", "true"), schema("a", "true")],
            accepts: Vec::new(),
        },
    ] {
        let mut package = package(ECHO);
        package.events = events;
        let error = ServiceComponents::new()
            .bind(package, component(ArtifactKind::ServiceExample))
            .expect_err("a malformed contract is refused");
        assert!(
            matches!(
                error,
                HostError::Artifact(ArtifactError::EventContractInvalid)
            ),
            "{error}"
        );
    }
}
