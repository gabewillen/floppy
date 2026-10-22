//! Extension components under the Wasmtime session host: the example
//! namespace (`components/scxml-extension-example`), linked to real sessions
//! of the shipped machine, data models, and I/O processors.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies every component
//! into this suite's own artifact directory; extension components are bound
//! through the release manifest (and, for the caller path, by SHA-256).

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument,
    ExtensionComponents, ExtensionPackage, HostError, LiveClock, LiveHttp, MachineScripts,
    RunFailure, ScriptedEvent, SessionEmbedder, SessionHost, SessionHostComponents, SessionOpen,
    SessionOutcome, SessionRun, SnapshotRequest, TelemetryProviders, Terminal,
    TrustedComponentDigest, TrustedManifestDigest, WasmtimeHostConfig, WasmtimeStoreConfig,
    authenticate_component, authenticate_extension_component,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const EXAMPLE_URI: &str = "https://stateforward.org/scxml/example";

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
    (ArtifactKind::ExtensionExample, "scxml_extension_example"),
];

struct Release {
    components: Vec<AuthenticatedComponent>,
    example_bytes: Vec<u8>,
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
            let artifacts = target.join("wasm-artifacts/namespace-extension components");
            let status = Command::new("sh")
                .arg(workspace().join("scripts/verify-wasm-capabilities.sh"))
                .args(COMPONENTS.iter().map(|(_, name)| *name))
                .current_dir(workspace())
                .env("CARGO_TARGET_DIR", &target)
                // This suite's own artifact directory: suites sharing the Cargo
                // target never rewrite a manifest another suite is reading.
                .env("SCXML_WASM_ARTIFACTS_DIR", &artifacts)
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
        let example_bytes =
            std::fs::read(artifacts.join("scxml_extension_example.virtualized.wasm"))
                .expect("example artifact is readable");
        Release {
            components,
            example_bytes,
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

fn package(namespace_uri: &str) -> ExtensionPackage {
    let schema = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{namespace_uri}"/>"#
    )
    .into_bytes();
    ExtensionPackage {
        namespace_uri: namespace_uri.into(),
        package_version: "0.2.0".into(),
        extension_artifact: "scxml_extension_example.virtualized.wasm".into(),
        schema_artifact: "example.xsd".into(),
        schema_sha256: format!("{:x}", Sha256::digest(&schema)),
        wit_world: "stateforward:scxml/executable-content-extension".into(),
        wit_version: "0.2.0".into(),
        schema,
    }
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

fn host_with(
    config: &WasmtimeHostConfig,
    providers: &TelemetryProviders,
    extensions: ExtensionComponents,
) -> Result<SessionHost, HostError> {
    SessionHost::new(&session_components(), config, providers)?
        .with_extension_components(extensions)
}

/// The example namespace bound to its own URI.
fn extensions() -> ExtensionComponents {
    ExtensionComponents::new()
        .bind(
            package(EXAMPLE_URI),
            component(ArtifactKind::ExtensionExample),
        )
        .expect("the example namespace binds")
}

fn host() -> SessionHost {
    host_with(
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
        extensions(),
    )
    .expect("the verified components link")
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

fn scxml(data_model: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="{data_model}" initial="s0">{body}</scxml>"#
    )
}

fn run_on(
    host: &SessionHost,
    source: &str,
    events: &[&str],
) -> Result<SessionOutcome, scxml_host_wasmtime::SessionHostError> {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "namespace.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let bytes = ir.to_json().expect("fixture IR encodes");
    let events: Vec<ScriptedEvent> = events.iter().copied().map(ScriptedEvent::named).collect();
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
}

fn labels(outcome: &SessionOutcome) -> Vec<String> {
    outcome
        .root_logs
        .iter()
        .map(|log| log.label.clone().unwrap_or_default())
        .collect()
}

fn texts(outcome: &SessionOutcome) -> Vec<String> {
    outcome
        .root_logs
        .iter()
        .map(|log| log.text.clone())
        .collect()
}

#[test]
fn extension_failures_and_declined_elements_raise_error_execution() {
    let host = host();
    for element in ["<example:fail/>", "<example:unknown/>"] {
        let outcome = run_on(
            &host,
            &scxml(
                "ecmascript",
                &format!(
                    r#"<datamodel><data id="x"/></datamodel>
                       <state id="s0">
                         <onentry>{element}<log label="after"/></onentry>
                         <transition event="error.execution" target="failed"/>
                       </state>
                       <final id="failed"/>"#
                ),
            ),
            &[],
        )
        .expect("the run completes");
        assert_eq!(outcome.configuration, ["failed"], "{element}");
        assert!(
            labels(&outcome).is_empty(),
            "{element}: the failure aborts the rest of the block"
        );
    }
}

#[test]
fn raise_send_and_in_state_act_on_the_executing_session() {
    let host = host();
    let outcome = run_on(
        &host,
        &scxml(
            "null",
            r##"<state id="s0">
                  <onentry>
                    <example:raise event="not.here" in="elsewhere"/>
                    <example:send event="external.one"/>
                    <example:send event="internal.one" target="#_internal"/>
                    <example:raise event="internal.two" in="s0"/>
                  </onentry>
                  <transition event="not.here" target="wrong"/>
                  <transition event="internal.one" target="s1"><log label="internal.one"/></transition>
                </state>
                <state id="s1">
                  <transition event="internal.two" target="s2"><log label="internal.two"/></transition>
                </state>
                <state id="s2">
                  <transition event="external.one" target="done"><log label="external.one"/></transition>
                </state>
                <final id="wrong"/><final id="done"/>"##,
        ),
        &[],
    )
    .expect("the run completes");
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(
        labels(&outcome),
        ["internal.one", "internal.two", "external.one"],
        "internal events precede the self-addressed external one"
    );
}

/// `session-snapshot-host.snapshot` hands an extension component the canonical
/// `scxml.session-snapshot/v2` document of the executing session: the
/// configuration its effect observes, the transitions walk from there, the
/// events this `execute` call queued so far, its delayed sends, and the data
/// model unless excluded; `invocations` and `previews` are always excluded and
/// carriers stripped. The same call answers `unavailable` in `initialize`.
#[test]
fn a_extension_reads_the_executing_session_snapshot_only_during_execution() {
    let host = host();
    let outcome = run_on(
        &host,
        &scxml(
            "ecmascript",
            r#"<datamodel><data id="secret" expr="'s3cret'"/><data id="doc"/><data id="full"/><data id="init"/></datamodel>
                <state id="s0">
                  <transition event="go" target="ask"/>
                </state>
                <state id="ask">
                  <onentry>
                    <send event="later" delay="5s" id="pending.one"/>
                    <example:raise event="queued.one"/>
                    <example:snapshot location="doc" exclude="data_model"/>
                    <example:snapshot location="full"/>
                    <example:snapshot location="init" at="initialize"/>
                    <log label="doc" expr="doc"/>
                    <log label="full" expr="full"/>
                    <log label="init" expr="init"/>
                  </onentry>
                  <transition event="answer.yes" target="done"/>
                  <transition event="answer.no answer.maybe" target="done" cond="true"/>
                  <transition event="queued.one"/>
                </state>
                <final id="done"/>"#,
        ),
        &["go"],
    )
    .expect("the run completes");
    let texts = texts(&outcome);
    let [doc, full, init] = texts.as_slice() else {
        panic!("three logs: {texts:?}");
    };
    assert_eq!(init, "unavailable", "no snapshot outside execute");
    for text in [doc, full] {
        assert!(
            scxml_snapshot::check(text.as_bytes())
                .expect("a structurally valid document")
                .is_empty(),
            "the document is canonical and consistent"
        );
    }
    let doc: serde_json::Value = serde_json::from_str(doc).expect("JSON");
    assert_eq!(doc["version"], "scxml.session-snapshot/v2");
    assert_eq!(
        doc["options"],
        serde_json::json!({
            "exclude": ["data_model", "previews", "invocations"],
            "trace_context": "strip",
            "point": "executable-content"
        })
    );
    let root = &doc["snapshot"]["root"];
    assert_eq!(root["session_id"], "root");
    assert_eq!(root["status"], "running");
    assert_eq!(
        root["configuration"],
        serde_json::json!({"atomic": ["ask"], "full": ["ask"]}),
        "the configuration the entry content observes; s0 was exited"
    );
    assert!(root.get("data_model").is_none());
    assert!(root.get("invocations").is_none());
    assert_eq!(root["queues"]["internal"][0]["name"], "queued.one");
    assert_eq!(root["pending_sends"][0]["sendid"], "pending.one");
    assert_eq!(
        listed_transitions(root),
        [
            ("ask".to_owned(), vec!["answer.yes".to_owned()], false),
            (
                "ask".to_owned(),
                vec!["answer.no".to_owned(), "answer.maybe".to_owned()],
                true
            ),
            ("ask".to_owned(), vec!["queued.one".to_owned()], false),
        ]
    );
    let full: serde_json::Value = serde_json::from_str(full).expect("JSON");
    let variables = full["snapshot"]["root"]["data_model"]
        .as_array()
        .expect("the data model is included unless excluded");
    assert!(
        variables
            .iter()
            .any(|variable| variable["name"] == "secret" && variable["value"]["value"] == "s3cret"),
        "{variables:?}"
    );
    assert!(
        variables
            .iter()
            .any(|variable| variable["name"] == "doc" && variable["value"]["type"] == "text"),
        "an assignment the extension component made earlier in the `execute` call block is visible"
    );
}

/// An ECMAScript child document that may use the example namespace.
fn child(initial: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="ecmascript" initial="{initial}">{body}</scxml>"#
    )
}

/// `(label, text)` of every root log record, in order.
fn records(outcome: &SessionOutcome) -> Vec<(String, String)> {
    outcome
        .root_logs
        .iter()
        .map(|log| (log.label.clone().unwrap_or_default(), log.text.clone()))
        .collect()
}

/// A child `kid` holding `secret` that waits in `k0` once it told its
/// parent it is `ready`.
fn waiting_kid() -> String {
    format!(
        r#"<invoke id="kid"><content>{}</content></invoke>"#,
        child(
            "k0",
            r##"<datamodel><data id="secret" expr="'k'"/></datamodel><state id="k0"><onentry><send event="ready" target="#_parent"/></onentry><transition event="poke" target="k1"/></state><state id="k1"/>"##,
        )
    )
}

/// The checkpoint document of the session `session_id` in `text`.
fn checkpoint(text: &str, session_id: &str) -> serde_json::Value {
    assert!(
        scxml_snapshot::check(text.as_bytes())
            .expect("a structurally valid document")
            .is_empty(),
        "the document is canonical and consistent: {text}"
    );
    let doc: serde_json::Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(doc["version"], "scxml.session-snapshot/v2");
    // `checkpoint` is the default, never written.
    assert!(doc["options"].get("point").is_none(), "{doc}");
    assert_eq!(doc["options"]["trace_context"], "strip");
    let exclude = doc["options"]["exclude"].as_array().expect("exclude");
    assert!(exclude.contains(&"invocations".into()) && exclude.contains(&"previews".into()));
    assert_eq!(doc["snapshot"]["root"]["session_id"], session_id);
    doc
}

/// `session-snapshot-host.snapshot` with a target reads another session of
/// the run, resolved as a `<send>` target is (`#_<invokeid>`, `#_parent`,
/// `#_scxml_<sessionid>`): that session's canonical document at its last
/// checkpoint, its configuration and data model, with `exclude` honored.
#[test]
fn a_extension_reads_another_session_of_the_run_by_its_send_target() {
    let a = child(
        "a0",
        r##"<datamodel><data id="up"/><data id="sib"/></datamodel><state id="a0"><transition event="read"><example:snapshot location="up" target="#_parent"/><example:snapshot location="sib" target="#_scxml_root.kid" exclude="data_model"/><send event="read" target="#_parent"><param name="up" expr="up"/><param name="sib" expr="sib"/></send></transition></state>"##,
    );
    let outcome = run_on(
        &host(),
        &scxml(
            "ecmascript",
            &format!(
                r#"<datamodel><data id="flag" expr="'root'"/><data id="byid"/><data id="byaddress"/></datamodel>
                <state id="s0" initial="wait">{}<invoke id="a"><content>{a}</content></invoke>
                  <state id="wait"><transition event="ready" target="read"/></state>
                  <state id="read">
                    <onentry>
                      <example:snapshot location="byid" target="{}"/>
                      <example:snapshot location="byaddress" target="{}"/>
                      <log label="byid" expr="byid"/>
                      <log label="byaddress" expr="byaddress"/>
                      <send event="read" target="{}"/>
                    </onentry>
                    <transition event="read" target="done">
                      <log label="up" expr="_event.data.up"/>
                      <log label="sib" expr="_event.data.sib"/>
                    </transition>
                  </state>
                </state>
                <final id="done"/>"#,
                waiting_kid(),
                "#_kid",
                "#_scxml_root.kid",
                "#_a",
            ),
        ),
        &[],
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    let records = records(&outcome);
    let text = |label: &str| {
        records
            .iter()
            .find(|(found, _)| found == label)
            .map_or_else(
                || panic!("{label} in {records:?}"),
                |(_, text)| text.clone(),
            )
    };
    let (byid, byaddress) = (text("byid"), text("byaddress"));
    assert_eq!(byid, byaddress, "both addresses name the same session");
    let kid = checkpoint(&byid, "root.kid");
    let root = &kid["snapshot"]["root"];
    assert_eq!(
        root["configuration"],
        serde_json::json!({"atomic": ["k0"], "full": ["k0"]})
    );
    assert!(
        root["data_model"]
            .as_array()
            .expect("the data model")
            .iter()
            .any(|variable| variable["name"] == "secret" && variable["value"]["value"] == "k"),
        "{root}"
    );
    let parent = checkpoint(&text("up"), "root");
    assert!(
        parent["snapshot"]["root"]["data_model"]
            .as_array()
            .expect("the data model")
            .iter()
            .any(|variable| variable["name"] == "flag")
    );
    let sibling = checkpoint(&text("sib"), "root.kid");
    assert!(sibling["snapshot"]["root"].get("data_model").is_none());
}

/// A target that is malformed, names no session of the run, or names one
/// that finished is `unavailable` (the example assigns that code label), and
/// nothing changes: the read of a live session changes neither that
/// session's document nor its trace.
#[test]
fn a_extension_snapshot_of_an_unavailable_target_fails_and_changes_nothing() {
    let reads = |targets: &[&str]| {
        let blocks = targets.iter().fold(String::new(), |mut blocks, target| {
            write!(
                blocks,
                r#"<onentry><example:snapshot location="doc" target="{target}"/><log label="read" expr="doc.length > 20 ? 'document' : doc"/></onentry>"#
            )
            .expect("writing to a String cannot fail");
            blocks
        });
        let gone = child("g0", r#"<final id="g0"/>"#);
        run_on(
            &host(),
            &scxml(
                "ecmascript",
                &format!(
                    r#"<datamodel><data id="doc"/><data id="n" expr="0"/></datamodel>
                    <state id="s0" initial="wait">{}<invoke id="gone"><content>{gone}</content></invoke>
                      <state id="wait"><transition event="ready" cond="n === 1" target="read"/><transition event="done.invoke.gone"><assign location="n" expr="1"/></transition><transition event="ready"><assign location="n" expr="1"/><raise event="ready"/></transition></state>
                      <state id="read">{blocks}<onentry><send event="end" delay="1ms"/></onentry><transition event="end" target="done"/></state>
                    </state>
                    <final id="done"/>"#,
                    waiting_kid()
                ),
            ),
            &[],
        )
        .expect("the run completes")
    };
    let failed = reads(&[
        "nowhere",
        "#_scxml_root.nobody",
        "#_nobody",
        "#_scxml_root.gone",
    ]);
    assert_eq!(failed.configuration, ["done"], "{failed:?}");
    assert_eq!(
        texts(&failed),
        ["unavailable", "unavailable", "unavailable", "unavailable"]
    );
    // The kid's trace is the same whether or not the root read it twice.
    let kid_trace = |outcome: &SessionOutcome| -> Vec<String> {
        outcome
            .trace
            .iter()
            .filter(|entry| entry.session == "root.kid")
            .map(|entry| format!("{:?}", entry.observation))
            .collect()
    };
    let read = reads(&["#_kid", "#_kid"]);
    assert_eq!(texts(&read), ["document", "document"]);
    let unread = reads(&[]);
    assert_eq!(kid_trace(&read), kid_trace(&unread));
}

/// A recorded live session whose extension reads another session of the
/// run replays to the same documents: the read records nothing, and is a
/// pure function of the run's state, which replay reproduces.
#[test]
fn a_recorded_read_of_another_session_replays_to_the_same_document() {
    let source = scxml(
        "ecmascript",
        &format!(
            r##"<datamodel><data id="doc"/></datamodel>
            <state id="s0">{}
              <transition event="go"><example:snapshot location="doc" target="#_kid"/><log label="doc" expr="doc"/></transition>
            </state>"##,
            waiting_kid()
        ),
    );
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "namespace.scxml",
        source.as_str(),
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let bytes = ir.to_json().expect("fixture IR encodes");
    let host = host();
    let open = |record: bool| {
        host.open(SessionOpen {
            machine_ir: &bytes,
            data_model: ir.data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(NoResources),
            max_commands: 256,
            clock: LiveClock::Logical,
            record,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens")
    };
    let mut live = open(true);
    live.send(&ScriptedEvent::named("go")).expect("go");
    let outputs = live.take_outputs();
    let read = format!("{outputs:?}");
    assert!(read.contains("root.kid"), "the root read its child: {read}");
    let records = live.take_recorded();
    live.close().expect("close");
    let mut replay = open(false);
    for record in &records {
        replay.apply(record).expect("the record applies");
    }
    assert_eq!(replay.take_outputs(), outputs);
    replay.close().expect("close");
}

/// The entry content of a compound state reads a snapshot taken before any
/// of its children is entered: the compound state is in the configuration
/// and no atomic state is yet, and the walk starts from the compound state.
#[test]
fn a_extension_reads_the_snapshot_in_a_compound_state_entry_content() {
    let host = host();
    let outcome = run_on(
        &host,
        &scxml(
            "ecmascript",
            r#"<datamodel><data id="doc"/></datamodel>
                <state id="s0">
                  <transition target="outer"/>
                </state>
                <state id="outer">
                  <onentry>
                    <example:snapshot location="doc" exclude="data_model"/>
                    <log label="doc" expr="doc"/>
                  </onentry>
                  <transition event="answer.yes" target="done"/>
                  <state id="inner"/>
                </state>
                <final id="done"/>"#,
        ),
        &[],
    )
    .expect("the run completes");
    let texts = texts(&outcome);
    let [doc] = texts.as_slice() else {
        panic!("one log: {texts:?}");
    };
    assert!(
        scxml_snapshot::check(doc.as_bytes())
            .expect("a structurally valid document")
            .is_empty(),
        "the document is canonical and consistent: {doc}"
    );
    let doc: serde_json::Value = serde_json::from_str(doc).expect("JSON");
    let root = &doc["snapshot"]["root"];
    assert_eq!(
        root["configuration"],
        serde_json::json!({"atomic": [], "full": ["outer"]}),
        "the entry content observes the compound state before its children"
    );
    assert_eq!(
        listed_transitions(root),
        [("outer".to_owned(), vec!["answer.yes".to_owned()], false)]
    );
    assert_eq!(outcome.configuration, ["inner"]);
}

/// `(source, events, has_cond)` of each listed transition of a session
/// object; an executable-content snapshot has no previews.
fn listed_transitions(root: &serde_json::Value) -> Vec<(String, Vec<String>, bool)> {
    root["transitions"]
        .as_array()
        .expect("listed transitions")
        .iter()
        .map(|transition| {
            assert!(transition.get("exit").is_none() && transition.get("entry").is_none());
            (
                transition["source"].as_str().expect("source").to_owned(),
                transition["events"]
                    .as_array()
                    .expect("events")
                    .iter()
                    .map(|event| event.as_str().expect("event").to_owned())
                    .collect(),
                transition["has_cond"].as_bool().expect("has_cond"),
            )
        })
        .collect()
}

/// An extension component's `execution-host.send` routes exactly as `<send>`
/// routes an SCXML Event I/O Processor send; a request for any other processor
/// is `invalid-input`, which the extension component's failed `execute` call
/// raises as `error.execution` (SCXML 6.2: an unsupported type is
/// `error.execution`).
#[test]
fn a_extension_send_through_another_processor_raises_error_execution() {
    let host = host();
    let outcome = run_on(
        &host,
        &scxml(
            "null",
            r##"<state id="s0">
                  <onentry>
                    <example:send event="scxml.typed" type="scxml" target="#_internal"/>
                    <example:send event="via.http" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="#_internal"/>
                  </onentry>
                  <transition event="scxml.typed"><log label="scxml.typed"/></transition>
                  <transition event="via.http" target="wrong"/>
                  <transition event="error.execution" target="done"><log label="rejected"/></transition>
                </state>
                <final id="wrong"/><final id="done"/>"##,
        ),
        &[],
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(labels(&outcome), ["scxml.typed", "rejected"]);
}

/// An extension component's send to `#_internal` meets `<send>`'s own rules
/// (SCXML 6.2.1: 'delay' "Must not occur ... when the attribute 'target' has
/// the value \"_internal\"") and the WIT `send-request` precondition (an event
/// send id equal to the request's): each violation raises error.execution in
/// the executing session, as `<send>` does, and queues nothing.
#[test]
fn a_extension_send_to_internal_meets_the_send_rules() {
    let host = host();
    for send in [
        r##"<example:send event="leaked" target="#_internal" delay="5" id="d"/>"##,
        r##"<example:send event="leaked" target="#_internal" id="a" eventid="b"/>"##,
        r##"<example:send event="leaked" target="#_internal" eventid="b"/>"##,
    ] {
        let outcome = run_on(
            &host,
            &scxml(
                "null",
                &format!(
                    r#"<state id="s0">
                         <onentry>{send}</onentry>
                         <transition event="leaked" target="wrong"/>
                         <transition event="error.execution" target="done"/>
                       </state>
                       <final id="wrong"/><final id="done"/>"#
                ),
            ),
            &[],
        )
        .expect("the run completes");
        assert_eq!(outcome.configuration, ["done"], "{send}");
    }
    // A matching send id is delivered.
    let outcome = run_on(
        &host,
        &scxml(
            "null",
            r##"<state id="s0">
                  <onentry><example:send event="ok" target="#_internal" id="a" eventid="a"/></onentry>
                  <transition event="ok" target="done"/>
                  <transition event="error.execution" target="wrong"/>
                </state>
                <final id="wrong"/><final id="done"/>"##,
        ),
        &[],
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
}

#[test]
fn an_unbound_namespace_fails_the_session_closed() {
    let host = host();
    let failure = run_on(
        &host,
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:other="https://example.org/unbound" version="1.0" datamodel="null" initial="s0"><state id="s0"><onentry><other:thing/></onentry><transition event="error.execution" target="failed"/></state><final id="failed"/></scxml>"#,
        &[],
    )
    .expect_err("an unbound namespace never degrades into core SCXML");
    assert!(
        matches!(
            failure.failure,
            RunFailure::Machine {
                code: "unavailable",
                ..
            }
        ),
        "{:?}",
        failure.failure
    );
    let unlinked = SessionHost::new(
        &session_components(),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the verified components link");
    let failure = run_on(
        &unlinked,
        &scxml(
            "null",
            r#"<state id="s0"><onentry><example:raise/></onentry></state>"#,
        ),
        &[],
    )
    .expect_err("a host with no extension component bound fails the session closed");
    assert!(matches!(
        failure.failure,
        RunFailure::Machine {
            code: "unavailable",
            ..
        }
    ));
}

#[test]
fn a_trapping_extension_raises_error_execution_and_is_retired_for_the_session() {
    let host = host_with(
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
        extensions()
            .with_call_fuel(5_000_000)
            .expect("non-zero fuel"),
    )
    .expect("the verified components link");
    let outcome = run_on(
        &host,
        &scxml(
            "null",
            r#"<state id="s0">
                 <onentry><example:spin/></onentry>
                 <transition event="error.execution" target="s1"><log label="spun"/></transition>
               </state>
               <state id="s1">
                 <onentry><example:raise event="alive"/></onentry>
                 <transition event="alive" target="wrong"/>
                 <transition event="error.execution" target="retired"><log label="retired"/></transition>
               </state>
               <final id="wrong"/><final id="retired"/>"#,
        ),
        &[],
    )
    .expect("an extension component trap is a recoverable execution error");
    assert_eq!(outcome.configuration, ["retired"]);
    assert_eq!(labels(&outcome), ["spun", "retired"]);
}

#[test]
fn extension_instructions_are_charged_to_the_executing_call() {
    // The call budget (store fuel) is below the extension component's own cap:
    // the spinning extension component exhausts what the machine call has left,
    // so the machine call itself runs out of fuel instead of continuing.
    let config = WasmtimeHostConfig {
        store: WasmtimeStoreConfig {
            fuel: 50_000_000,
            ..WasmtimeStoreConfig::default()
        },
        ..WasmtimeHostConfig::default()
    };
    let host = host_with(&config, &TelemetryProviders::new(), extensions())
        .expect("the verified components link");
    let control = run_on(
        &host,
        &scxml(
            "null",
            r#"<state id="s0"><onentry><example:raise event="go"/></onentry><transition event="go" target="done"/></state><final id="done"/>"#,
        ),
        &[],
    )
    .expect("the budget covers an ordinary extension");
    assert_eq!(control.terminal, Terminal::Finished);
    let failure = run_on(
        &host,
        &scxml(
            "null",
            r#"<state id="s0"><onentry><example:spin/></onentry><transition event="error.execution" target="done"/></state><final id="done"/>"#,
        ),
        &[],
    )
    .expect_err("the spin spends the machine call's budget");
    assert!(
        matches!(failure.failure, RunFailure::Host(HostError::Wasmtime(_))),
        "{:?}",
        failure.failure
    );
}

#[test]
fn every_session_owns_its_extension_instances_and_they_are_initialized_and_shut_down() {
    let exporter = InMemorySpanExporter::default();
    let providers = TelemetryProviders::from_providers(
        SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build(),
        SdkMeterProvider::builder().build(),
    );
    let host = host_with(&WasmtimeHostConfig::default(), &providers, extensions())
        .expect("the verified components link");
    // Every session has its own extension component instances (and the child
    // compiles against the host's bound packages).
    let child = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="ecmascript" initial="c"><state id="c"><onentry><example:raise event="child.ran"/></onentry><transition event="child.ran" target="f"/></state><final id="f"><donedata><content expr="'child'"/></donedata></final></scxml>"#
    );
    let outcome = run_on(
        &host,
        &scxml(
            "ecmascript",
            &format!(
                r#"<state id="s0">
                     <onentry><example:raise event="parent.ran"/></onentry>
                     <invoke type="scxml"><content>{child}</content></invoke>
                     <transition event="done.invoke" target="done"><log label="child" expr="JSON.stringify(_event.data)"/></transition>
                     <transition event="error.execution" target="failed"/>
                   </state>
                   <final id="done"/><final id="failed"/>"#
            ),
        ),
        &[],
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(texts(&outcome), ["\"child\""]);
    drop(host);
    let spans = exporter.get_finished_spans().expect("spans are readable");
    let extension_signals = |operation: &str| {
        spans
            .iter()
            .filter(|span| {
                span.name == operation
                    && span.attributes.contains(&KeyValue::new(
                        "scxml.module",
                        "executable-content-extension",
                    ))
                    && span
                        .attributes
                        .contains(&KeyValue::new("scxml.outcome", "ok"))
            })
            .count()
    };
    // Two sessions (root and child), one bound extension component each.
    assert_eq!(extension_signals("initialize"), 2);
    assert_eq!(
        extension_signals("shutdown"),
        2,
        "every instance shuts down once"
    );
    // Execution propagates the effect's trace context: every extension
    // component `execute` signal joins a trace of the machine's own batch
    // spans.
    let machine_traces: Vec<_> = spans
        .iter()
        .filter(|span| {
            span.attributes
                .contains(&KeyValue::new("scxml.module", "machine-component"))
        })
        .map(|span| span.span_context.trace_id())
        .collect();
    let executes: Vec<_> = spans
        .iter()
        .filter(|span| {
            span.name == "execute"
                && span.attributes.contains(&KeyValue::new(
                    "scxml.module",
                    "executable-content-extension",
                ))
        })
        .collect();
    assert!(!executes.is_empty());
    assert!(
        executes
            .iter()
            .all(|span| machine_traces.contains(&span.span_context.trace_id())),
        "extension component signals share the executing batch's trace"
    );
    assert!(
        spans.iter().all(|span| span
            .attributes
            .iter()
            .all(|attribute| !attribute.value.as_str().contains("stateforward.org"))),
        "namespace URIs never become attributes"
    );
}

#[test]
fn bindings_are_checked_before_and_at_instantiation() {
    let example = component(ArtifactKind::ExtensionExample);
    for uri in [
        "http://www.w3.org/2005/07/scxml",
        "http://www.w3.org/XML/1998/namespace",
        "relative/uri",
    ] {
        assert!(matches!(
            ExtensionComponents::new().bind(package(uri), example),
            Err(HostError::Artifact(ArtifactError::NamespaceBinding))
        ));
    }
    let twice = ExtensionComponents::new()
        .bind(package(EXAMPLE_URI), example)
        .expect("first binding")
        .bind(package(EXAMPLE_URI), example);
    assert!(matches!(
        twice,
        Err(HostError::Artifact(ArtifactError::NamespaceBinding))
    ));
    assert!(matches!(
        ExtensionComponents::new().bind(package(EXAMPLE_URI), component(ArtifactKind::Clock)),
        Err(HostError::Artifact(ArtifactError::ArtifactKindMismatch))
    ));

    // A caller component authenticated by digest, bound to a URI its
    // `namespace-uri` does not answer: the session fails at construction.
    let bytes = &release().example_bytes;
    let digest = TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(bytes)))
        .expect("canonical digest");
    let caller = authenticate_extension_component(bytes, digest).expect("the digest matches");
    assert_eq!(caller.kind(), ArtifactKind::ExtensionComponent);
    let mismatched = host_with(
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
        ExtensionComponents::new()
            .bind(package("https://example.org/not-the-example"), &caller)
            .expect("the binding itself is well-formed"),
    )
    .expect("the verified components link");
    let failure = run_on(&mismatched, &scxml("null", r#"<state id="s0"/>"#), &[])
        .expect_err("the extension component does not implement the bound URI");
    assert!(
        matches!(&failure.failure, RunFailure::Host(HostError::Component(code)) if code == "extension-component-uri-mismatch"),
        "{:?}",
        failure.failure
    );
    let wrong_digest = TrustedComponentDigest::from_hex(&"0".repeat(64)).expect("canonical");
    assert!(matches!(
        authenticate_extension_component(bytes, wrong_digest),
        Err(HostError::Artifact(ArtifactError::ArtifactDigestMismatch))
    ));
}

/// The `<invoke src>` documents of one run: `child.scx`, carrying `bindings`
/// of its own when given (the release it pins).
struct ChildDocument {
    text: String,
    bindings: Option<ExtensionComponents>,
}

impl SessionEmbedder for ChildDocument {
    fn document(&self, kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        (kind == DocumentKind::Invocation && uri == "child.scx").then(|| EmbeddedDocument {
            text: self.text.clone(),
            media_type: None,
            extensions: self.bindings.clone(),
            services: None,
            event_schemas: std::collections::BTreeMap::new(),
            base_uri: None,
        })
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        MachineScripts::none()
    }
}

/// An `<invoke src>` document's own bindings serve its sessions (and the
/// inline `<content>` sessions below them), alongside the host's, which
/// serve every other session: here the host binds no release of the example
/// namespace and the child's own bindings do, so the child (and its inline
/// `<content>` session) runs the element, and the child does not even
/// compile when it carries no bindings of its own.
#[test]
fn an_invoke_src_document_binds_its_own_release_for_its_sessions() {
    let host = host_with(
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
        ExtensionComponents::new(),
    )
    .expect("the verified components link");
    let element = r#"<example:raise event="ran"/>"#;
    let child = |grandchild: &str| {
        let ran = if grandchild.is_empty() {
            r#"<transition target="ok"/>"#.to_owned()
        } else {
            format!(
                r#"{grandchild}<transition event="done.invoke" target="ok"><assign location="result" expr="'ran, then ' + _event.data"/></transition>"#
            )
        };
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="ecmascript" initial="c"><datamodel><data id="result" expr="'ran'"/></datamodel><state id="c"><onentry>{element}</onentry><transition event="ran" target="ran"/><transition event="error.execution" target="refused"/></state><state id="ran">{ran}</state><final id="ok"><donedata><content expr="result"/></donedata></final><final id="refused"><donedata><content expr="'refused'"/></donedata></final></scxml>"#
        )
    };
    let parent = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="ecmascript" initial="p"><state id="p"><transition target="q"/></state><state id="q"><invoke type="scxml" src="child.scx"/><transition event="done.invoke" target="done"><log label="child" expr="_event.data"/></transition><transition event="error.execution" target="done"><log label="child" expr="'failed'"/></transition></state><final id="done"/></scxml>"#
    );
    let run = |child_text: String, bindings: Option<ExtensionComponents>| {
        let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
            SOURCE,
            "parent.scx",
            parent.as_str(),
        ))
        .expect("the parent parses");
        let ir = scxml_compiler_core::lower(&document).expect("the parent lowers");
        let bytes = ir.to_json().expect("the parent's IR encodes");
        let outcome = host
            .run(&SessionRun {
                machine_ir: &bytes,
                data_model: ir.data_model.as_deref(),
                session_id: "root",
                events: &[],
                external_client: None,
                embedder: Rc::new(ChildDocument {
                    text: child_text,
                    bindings,
                }),
                max_commands: 1_024,
                snapshots: SnapshotRequest::none(),
            })
            .expect("the run completes");
        assert_eq!(outcome.configuration, ["done"]);
        texts(&outcome)
    };
    let with_release = || {
        let mut release = package(EXAMPLE_URI);
        release.package_version = "0.3.0".into();
        ExtensionComponents::new()
            .bind(release, component(ArtifactKind::ExtensionExample))
            .expect("the child's release binds")
    };
    // Without bindings of its own the child, compiled against the host's
    // (none), is rejected.
    assert_eq!(run(child(""), None), ["failed"]);
    // The child's own release serves the child.
    assert_eq!(run(child(""), Some(with_release())), ["ran"]);
    // An inline `<content>` session below the child inherits the child's
    // release.
    let grandchild = format!(
        r#"<invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:example="{EXAMPLE_URI}" version="1.0" datamodel="ecmascript" initial="g"><state id="g"><onentry>{element}</onentry><transition event="ran" target="f"/><transition event="error.execution" target="r"/></state><final id="f"><donedata><content expr="'ran'"/></donedata></final><final id="r"><donedata><content expr="'refused'"/></donedata></final></scxml></content></invoke>"#
    );
    assert_eq!(
        run(child(&grandchild), Some(with_release())),
        ["ran, then ran"]
    );
}
