//! The Wasmtime session host runs real documents through the shipped
//! components: the generic machine, the null and ECMAScript data models,
//! the SCXML Event I/O Processor with its clock, the `BasicHTTP` processor
//! over the loopback, and the compiler for invoked child documents.
//!
//! `scripts/verify-wasm-capabilities.sh` builds and verifies the release
//! components into `target/session-host-proof` (this suite's own artifacts
//! directory inside it).

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

use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument, HostError, InvokeLimits,
    LOOPBACK_ORIGIN, MachineScripts, ResolvedScript, RunFailure, ScriptPolicy, ScriptResolveError,
    ScriptResolver, ScriptedEvent, SessionEmbedder, SessionHost, SessionHostComponents,
    SessionOutcome, SessionRun, SnapshotRequest, TelemetryProviders, Terminal, TraceObservation,
    TrustedComponentDigest, TrustedManifestDigest, WasmtimeHostConfig, WasmtimeStoreConfig,
    authenticate_component, basichttp_types, loopback_location,
};
use sha2::{Digest as _, Sha256};

mod valid_document;

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

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

fn components() -> &'static [AuthenticatedComponent] {
    static COMPONENTS_CELL: OnceLock<Vec<AuthenticatedComponent>> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let release = prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace.join("target/session-host-proof");
            // This suite's own artifact directory: suites sharing the Cargo
            // target never rewrite a manifest another suite is reading.
            let release = target.join("wasm-artifacts/session-host");
            let status = Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .arg("--artifacts-dir")
                .arg(&release)
                .args(COMPONENTS.iter().map(|(_, name)| *name))
                .current_dir(workspace)
                .env("CARGO_TARGET_DIR", &target)
                .status()
                .expect("the capability script runs");
            assert!(status.success(), "the release capability proof must pass");
            release
        });
        let manifest = std::fs::read(release.join("manifest.tsv")).expect("manifest is readable");
        let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical");
        COMPONENTS
            .iter()
            .map(|(kind, name)| {
                let path = format!("{name}.virtualized.wasm");
                let bytes = std::fs::read(release.join(&path)).expect("artifact is readable");
                authenticate_component(&manifest, trusted, *kind, &path, &bytes)
                    .expect("artifact authenticates")
            })
            .collect()
    })
}

fn host() -> SessionHost {
    host_with(&WasmtimeHostConfig::default())
}

fn host_with(config: &WasmtimeHostConfig) -> SessionHost {
    let all = components();
    SessionHost::new(
        &SessionHostComponents {
            machine: &all[0],
            compiler: &all[1],
            data_model_ecmascript: &all[2],
            data_model_xpath: &all[3],
            data_model_null: &all[4],
            io_scxml: &all[5],
            io_basichttp: &all[6],
            clock: &all[7],
        },
        config,
        &TelemetryProviders::new(),
    )
    .expect("the verified components link")
}

/// Documents keyed by URI; no scripts.
struct Documents(Vec<(&'static str, String)>);

impl SessionEmbedder for Documents {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        self.0
            .iter()
            .find(|(key, _)| *key == uri)
            .map(|(_, text)| text.clone().into())
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        MachineScripts::none()
    }
}

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "session-host.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn scxml(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="s0">{body}</scxml>"#
    )
}

/// The command budget of a run expected to end on its own, or at a bound
/// the run itself reaches first.
const MAX_COMMANDS: u64 = 100_000;

/// The command budget of [`run`]: enough for any document it runs.
const RUN_COMMANDS: u64 = 256;

fn run_with(
    host: &SessionHost,
    source: &str,
    events: &[String],
    documents: Vec<(&'static str, String)>,
    max_commands: u64,
) -> Result<SessionOutcome, scxml_host_wasmtime::SessionHostError> {
    let (ir, data_model) = lower(source);
    let events: Vec<ScriptedEvent> = events.iter().map(ScriptedEvent::named).collect();
    host.run(&SessionRun {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        events: &events,
        external_client: None,
        embedder: Rc::new(Documents(documents)),
        max_commands,
        snapshots: SnapshotRequest::none(),
    })
}

fn run(source: &str) -> SessionOutcome {
    run_with(&host(), source, &[], Vec::new(), RUN_COMMANDS).expect("the run completes")
}

fn labels(outcome: &SessionOutcome) -> Vec<String> {
    outcome
        .root_logs
        .iter()
        .map(|log| log.label.clone().unwrap_or_default())
        .collect()
}

#[test]
fn delayed_send_is_delivered_on_the_session_clock() {
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><send event="tick" delay="5ms" id="t"/><log label="scheduled"/></onentry><transition event="tick" target="done"><log label="ticked"/></transition></state><final id="done"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(labels(&outcome), ["scheduled", "ticked"]);
    // start, then the delayed event.
    assert_eq!(outcome.commands, 2);
    let commands = outcome
        .trace
        .iter()
        .filter(|entry| entry.observation == TraceObservation::Command)
        .count();
    assert_eq!(commands, 2);
    assert!(outcome.trace.iter().all(|entry| entry.session == "root"));
}

#[test]
fn delivered_events_carry_only_the_authored_send_id() {
    // SCXML C.1: the delivered `_event.sendid` matches the sender's id when
    // the author gave `id` or `idlocation`, and is left empty otherwise,
    // delayed or not.
    let outcome = run(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="loc"/></datamodel><state id="s0"><onentry><send event="plain" delay="1ms"/><send event="located" idlocation="loc"/><send event="named" id="mine" delay="2ms"/></onentry><transition event="plain" cond="_event.sendid === undefined"><log label="plain-empty"/></transition><transition event="located" cond="typeof loc === 'string' &amp;&amp; _event.sendid === loc"><log label="located-matches"/></transition><transition event="named" cond="_event.sendid === 'mine'" target="done"><log label="named-matches"/></transition><transition event="*" target="wrong"/></state><final id="done"/><final id="wrong"/></scxml>"#,
    );
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(
        labels(&outcome),
        ["located-matches", "plain-empty", "named-matches"]
    );
}

/// The session clock has no capacity bound of its own: 1025 pending
/// delayed sends all schedule.
#[test]
fn many_delayed_sends_schedule() {
    let outcome = run_with(
        &host(),
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="items" expr="Array.from({ length: 1025 }, function (_, i) { return i; })"/><data id="sent" expr="0"/></datamodel><state id="s0"><onentry><foreach array="items" item="i"><send event="later" delay="3600s"/><assign location="sent" expr="sent + 1"/></foreach><log label="scheduled" expr="sent"/></onentry><transition event="error.execution" target="done"/></state><final id="done"/></scxml>"#,
        &[],
        Vec::new(),
        MAX_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["s0"]);
    assert_eq!(labels(&outcome), ["scheduled"]);
    assert_eq!(outcome.root_logs[0].text, "1025");
}

/// The session clock has no timer identifier bound of its own: a delayed
/// send whose id is longer than 256 bytes schedules.
#[test]
fn a_delayed_send_with_a_long_id_schedules() {
    let outcome = run(&scxml(&format!(
        r#"<state id="s0"><onentry><send event="later" delay="3600s" id="{}"/><log label="scheduled"/></onentry><transition event="error.execution" target="done"/></state><final id="done"/>"#,
        "i".repeat(512)
    )));
    assert_eq!(outcome.configuration, ["s0"]);
    assert_eq!(labels(&outcome), ["scheduled"]);
}

/// The ECMAScript data model has no expression byte bound of its own: an
/// expression longer than 16 KiB evaluates.
#[test]
fn a_long_ecmascript_expression_evaluates() {
    let outcome = run(&format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><onentry><log label="value" expr="1{}"/></onentry><transition event="error.execution" target="done"><log label="declined"/></transition></state><final id="done"/></scxml>"#,
        " ".repeat(65_536)
    ));
    assert_eq!(labels(&outcome), ["value"]);
}

/// The ECMAScript data model bounds neither a script's loop iterations nor
/// its value stack: the host's fuel and memory bound them. A script that
/// loops 200,000 times, or calls a function with 20,000 arguments,
/// completes.
#[test]
fn a_long_running_ecmascript_script_completes() {
    let source = |script: &str| {
        format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><onentry><script>{script}</script><log label="completed"/></onentry><transition event="error.execution" target="done"><log label="declined"/></transition></state><final id="done"/></scxml>"#
        )
    };
    // An error ends the `<script>`'s block (SCXML 4.9), so `completed` is
    // logged only when the script finishes.
    let looping = labels(&run(&source("var i = 0; while (i &lt; 100001) { i++; }")));
    assert_eq!(looping, ["completed"]);
    let wide = run(&source(&format!(
        "(function () {{ return arguments.length; }})({});",
        vec!["0"; 20_000].join(",")
    )));
    assert_eq!(labels(&wide), ["completed"]);
}

/// The ECMAScript data model answers `In()` about any number of distinct
/// state ids in one operation.
#[test]
fn a_condition_asking_in_about_many_distinct_ids_is_evaluated() {
    let outcome = run(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><transition cond="(function () { for (var i = 0; i &lt; 1000; i++) In('q' + i); return true; })()" target="taken"/><transition event="error.execution" target="declined"/></state><final id="taken"/><final id="declined"/></scxml>"#,
    );
    assert_eq!(outcome.configuration, ["taken"]);
}

/// Inline `<invoke><content>` is serialized into the child document the
/// compiler receives, which the machine bounds at 1 MiB in a wasm build.
/// Content that serializes past it fails that `<invoke>`, raising
/// `error.execution` (SCXML 3.12.2: the processor "MUST signal any errors
/// that occur by raising SCXML events whose names begin with 'error.'");
/// the session goes on.
#[test]
fn inline_invoke_content_over_the_machine_bound_raises_error_execution() {
    let text = "x".repeat(600_000);
    let outcome = run(&scxml(&format!(
        r#"<state id="s0"><invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c"><datamodel><data id="a">{text}</data><data id="b">{text}</data></datamodel><state id="c"/></scxml></content></invoke><transition event="error.execution" target="declined"/></state><final id="declined"/>"#
    )));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["declined"]);
}

/// SCXML 6.4: inline `<content>` is the child's document. Its text and
/// attribute values reach the child as the parser decoded them: `&lt;` in a
/// `<script>` and `&amp;` in a `<data>` value read back unchanged, and a
/// tab, line feed, and carriage return written as character references in
/// an attribute value (which XML attribute-value normalization would
/// otherwise turn into spaces) stay in the child's `<log label>`.
#[test]
fn inline_invoke_content_keeps_escaped_text_and_attribute_values() {
    let outcome = run(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><invoke><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><datamodel><data id="d">x &amp; y</data></datamodel><script>var lt = 1 &lt; 2;</script><state id="c"><onentry><log label="a&#9;b&#10;c&#13;d &gt; e"/><if cond="lt === true &amp;&amp; d === 'x &amp; y'"><send target="#_parent" event="intact"/><else/><send target="#_parent" event="altered"/></if></onentry></state></scxml></content></invoke><transition event="intact" target="pass"/><transition event="*" target="fail"/></state><final id="pass"/><final id="fail"/></scxml>"##,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    let child_labels: Vec<_> = outcome
        .trace
        .iter()
        .filter_map(|entry| match &entry.observation {
            TraceObservation::Log(log) if entry.session != "root" => log.label.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(child_labels, ["a\tb\nc\rd > e"]);
}

/// The null data model has no condition byte bound of its own: an
/// `In()` condition padded past 16 KiB is evaluated.
#[test]
fn a_long_null_condition_is_evaluated() {
    let outcome = run(&scxml(&format!(
        r#"<state id="s0"><transition cond="In('s0'){}" target="taken"/><transition event="error.execution" target="declined"/></state><final id="taken"/><final id="declined"/>"#,
        " ".repeat(65_536)
    )));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["taken"]);
}

/// Every other null data-model evaluation is answered `execution`, whatever
/// its length (SCXML B.1: no value, location, or script language): its
/// block ends with `error.execution` (SCXML 5.9.2, 5.9.3, 4.6, 5.4, 5.8) and
/// the session goes on.
#[test]
fn a_null_evaluation_of_any_length_raises_error_execution() {
    // Each template's `TEXT` is replaced by the input.
    let fragments = [
        ("log expr", r#"<log label="value" expr="TEXT"/>"#),
        ("send eventexpr", r#"<send eventexpr="TEXT"/>"#),
        (
            "param expr",
            r#"<send event="e"><param name="p" expr="TEXT"/></send>"#,
        ),
        (
            "param location",
            r#"<send event="e"><param name="p" location="TEXT"/></send>"#,
        ),
        ("foreach array", r#"<foreach array="TEXT" item="i"/>"#),
        ("assign location", r#"<assign location="TEXT" expr="1"/>"#),
        ("assign expr", r#"<assign location="v" expr="TEXT"/>"#),
        ("script", "<script>TEXT</script>"),
    ];
    for (name, template) in fragments {
        for length in [1, 65_537] {
            let outcome = run(&scxml(&format!(
                r#"<state id="s0"><onentry>{}<log label="unreached"/></onentry><transition event="error.execution" target="done"><log label="declined"/></transition></state><final id="done"/>"#,
                template.replace("TEXT", &"x".repeat(length))
            )));
            assert_eq!(
                outcome.terminal,
                Terminal::Finished,
                "{name} of {length} bytes"
            );
            assert_eq!(outcome.configuration, ["done"], "{name} of {length} bytes");
            assert_eq!(labels(&outcome), ["declined"], "{name} of {length} bytes");
        }
    }
}

/// SCXML 4.6: "If 'array' does not evaluate to a legal iterable collection
/// … the SCXML processor MUST terminate execution of the <foreach> element
/// and the block that contains it, and place the error error.execution on
/// the internal event queue." In the ECMAScript data model only an
/// `instanceof(Array)` value is a legal collection (B.2.11); an XML `<data>`
/// value (a DOM document) is not, so its block ends with `error.execution`
/// and the session goes on.
#[test]
fn an_ecmascript_foreach_over_an_xml_value_raises_error_execution() {
    let outcome = run(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="doc"><items><item/><item/></items></data></datamodel><state id="s0"><onentry><foreach array="doc" item="i"><log label="iterated"/></foreach><log label="unreached"/></onentry><transition event="error.execution" target="done"><log label="declined"/></transition></state><final id="done"/></scxml>"#,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(labels(&outcome), ["declined"]);
}

/// SCXML 3.2.1 gives `datamodel` the default "platform-specific"; this
/// project's default is ECMAScript. A document that names no data model
/// declares data, evaluates ECMAScript, and answers `In()` through the
/// ECMAScript data model the host links from its Machine IR.
#[test]
fn a_document_without_datamodel_runs_under_the_ecmascript_default() {
    let (_, data_model) = lower(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s0"><state id="s0"/></scxml>"#,
    );
    assert_eq!(data_model.as_deref(), Some("ecmascript"));
    let outcome = run(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s0"><datamodel><data id="x" expr="[1, 2].length"/></datamodel><state id="s0"><transition cond="x === 2 &amp;&amp; In('s0')" target="done"><log label="default" expr="typeof x"/></transition></state><final id="done"/></scxml>"#,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(outcome.root_logs[0].text, "number");
}

#[test]
fn cancelled_delayed_send_is_never_delivered() {
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><send event="late" delay="5ms" id="t"/><send event="early" delay="1ms"/><cancel sendid="t"/></onentry><transition event="late" target="bad"/><transition event="early" target="s1"/></state><state id="s1"><transition event="late" target="bad"/></state><final id="bad"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Running);
    assert_eq!(outcome.configuration, ["s1"]);
}

#[test]
fn immediate_self_send_is_enqueued_after_raised_events() {
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><send event="external"/><raise event="internal"/></onentry><transition event="internal" target="s1"><log label="internal"/></transition></state><state id="s1"><transition event="external" target="done"><log label="external"/></transition></state><final id="done"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["internal", "external"]);
    assert!(
        outcome
            .trace
            .iter()
            .any(|entry| { entry.observation == TraceObservation::Raised("internal".into()) })
    );
    // Both events are processed by transition selection, internal first.
    let processed: Vec<&str> = outcome
        .trace
        .iter()
        .filter_map(|entry| match &entry.observation {
            TraceObservation::Processed(event) => Some(event.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(processed, ["internal", "external"]);
}

#[test]
fn scripted_events_and_the_shared_budget_bound_the_run() {
    let source = scxml(
        r#"<state id="s0"><transition event="go" target="s1"/></state><state id="s1"><transition event="go" target="s0"/></state>"#,
    );
    let host = host();
    let events = vec!["go".to_owned(); 3];
    let outcome = run_with(&host, &source, &events, Vec::new(), 16).expect("within budget");
    assert_eq!(outcome.configuration, ["s1"]);
    assert_eq!(outcome.commands, 4);
    let exhausted = run_with(&host, &source, &events, Vec::new(), 3)
        .expect_err("a budget below the commands fails");
    assert!(matches!(exhausted.failure, RunFailure::CommandBudget));
    assert_eq!(exhausted.commands, 3);
    let invalid = run_with(&host, &source, &["not a name".to_owned()], Vec::new(), 16)
        .expect_err("an invalid scripted event name is a harness failure");
    assert!(matches!(
        invalid.failure,
        RunFailure::Harness {
            code: "event-name-invalid",
            ..
        }
    ));
    let zero = run_with(&host, &source, &events, Vec::new(), 0)
        .expect_err("a zero command budget is rejected before any call");
    assert!(
        matches!(
            &zero.failure,
            RunFailure::Harness { code: "command-budget-invalid", message }
                if message == "max_commands must be greater than zero"
        ),
        "the violation names the `SessionRun::max_commands` field: {:?}",
        zero.failure
    );
    assert_eq!(zero.commands, 0);
}

#[test]
fn unknown_session_targets_raise_error_communication() {
    let outcome = run(&scxml(
        r##"<state id="s0"><onentry><send event="lost" target="#_scxml_nobody"/></onentry><transition event="error.communication" target="done"><log label="communication"/></transition></state><final id="done"/>"##,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["communication"]);
}

#[test]
fn basichttp_self_post_arrives_through_the_loopback() {
    let location = loopback_location("root");
    let outcome = run(&scxml(&format!(
        r#"<state id="s0"><onentry><send event="posted" type="basichttp" target="{location}"/></onentry><transition event="posted" target="done"><log label="posted"/></transition></state><final id="done"/>"#
    )));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["posted"]);
    let unreachable = run(&scxml(&format!(
        r#"<state id="s0"><onentry><send event="posted" type="basichttp" target="{LOOPBACK_ORIGIN}/sessions/nobody/basichttp"/></onentry><transition event="error.communication" target="done"/></state><final id="done"/>"#
    )));
    assert_eq!(unreachable.terminal, Terminal::Finished);
}

#[test]
fn external_client_post_reaches_the_root_session() {
    let host = host();
    let (ir, data_model) = lower(&scxml(
        r#"<state id="s0"><transition event="knock" target="done"><log label="knocked"/></transition></state><final id="done"/>"#,
    ));
    let client = |location: &str| basichttp_types::HttpRequest {
        method: "POST".into(),
        uri: location.to_owned(),
        headers: vec![basichttp_types::HttpHeader {
            name: "_scxmleventname".into(),
            value: "knock".into(),
        }],
        body: Vec::new(),
    };
    let outcome = host
        .run(&SessionRun {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: Some(&client),
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 64,
            snapshots: SnapshotRequest::none(),
        })
        .expect("the external-client run completes");
    assert_eq!(
        outcome
            .http_response
            .as_ref()
            .map(|response| response.status),
        Some(200)
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["knocked"]);
}

const CHILD: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="hello" target="#_parent"/><log label="child"/></onentry><transition event="stop" target="cdone"/></state><final id="cdone"/></scxml>"##;

#[test]
fn invoked_child_runs_in_its_own_session_and_reports_done() {
    let outcome = run_with(
        &host(),
        &scxml(
            r##"<state id="s0" initial="a"><invoke id="kid" type="scxml" src="child.scxml"/><state id="a"><transition event="hello" target="b"><log label="hello"/></transition></state><state id="b"><onentry><send event="stop" target="#_kid"/></onentry></state><transition event="done.invoke.kid" target="done"><log label="done"/></transition></state><final id="done"/>"##,
        ),
        &[],
        vec![("child.scxml", CHILD.to_owned())],
        256,
    )
    .expect("the parent and its child complete");
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["hello", "done"]);
    let invoked_session = outcome
        .trace
        .iter()
        .find(|entry| entry.parent.as_deref() == Some("root"))
        .expect("the invoked session is traced")
        .session
        .clone();
    assert!(outcome.trace.iter().any(|entry| {
        entry.session == invoked_session
            && matches!(&entry.observation, TraceObservation::Log(log) if log.label.as_deref() == Some("child"))
    }));
}

#[test]
fn unresolvable_child_document_fails_the_invocation_only() {
    let outcome = run(&scxml(
        r#"<state id="s0"><invoke id="kid" type="scxml" src="missing.scxml"/><transition event="error.execution" target="done"><log label="failed"/></transition></state><final id="done"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["failed"]);
}

const WAITING_CHILD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"/></scxml>"#;

/// An invocation beyond the embedder's invoke limits cannot start: the
/// invoking session sees `error.execution` for that `<invoke>` and keeps
/// running (SCXML 6.4), and the sessions within the limits run.
#[test]
fn an_invocation_beyond_the_embedder_invoke_limits_raises_error_execution() {
    let host = host()
        .with_invoke_limits(InvokeLimits {
            max_invoke_depth: Some(4),
            max_invoked_sessions: Some(2),
        })
        .expect("an unshared host");
    let outcome = run_with(
        &host,
        &scxml(
            r#"<state id="s0"><invoke id="k0" type="scxml" src="child.scxml"/><invoke id="k1" type="scxml" src="child.scxml"/><invoke id="k2" type="scxml" src="child.scxml"/><transition event="error.execution" target="done"><log label="bounded"/></transition></state><final id="done"/>"#,
        ),
        &[],
        vec![("child.scxml", WAITING_CHILD.to_owned())],
        256,
    )
    .expect("the run completes");
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(labels(&outcome), ["bounded"]);
}

/// Invoked sessions (with `_parent`) of `root`, by `_sessionid`.
fn invoked_sessions(outcome: &SessionOutcome) -> std::collections::BTreeSet<&str> {
    outcome
        .trace
        .iter()
        .filter(|entry| entry.parent.as_deref() == Some("root"))
        .map(|entry| entry.session.as_str())
        .collect()
}

/// The native host imposes no invoke limit of its own: by default more
/// than 255 invoked sessions run at once under one root session.
#[test]
fn more_than_255_invoked_sessions_run_by_default() {
    let mut invokes = String::new();
    for index in 0..256 {
        write!(
            invokes,
            r#"<invoke id="k{index}" type="scxml" src="child.scxml"/>"#
        )
        .expect("writing to a String cannot fail");
    }
    let outcome = run_with(
        &host(),
        &scxml(&format!(
            r#"<state id="s0"><onentry><send event="check" delay="1ms"/></onentry>{invokes}<transition event="error.execution" target="bounded"/><transition event="check" target="done"/></state><final id="done"/><final id="bounded"/>"#
        )),
        &[],
        vec![("child.scxml", WAITING_CHILD.to_owned())],
        MAX_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(invoked_sessions(&outcome).len(), 256);
}

/// The native host imposes no invoke-depth limit of its own: by default a
/// chain of sessions 40 levels deep runs, and `done.invoke` climbs back to
/// the root.
#[test]
fn invocations_deeper_than_32_levels_run_by_default() {
    const DEPTH: usize = 40;
    let mut documents = Vec::new();
    for level in 1..=DEPTH {
        let body = if level == DEPTH {
            r#"<final id="c0"/>"#.to_owned()
        } else {
            format!(
                r#"<state id="c0"><invoke id="k" type="scxml" src="level{}.scxml"/><transition event="done.invoke.k" target="cdone"/></state><final id="cdone"/>"#,
                level + 1
            )
        };
        let uri: &'static str = Box::leak(format!("level{level}.scxml").into_boxed_str());
        documents.push((
            uri,
            format!(
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0">{body}</scxml>"#
            ),
        ));
    }
    let outcome = run_with(
        &host(),
        &scxml(
            r#"<state id="s0"><invoke id="k" type="scxml" src="level1.scxml"/><transition event="done.invoke.k" target="done"><log label="deep"/></transition><transition event="error.execution" target="bounded"/></state><final id="done"/><final id="bounded"/>"#,
        ),
        &[],
        documents,
        MAX_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
    assert_eq!(labels(&outcome), ["deep"]);
    let sessions: std::collections::BTreeSet<&str> = outcome
        .trace
        .iter()
        .map(|entry| entry.session.as_str())
        .collect();
    assert_eq!(sessions.len(), 1 + DEPTH, "the root and every level");
}

/// The native host bounds neither scripted events, `<log>` records, nor
/// trace records: 32,769 scripted events each logging run to the end, with
/// every log and every trace observation kept.
#[test]
fn scripted_events_logs_and_trace_have_no_host_imposed_bound() {
    const EVENTS: usize = 32_769;
    let events = vec!["e".to_owned(); EVENTS];
    let outcome = run_with(
        &host(),
        &scxml(r#"<state id="s0"><transition event="e"><log label="e"/></transition></state>"#),
        &events,
        Vec::new(),
        MAX_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.terminal, Terminal::Running);
    assert_eq!(outcome.root_logs.len(), EVENTS);
    // `start`, then per event one command, the processed event, and its
    // log: over 65,536 records, every one kept.
    let count = |kind: fn(&TraceObservation) -> bool| {
        outcome
            .trace
            .iter()
            .filter(|entry| kind(&entry.observation))
            .count()
    };
    assert_eq!(
        count(|observation| *observation == TraceObservation::Command),
        1 + EVENTS
    );
    assert_eq!(
        count(|observation| *observation == TraceObservation::Processed("e".into())),
        EVENTS
    );
    assert_eq!(
        count(|observation| matches!(observation, TraceObservation::Log(_))),
        EVENTS
    );
    assert_eq!(outcome.trace.len(), 1 + 3 * EVENTS);
}

/// The native host serves a loaded document of any size: a `<data src>`
/// document over 4 MiB reaches the machine (SCXML 5.3), which reads it as
/// whitespace-normalized text (B.2.1).
#[test]
fn a_loaded_document_over_4_mib_is_served() {
    let text = format!("x{}y", " ".repeat(4 * 1024 * 1024));
    let outcome = run_with(
        &host(),
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="d" src="big.txt"/></datamodel><state id="s0"><transition cond="d === 'x y'" target="done"/><transition event="error.execution" target="failed"/></state><final id="done"/><final id="failed"/></scxml>"#,
        &[],
        vec![("big.txt", text)],
        256,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"]);
}

/// A document whose only data-model use is the executable content of a
/// compound state's `<initial>` transition (SCXML 3.6, 5.9) initializes the
/// ECMAScript data model and runs.
#[test]
fn nested_initial_transition_content_alone_links_the_data_model() {
    let outcome = run(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="p"><state id="p"><initial><transition target="a"><log label="initial" expr="1 + 1"/></transition></initial><state id="a"/></state></scxml>"#,
    );
    assert_eq!(outcome.terminal, Terminal::Running);
    assert_eq!(outcome.configuration, ["a"]);
    assert_eq!(labels(&outcome), ["initial"]);
}

/// A valid 16-region parallel state whose regions all transition on every
/// event costs the shipped machine about 0.7 million fuel per event (linear
/// in the region count); it runs under the default budget.
#[test]
fn wide_parallel_toggle_runs_under_the_default_fuel() {
    let mut regions = String::new();
    for region in 0..16 {
        write!(
            regions,
            r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
        )
        .expect("writing to a string succeeds");
    }
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">{regions}</parallel></scxml>"#
    );
    let events = vec!["t".to_owned(); 3];
    let outcome = run_with(&host(), &source, &events, Vec::new(), 16)
        .expect("the parallel toggle runs under the default fuel");
    assert_eq!(outcome.terminal, Terminal::Running);
    assert_eq!(outcome.commands, 4);
    let toggled: Vec<String> = (0..16).map(|region| format!("r{region}b")).collect();
    assert_eq!(outcome.configuration, toggled);
}

/// The largest documented ECMAScript workload runs in one call under the
/// default fuel: a `<foreach>` with an `<assign>` over 1 000 items
/// (`DEFAULT_FUEL` sizing, measured by `tests/fuel_sizing.rs`).
#[test]
fn thousand_item_foreach_runs_under_the_default_fuel() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="arr" expr="(function(){var r=[];for(var i=0;i&lt;1000;i++)r.push(i);return r})()"/><data id="acc" expr="0"/></datamodel><state id="s0"><onentry><foreach array="arr" item="it"><assign location="acc" expr="acc + it"/></foreach><log label="sum" expr="acc"/></onentry></state></scxml>"#;
    let outcome = run_with(&host(), source, &[], Vec::new(), 16)
        .expect("the 1 000-item foreach runs under the default fuel");
    assert_eq!(outcome.terminal, Terminal::Running);
    let sum = outcome
        .root_logs
        .iter()
        .find(|log| log.label.as_deref() == Some("sum"))
        .expect("the sum is logged");
    assert!(format!("{sum:?}").contains("499500"), "{sum:?}");
}

/// Regions of the wide eventless runaway: wide enough that it traps out of
/// fuel at about half the machine's per-call command bound (narrower
/// machines, 64 regions among them, reach the bound first).
const RUNAWAY_REGIONS: usize = 128;

/// A wide machine that never reaches a stable configuration exhausts the default fuel before the
/// machine's per-call command bound: 128 regions that all toggle on every
/// eventless microstep trap out of fuel inside the first call, at the same
/// point on every run.
#[test]
fn wide_eventless_runaway_exhausts_the_default_fuel() {
    let mut regions = String::new();
    for region in 0..RUNAWAY_REGIONS {
        write!(
            regions,
            r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition target="r{region}b"/></state><state id="r{region}b"><transition target="r{region}a"/></state></state>"#
        )
        .expect("writing to a string succeeds");
    }
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="wide"><parallel id="wide">{regions}</parallel></scxml>"#
    );
    let host = host();
    let trapped: Vec<u64> = (0..2)
        .map(|_| {
            let error = run_with(&host, &source, &[], Vec::new(), MAX_COMMANDS)
                .expect_err("the wide runaway never reaches a stable configuration");
            assert!(
                matches!(
                    &error.failure,
                    RunFailure::Host(HostError::Wasmtime(trap))
                        if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
                ),
                "{:?}",
                error.failure
            );
            error.commands
        })
        .collect();
    assert_eq!(trapped[0], trapped[1], "fuel runs out at the same command");
}

/// A document whose `<script>` keeps adding 60 KB strings grows the data
/// model's memory a little at a time toward the store's 64 MiB. Every
/// growth moves and copies the whole memory in the host and is taken from
/// the command's copy allowance
/// (`memory_growth_copies_are_bounded_by_the_calls_copy_allowance` in
/// `data_model_component.rs` measures the copies), so the command ends in a
/// fuel trap after at most three billion bytes of copying (its default
/// fuel's allowance), before the memory reaches the store's bound.
#[test]
fn a_document_that_grows_memory_in_small_steps_runs_out_of_fuel() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><onentry><script>var s = 'x'.repeat(60000); var a = []; for (var i = 0; i &lt; 5000; i++) a.push(s + i);</script></onentry><transition event="error.*" target="f"/></state><final id="f"/></scxml>"#;
    let error = run_with(&host(), source, &[], Vec::new(), MAX_COMMANDS)
        .expect_err("the growth exhausts the command's fuel");
    assert!(
        matches!(
            &error.failure,
            RunFailure::Host(HostError::Wasmtime(trap))
                if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
        ),
        "{:?}",
        error.failure
    );
}

/// States in the inline child of the copy allowance proof.
const LARGE_CHILD_STATES: usize = 4_000;

/// A 4,000-state inline child (about 270 KB) of an ECMAScript root, a
/// document `compile` accepts, starts on the default fuel: the copies its
/// compile and start make as their memories grow come out of each
/// command's copy allowance, not its Wasm fuel.
#[test]
fn a_large_inline_child_starts_on_the_default_fuel() {
    let mut states = String::new();
    for state in 0..LARGE_CHILD_STATES {
        let next = (state + 1) % LARGE_CHILD_STATES;
        write!(
            states,
            r#"<state id="c{state}"><transition event="e{state}" target="c{next}"/></state>"#
        )
        .expect("writing to a string succeeds");
    }
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><state id="s0"><invoke type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="c0">{states}</scxml></content></invoke><transition event="error.*" target="f"/></state><final id="f"/></scxml>"#
    );
    valid_document::assert_valid("large-inline-child.scxml", &source);
    let outcome = run_with(&host(), &source, &[], Vec::new(), MAX_COMMANDS)
        .expect("the child starts within the command's fuel and copy allowance");
    assert_eq!(outcome.configuration, ["s0"], "the invocation did not fail");
}

/// Fuel bounds the Wasm work of one call; the machine's per-call command
/// bound stops logical runaways. Under the default budget an eventless
/// transition loop and a 0 ms self-send chain stop at the command bound,
/// identically on every run; under a starved budget the same eventless loop
/// exhausts fuel instead (a Wasmtime out-of-fuel trap), also identically.
#[test]
fn runaway_machines_stop_deterministically() {
    let eventless = scxml(
        r#"<state id="s0"><transition target="s1"/></state><state id="s1"><transition target="s0"/></state>"#,
    );
    let self_send = scxml(
        r#"<state id="s0"><onentry><send event="again" delay="0ms"/></onentry><transition event="again" target="s0"/></state>"#,
    );
    let host = host();
    for source in [&eventless, &self_send] {
        let runs: Vec<u64> = (0..2)
            .map(|_| {
                let error = run_with(&host, source, &[], Vec::new(), MAX_COMMANDS)
                    .expect_err("a runaway never reaches a stable configuration");
                assert!(
                    matches!(error.failure, RunFailure::CommandBound),
                    "{:?}",
                    error.failure
                );
                error.commands
            })
            .collect();
        assert_eq!(runs[0], runs[1], "the runaway stops at the same point");
    }

    let starved = host_with(&WasmtimeHostConfig {
        store: WasmtimeStoreConfig {
            fuel: 10_000_000,
            ..WasmtimeStoreConfig::default()
        },
        ..WasmtimeHostConfig::default()
    });
    let trapped: Vec<u64> = (0..2)
        .map(|_| {
            let error = run_with(&starved, &eventless, &[], Vec::new(), MAX_COMMANDS)
                .expect_err("the starved budget traps");
            assert!(
                matches!(
                    &error.failure,
                    RunFailure::Host(HostError::Wasmtime(trap))
                        if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
                ),
                "{:?}",
                error.failure
            );
            error.commands
        })
        .collect();
    assert_eq!(trapped[0], trapped[1], "fuel runs out at the same command");
}

/// One host call is bounded by one fuel budget across every component it
/// reaches: the data-model calls a machine call makes are charged to the
/// machine call's budget, not given a fresh budget each. Ten `<foreach>`
/// loops over 200 items, each running a heavy `<script>`, in one `<onentry>`
/// exhaust the default budget inside the first call (they ran for minutes
/// when every data-model call was refueled). The proof is deterministic: a
/// fresh budget per data-model call would never trap on fuel here, and the
/// trap lands in the first call on every run.
#[test]
fn nested_data_model_work_is_charged_to_the_machine_call_budget() {
    let loops = r#"<foreach array="arr" item="it"><script>for(var j=0;j&lt;5;j++){acc+=burn()}</script></foreach>"#
        .repeat(10);
    let source = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0"><datamodel><data id="arr" expr="(function(){{var r=[];for(var i=0;i&lt;200;i++)r.push(i);return r}})()"/><data id="acc" expr="0"/></datamodel><script>function burn(){{var x=0;for(var i=0;i&lt;99000;i++){{x+=i}}return x}}</script><state id="s0"><onentry>{loops}<log label="done"/></onentry></state></scxml>"#
    );
    let trapped: Vec<u64> = (0..2)
        .map(|_| {
            let error = run_with(&host(), &source, &[], Vec::new(), 16)
                .expect_err("the nested work exceeds one call's budget");
            assert!(
                matches!(
                    &error.failure,
                    RunFailure::Host(HostError::Wasmtime(trap))
                        if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
                ),
                "{:?}",
                error.failure
            );
            error.commands
        })
        .collect();
    assert!(trapped[0] <= 1, "the first call (start) runs out");
    assert_eq!(trapped[0], trapped[1], "fuel runs out at the same command");
}

/// An ECMAScript document whose initial state is `s0`.
fn ecmascript(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0">{body}</scxml>"#
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

#[test]
fn an_ignored_internal_event_reselects_eventless_transitions_with_it_bound() {
    // W3C Appendix D `mainEventLoop`: when the dequeued internal event
    // enables no transition, the loop runs `selectEventlessTransitions`
    // again with that event bound as `_event`. Only that re-selection
    // reaches `done`: the eventless guard holds only while `_event` is the
    // ignored event, and nothing else ever runs.
    let outcome = run(&ecmascript(
        r#"<state id="s0"><onentry><raise event="ignored"/></onentry><transition cond="typeof _event !== 'undefined' &amp;&amp; _event.name === 'ignored'" target="done"><log label="reselected" expr="_event.name"/></transition></state><final id="done"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished, "{outcome:?}");
    assert_eq!(records(&outcome), [("reselected".into(), "ignored".into())]);
}

#[test]
fn internal_sends_and_raises_share_one_fifo_internal_queue() {
    // SCXML 6.2 (C.1): "If the target is the special term '#_internal',
    // the Processor MUST add the event to the internal event queue of the
    // sending session", the queue `<raise>` also appends to (3.14,
    // Appendix D `internalQueue.enqueue`), so interleaved raises and
    // internal sends are processed in document order, all typed
    // `internal` (5.10.1).
    let outcome = run(&ecmascript(
        r##"<state id="s0"><onentry><raise event="r1"/><send target="#_internal" event="i1"/><raise event="r2"/><send target="#_internal" event="i2"/></onentry><transition event="r1 i1 r2 i2"><log label="event" expr="_event.name + ':' + _event.type"/></transition></state>"##,
    ));
    assert_eq!(outcome.terminal, Terminal::Running);
    assert_eq!(
        records(&outcome),
        [
            ("event".into(), "r1:internal".into()),
            ("event".into(), "i1:internal".into()),
            ("event".into(), "r2:internal".into()),
            ("event".into(), "i2:internal".into()),
        ]
    );
}

#[test]
fn late_binding_values_state_data_on_first_entry_and_not_before() {
    // SCXML 5.3: with `binding="late"` the processor "MUST create the data
    // elements at initialization time but MUST assign the specified
    // initial value to a given data element only when the state that
    // contains it is entered for the first time, before any <onentry>
    // markup". Official case 280 compares two variables that are both
    // undefined when binding never happens; here s0 requires `Var2` to be
    // declared but unvalued (so early valuing fails) and s1's onentry
    // requires its value (so never valuing fails).
    let outcome = run(&ecmascript(
        r#"<state id="s0"><transition cond="typeof Var2 === 'undefined'" target="s1"><log label="s0" expr="typeof Var2"/></transition><transition target="fail"/></state><state id="s1"><datamodel><data id="Var2" expr="1"/></datamodel><onentry><log label="s1" expr="Var2"/></onentry><transition cond="Var2 === 1" target="done"/><transition target="fail"/></state><final id="done"/><final id="fail"/>"#,
    ).replace("<scxml ", r#"<scxml binding="late" "#));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("s0".into(), "undefined".into()), ("s1".into(), "1".into())]
    );
}

#[test]
fn a_non_2xx_basichttp_answer_raises_error_communication_with_the_send_id() {
    // SCXML C.2.1: a receiver that adds the message to its queue "MUST
    // then indicate the result ... via a success response code 2XX"; any
    // other answer means the message was not dispatched, so the sender
    // raises error.communication (6.2.4: "If it is unable to dispatch the
    // message, the Processor MUST place the error error.communication on
    // the internal event queue") with the failed send's id (5.10.1). The
    // loopback receiver answers 404 for a location no session owns.
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><onentry><send id="post" event="posted" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="{LOOPBACK_ORIGIN}/sessions/nobody/basichttp"/></onentry><transition event="error.communication" target="done"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("error".into(), "post:platform".into())]
    );
}

#[test]
fn inbound_basichttp_events_carry_the_basichttp_origin_type() {
    // SCXML 5.10.1: for external events the processor SHOULD set
    // `origintype` to a value that, used as `<send type>`, sends a
    // response back; for a message received by the Basic HTTP Event I/O
    // Processor (C.2.1) that is its type URI.
    let location = loopback_location("root");
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><onentry><send event="posted" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="{location}"/></onentry><transition event="posted" target="done"><log label="origintype" expr="_event.origintype"/><log label="type" expr="_event.type"/></transition></state><final id="done"/>"#
    )));
    assert_eq!(outcome.terminal, Terminal::Finished, "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [
            (
                "origintype".into(),
                "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor".into()
            ),
            ("type".into(), "external".into())
        ]
    );
}

#[test]
fn a_delayed_send_whose_target_is_gone_at_dispatch_raises_error_communication() {
    // SCXML 6.2: the Processor "MUST dispatch the message only when the
    // delay interval elapses"; 6.2.4: "If it is unable to dispatch the
    // message, the Processor MUST place the error error.communication";
    // C.1: a session that "does not exist or is inaccessible" is such a
    // case. The child `c` finishes before the delayed send to `#_c`
    // falls due, so the sender gets error.communication carrying the
    // send's id (5.10.1), typed platform, on its internal event queue
    // (3.12.2, 6.2.4; the failure is known only after the sending
    // macrostep, as for a late BasicHTTP failure).
    let outcome = run(&ecmascript(
        r##"<state id="s0"><invoke id="c"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="ready" target="#_parent"/></onentry><transition event="stop" target="cf"/></state><final id="cf"/></scxml></content></invoke><transition event="ready"><send id="late" event="late" target="#_c" delay="10ms"/><send event="stop" target="#_c"/></transition><transition event="done.invoke.c"><log label="child-done"/></transition><transition event="error.communication" target="done"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"##,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [
            ("child-done".into(), String::new()),
            ("error".into(), "late:platform".into())
        ]
    );
}

#[test]
fn a_delayed_send_to_an_unknown_session_fails_when_its_delay_elapses() {
    // Every session of the run is accessible (C.1: "The set of SCXML
    // sessions that are accessible to a given SCXML Processor is
    // platform-dependent"), and which sessions run is known only when the
    // delayed send falls due (6.2: "It MUST dispatch the message only when
    // the delay interval elapses"). Then no session answers `nobody`: "If
    // the sending SCXML session specifies a session that does not exist or
    // is inaccessible, the SCXML Processor MUST place the error
    // error.communication on the internal event queue of the sending
    // session", with the send's id, after the raised `after`.
    let outcome = run(&ecmascript(
        r##"<state id="s0"><onentry><send id="lost" event="lost" target="#_scxml_nobody" delay="10ms"/><raise event="after"/></onentry><transition event="after"><log label="after"/></transition><transition event="error.communication" target="done"><log label="error" expr="_event.sendid"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"##,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [
            ("after".into(), String::new()),
            ("error".into(), "lost".into())
        ]
    );
}

#[test]
fn a_delayed_internal_send_raises_error_execution() {
    // SCXML 6.2.1: 'delay' and 'delayexpr' "Must not occur ... when the
    // attribute 'target' has the value "_internal"" (the special target
    // C.1 names '#_internal'); a document that breaks that constraint is
    // non-conformant (Appendix A). The processor treats the internal
    // target as not supported for a delayed send: 6.2.4, "If the value of
    // the 'target' or 'targetexpr' attribute is not supported or invalid,
    // the Processor MUST place the error error.execution on the internal
    // event queue". Nothing is delivered, now or later.
    let outcome = run(&ecmascript(
        r#"<state id="s0"><onentry><send id="soon" event="soon" targetexpr="'#_internal'" delayexpr="'10ms'"/></onentry><transition event="error.execution" target="s1"><log label="error" expr="_event.sendid"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><onentry><send event="settled" delay="20ms"/></onentry><transition event="settled" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("error".into(), "soon".into())]);
}

#[test]
fn a_state_listed_twice_in_a_target_is_entered_once() {
    // SCXML 3.11: "we therefore define a legal state specification to be a
    // set of states such that 1) no state is an ancestor of any other state
    // on the list, and 2) a full legal state configuration results when all
    // ancestors and default initial descendants have been added". `s1 s1`
    // and `initial="s0 s0"` each denote one state.
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><raise event="go"/></onentry><transition event="go" target="s1 s1"/></state><state id="s1"><onentry><log label="s1"/></onentry><transition target="done"/></state><final id="done"/>"#,
    )
    .replace(r#"initial="s0""#, r#"initial="s0 s0""#));
    assert_eq!(outcome.terminal, Terminal::Finished, "{outcome:?}");
    assert_eq!(labels(&outcome), ["s1"]);
}

#[test]
fn a_document_with_an_id_less_history_loads_and_runs() {
    // SCXML 3.14: "When such an attribute is defined to be optional and the
    // author omits it, then, for elements other than <send> and <invoke>,
    // the SCXML processor MUST generate a unique id automatically at
    // document load time." `<history>`'s `id` is optional (3.10), so a
    // document with an id-less history loads and runs.
    let outcome = run(&scxml(
        r#"<state id="s0"><history><transition target="a"/></history><state id="a"><onentry><log label="a"/></onentry><transition target="done"/></state></state><final id="done"/>"#,
    ));
    assert_eq!(outcome.terminal, Terminal::Finished, "{outcome:?}");
    assert_eq!(labels(&outcome), ["a"]);
}

#[test]
fn the_null_data_model_evaluates_no_value_expression() {
    // SCXML B.1.4: "There is no value expression language." B.1.3: "There
    // is no location expression language." A value expression the null
    // data model is handed (a non-conformant document, A.1; the validator
    // rejects it at load, which 5.9.4 permits) returns no legal data value:
    // 5.9.3 "If a value expression does not return a legal data value, the
    // SCXML Processor MUST place the error 'error.execution' in the internal
    // event queue."
    for element in [
        r#"<log label="value" expr="'text'"/>"#,
        r#"<log label="value" expr="null"/>"#,
        r#"<send eventexpr="'e'"/>"#,
    ] {
        let outcome = run(&scxml(&format!(
            r#"<state id="s0"><onentry>{element}<raise event="after"/></onentry><transition event="error.execution" target="done"/><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
        )));
        assert_eq!(outcome.configuration, ["done"], "{element}: {outcome:?}");
        assert!(labels(&outcome).is_empty(), "{element}: nothing is logged");
    }
}

#[test]
fn a_send_type_expression_that_names_no_type_raises_error_execution() {
    // SCXML 6.2.5: "If the SCXML Processor does not support the type that
    // is specified, it MUST place the event error.execution on the internal
    // event queue." A `typeexpr` whose value is not a string names no type
    // the processor supports.
    let outcome = run(&ecmascript(
        r#"<state id="s0"><onentry><send id="typed" event="e" typeexpr="42"/></onentry><transition event="error.execution" target="done"><log label="error" expr="_event.sendid"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("error".into(), "typed".into())]);
}

#[test]
fn inline_content_the_data_model_cannot_represent_raises_error_execution() {
    // SCXML 3.12.2: once executing, the processor "MUST signal any errors
    // that occur by raising SCXML events whose names begin with 'error.'";
    // 5.9.3: a value that is not a legal data value is error.execution. The
    // null data model has no data values at all (B.1.1 "There is no
    // underlying data model."), so the send is not delivered and the
    // session goes on.
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><send event="e"><content><x/></content></send></onentry><transition event="error.execution" target="done"/><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

#[test]
fn basichttp_inline_xml_content_is_the_message_body() {
    // SCXML C.2: "If a <content> child is present, the SCXML Processor MUST
    // use its value as the body of the message." 5.6.2: the children of
    // <content> "MAY consist of text, XML from any namespace, or a mixture of
    // both", and "If the 'expr' attribute is not present, the Processor MUST
    // use the children of <content> as the output."
    let location = loopback_location("root");
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><onentry><send event="posted" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="{location}"><content><x a="1">t</x></content></send></onentry><transition event="posted" target="done"><log label="body" expr="_event.data"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    // The markup stands alone, so it declares the namespace its element is
    // in (here the SCXML default namespace, SCXML G.6).
    assert_eq!(
        records(&outcome),
        [(
            "body".into(),
            r#"<x xmlns="http://www.w3.org/2005/07/scxml" a="1">t</x>"#.into()
        )]
    );
}

/// An ECMAScript parent that invokes `child` with `params` and logs the
/// first event the child sends back (`pass` or `done.invoke` passes).
fn invoke_with_params(params: &str, child: &str) -> SessionOutcome {
    run(&ecmascript(&format!(
        r#"<state id="s0"><invoke type="scxml">{params}<content>{child}</content></invoke><transition event="pass" target="done"/><transition event="done.invoke" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )))
}

#[test]
fn params_to_a_child_without_data_are_not_added_and_do_not_fail_the_invocation() {
    // SCXML 6.4.3: "If the names do not match, the Processor MUST NOT add
    // the value of the <param> element or namelist key/value pair to the
    // invoked session's data model. However the Processor MAY make the
    // values available by some other platform-specific means." A child that
    // declares no data matches no name, so the invocation simply runs.
    let outcome = invoke_with_params(
        r#"<param name="p" expr="1"/>"#,
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><final id="c"/></scxml>"#,
    );
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

#[test]
fn params_override_only_top_level_data_and_skip_its_own_value() {
    // SCXML 6.4.3: "If the 'name' of a <param> element in the <invoke>
    // matches the 'id' of a <data> element in the top-level data
    // declarations of the invoked session, the SCXML Processor MUST use the
    // value of the <param> element as the initial value of the
    // corresponding <data> element. (The top-level data declarations are
    // those that are contained in the <datamodel> element that is a child
    // of <scxml>.) (Note that this means that any value specified in the
    // <data> element is ignored.)" and "If the names do not match, the
    // Processor MUST NOT add the value ... to the invoked session's data
    // model." `top`'s own expr fails, but it is ignored, so no
    // error.execution; the state-scoped `nested` keeps its own value.
    let outcome = invoke_with_params(
        r#"<param name="top" expr="1"/><param name="nested" expr="1"/>"#,
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><datamodel><data id="top" expr="undefined.property"/></datamodel><state id="c"><datamodel><data id="nested" expr="0"/></datamodel><transition event="error.execution" target="bad"/><transition cond="top === 1 &amp;&amp; nested === 0" target="good"/><transition target="bad"/></state><state id="good"><onentry><send event="pass" target="#_parent"/></onentry></state><state id="bad"><onentry><send event="wrong" target="#_parent"/></onentry></state></scxml>"##,
    );
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

/// An ECMAScript child that doubles its `x` and `y` and returns them to its
/// parent under the same names.
const DOUBLING_CHILD: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c"><datamodel><data id="x" expr="0"/><data id="y" expr="0"/></datamodel><state id="c"><onentry><assign location="x" expr="x * 2"/><assign location="y" expr="y * 2"/><send event="reply" target="#_parent" namelist="x y"/></onentry></state></scxml>"##;

#[test]
fn an_empty_finalize_updates_the_namelist_and_param_locations() {
    // SCXML 6.5.2: "If no executable content is specified, the SCXML
    // Processor MUST update the data model each time an event is received
    // from the child process that was created by the parent <invoke>
    // element. Specifically if the parent <invoke> element contains a
    // 'namelist' attribute or one or more <param> children containing
    // 'location' attributes, then for each item in the 'namelist' attribute
    // and each such <param> element, the Processor MUST update the
    // corresponding location as if by <assign> with any return value that
    // has a name that matches the 'namelist' item or the 'name' of the
    // <param> element."
    let outcome = run(&ecmascript(&format!(
        r#"<datamodel><data id="x" expr="3"/><data id="z" expr="4"/></datamodel><state id="s0"><invoke type="scxml" namelist="x"><content>{DOUBLING_CHILD}</content><finalize/></invoke><transition event="reply" target="done"><log label="x" expr="x"/><log label="z" expr="z"/></transition><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("x".into(), "6".into()), ("z".into(), "4".into())]
    );
    let outcome = run(&ecmascript(&format!(
        r#"<datamodel><data id="x" expr="3"/><data id="z" expr="4"/></datamodel><state id="s0"><invoke type="scxml"><param name="y" location="z"/><content>{DOUBLING_CHILD}</content><finalize></finalize></invoke><transition event="reply" target="done"><log label="x" expr="x"/><log label="z" expr="z"/></transition><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("x".into(), "3".into()), ("z".into(), "8".into())]
    );
}

#[test]
fn an_absent_finalize_updates_nothing() {
    // SCXML 6.5.2: "Note that the automatic update does not take place if
    // the <finalize> element is absent as opposed to empty."
    let outcome = run(&ecmascript(&format!(
        r#"<datamodel><data id="x" expr="3"/></datamodel><state id="s0"><invoke type="scxml" namelist="x"><content>{DOUBLING_CHILD}</content></invoke><transition event="reply" target="done"><log label="x" expr="x"/></transition><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("x".into(), "3".into())]);
}

#[test]
fn an_empty_finalize_updates_xpath_locations_from_the_child_data() {
    // SCXML 6.5.2 under the XPath data model: the child returns
    // `<data id="x">` (CR 2014-03-13 C.3.7.1), which updates `$x`.
    let child = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath" initial="c"><datamodel><data id="x">0</data></datamodel><state id="c"><onentry><assign location="$x" expr="$x * 2"/><send event="reply" target="#_parent" namelist="$x"/></onentry></state></scxml>"##;
    let outcome = run(&format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath" initial="s0"><datamodel><data id="x">3</data></datamodel><state id="s0"><invoke type="scxml" namelist="$x"><content>{child}</content><finalize/></invoke><transition event="reply" target="done"><log label="x" expr="string($x)"/></transition><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/></scxml>"#
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("x".into(), "6".into())]);
}

#[test]
fn an_asynchronous_error_communication_goes_on_the_internal_queue() {
    // SCXML 3.12.2: the processor "MUST place these events in the internal
    // event queue"; 6.2.4: "If it is unable to dispatch the message, the
    // Processor MUST place the error error.communication on the internal
    // event queue." The delayed send to the child `c`, which has finished
    // when it falls due, cannot be dispatched, and that is known only after
    // its <send> completed. An internal event is never autoforwarded (6.4.1
    // forwards external events), so the autoforward child `w`, alive
    // throughout, never sees the error; through the external queue it
    // would have been forwarded, and `w` would answer `forwarded`.
    let outcome = run(&ecmascript(
        r##"<state id="p" initial="s0"><invoke id="w" autoforward="true"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="w0"><state id="w0"><transition event="error.communication"><send event="forwarded" target="#_parent"/></transition></state></scxml></content></invoke><state id="s0"><invoke id="c"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="ready" target="#_parent"/></onentry><transition event="stop" target="cf"/></state><final id="cf"/></scxml></content></invoke><transition event="ready"><send id="late" event="late" target="#_c" delay="10ms"/><send event="stop" target="#_c"/></transition><transition event="done.invoke.c"/><transition event="error.communication" target="s1"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><onentry><send event="settled" delay="30ms"/></onentry><transition event="settled" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state></state><final id="done"/><final id="fail"/>"##,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("error".into(), "late:platform".into())]
    );
}

#[test]
fn a_parent_reaches_its_child_through_the_childs_scxml_session_address() {
    // SCXML C.1: "If the target is the special term '#_scxml_sessionid',
    // where sessionid is the id of an SCXML session that is accessible to
    // the Processor, the Processor MUST add the event to the external queue
    // of that session." 5.10.1: for external events the processor "SHOULD
    // set [origin] to a value which, when used as the value of 'target',
    // will allow the receiver of the event to <send> a response back to
    // the originating entity". The child's own sessions invoked by this
    // parent are accessible to it: the reply to the child's `origin`
    // (`#_scxml_<child _sessionid>`) reaches the child.
    let outcome = run(&ecmascript(
        r##"<state id="s0"><invoke id="kid"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="k0"><state id="k0"><onentry><send event="hello" target="#_parent"/></onentry><transition event="reply" cond="_event.origin === '#_scxml_' + _event.data.parent" target="k1"><send event="got" target="#_parent"/></transition></state><state id="k1"/></scxml></content></invoke><transition event="hello" cond="_event.origin.indexOf('#_scxml_') === 0"><send event="reply" targetexpr="_event.origin"><param name="parent" expr="_sessionid"/></send></transition><transition event="got" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"##,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

/// `(session, label, text)` of every log record of the run, in trace order.
fn session_logs(outcome: &SessionOutcome) -> Vec<(String, String, String)> {
    outcome
        .trace
        .iter()
        .filter_map(|entry| match &entry.observation {
            TraceObservation::Log(log) => Some((
                entry.session.clone(),
                log.label.clone().unwrap_or_default(),
                log.text.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn a_child_and_a_grandchild_are_named_by_their_invoke_ids() {
    // SCXML 5.10: `_sessionid` is "the system-generated id for the current
    // SCXML session". The host names a child `<parent _sessionid>.<invokeid>`,
    // so an invoker knows its child's address before the child starts.
    let grandchild = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="g0"><state id="g0"><onentry><log label="id" expr="_sessionid"/></onentry></state></scxml>"#;
    let child = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c0"><state id="c0"><onentry><log label="id" expr="_sessionid"/></onentry><invoke id="grand" src="grandchild.scxml"/></state></scxml>"#;
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<state id="s0"><onentry><send event="check" delay="1ms"/></onentry><invoke id="kid" src="child.scxml"/><transition event="check" target="done"/></state><final id="done"/>"#,
        ),
        &[],
        vec![
            ("child.scxml", child.to_owned()),
            ("grandchild.scxml", grandchild.to_owned()),
        ],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        session_logs(&outcome),
        [
            ("root.kid".into(), "id".into(), "root.kid".into()),
            (
                "root.kid.grand".into(),
                "id".into(),
                "root.kid.grand".into()
            ),
        ]
    );
}

#[test]
fn a_generated_invoke_id_is_the_state_id_and_an_ordinal_of_the_invoking_session() {
    // SCXML 6.4.1: a generated id has "the form stateid.platformid", where
    // "platformid MUST be unique within the current session": `<state id>.<n>`,
    // the same form in a child session as in the root, and the child it
    // names is `<parent _sessionid>.<invokeid>`. `idlocation`,
    // `_event.invokeid` and `#_<invokeid>` carry it.
    let grandchild = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="g0"><state id="g0"><onentry><log label="id" expr="_sessionid"/></onentry></state></scxml>"#;
    let child = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><datamodel><data id="where"/></datamodel><state id="s"><onentry><log label="id" expr="_sessionid"/><send event="hello" target="#_parent"/></onentry><invoke src="grandchild.scxml" idlocation="where"/><transition event="ping"><log label="where" expr="where"/><send event="pong" target="#_parent"/></transition></state></scxml>"##;
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<datamodel><data id="where"/></datamodel><state id="s0" initial="s"><state id="s"><invoke src="child.scxml" idlocation="where"/><transition event="hello"><log label="hello" expr="_event.invokeid + ' ' + where"/><send event="ping" targetexpr="'#_' + where"/></transition><transition event="pong" target="done"/></state></state><final id="done"/>"#,
        ),
        &[],
        vec![
            ("child.scxml", child.to_owned()),
            ("grandchild.scxml", grandchild.to_owned()),
        ],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    let logs = session_logs(&outcome);
    assert_eq!(outcome.configuration, ["done"], "{logs:?}");
    for expected in [
        ("root.s.0", "id", "root.s.0"),
        ("root", "hello", "s.0 s.0"),
        ("root.s.0.s.0", "id", "root.s.0.s.0"),
        ("root.s.0", "where", "s.0"),
    ] {
        let expected = (
            expected.0.to_owned(),
            expected.1.to_owned(),
            expected.2.to_owned(),
        );
        assert!(logs.contains(&expected), "{expected:?} in {logs:?}");
    }
}

#[test]
fn a_child_invoked_again_under_its_invoke_id_keeps_its_address() {
    // SCXML 6.4: leaving the invoking state cancels the invocation, and
    // entering it again starts a new one under the same literal id. The
    // new session has the old one's `_sessionid`, and its address
    // (`#_scxml_<sessionid>`, C.1) reaches the new session.
    let child = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c0"><state id="c0"><onentry><log label="id" expr="_sessionid"/><send event="hello" target="#_parent"/></onentry><transition event="ping"><send event="pong" target="#_parent"/></transition></state></scxml>"##;
    let outcome = run_with(
        &host(),
        &ecmascript(
            r##"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><invoke id="kid" src="child.scxml"/><transition event="hello" cond="n === 0" target="s0"><assign location="n" expr="1"/></transition><transition event="hello" cond="n === 1"><send event="ping" target="#_scxml_root.kid"/></transition><transition event="pong" target="done"/><transition event="error.*" target="fail"><log label="error" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"##,
        ),
        &[],
        vec![("child.scxml", child.to_owned())],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        session_logs(&outcome),
        [
            ("root.kid".into(), "id".into(), "root.kid".into()),
            ("root.kid".into(), "id".into(), "root.kid".into()),
        ]
    );
}

#[test]
fn an_invocation_whose_child_session_id_is_long_starts() {
    // A child's `_sessionid` is `<parent _sessionid>.<invokeid>`, so the id
    // grows with the nesting depth, with no length bound: the child
    // `root.<1,000 bytes>` (1,005 bytes) invoking a 1,000-byte id names a
    // 2,006-byte grandchild, which starts.
    let kid = "k".repeat(1_000);
    let grand = "g".repeat(1_000);
    let child = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="check" delay="1ms"/></onentry><invoke id="{grand}" type="scxml" src="waiting.scxml"/><transition event="error.execution"><send event="refused" target="#_parent"/></transition><transition event="check"><send event="started" target="#_parent"/></transition></state></scxml>"##
    );
    let outcome = run_with(
        &host(),
        &scxml(&format!(
            r#"<state id="s0"><invoke id="{kid}" type="scxml" src="child.scxml"/><transition event="refused" target="refused"/><transition event="started" target="started"/></state><final id="refused"/><final id="started"/>"#
        )),
        &[],
        vec![
            ("child.scxml", child),
            ("waiting.scxml", WAITING_CHILD.to_owned()),
        ],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["started"]);
    let grandchild = format!("root.{kid}.{grand}");
    assert!(
        outcome
            .trace
            .iter()
            .any(|entry| entry.session == grandchild)
    );
}

/// An ECMAScript child document with `body` inside its `<scxml>`.
fn ecmascript_child(initial: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="{initial}">{body}</scxml>"#
    )
}

#[test]
fn a_grandchild_reaches_the_root_by_its_scxml_session_address() {
    // SCXML C.1: "If the target is the special term '#_scxml_sessionid',
    // where sessionid is the id of an SCXML session that is accessible to
    // the Processor, the Processor MUST add the event to the external queue
    // of that session"; "The set of SCXML sessions that are accessible to a
    // given SCXML Processor is platform-dependent": every session of the
    // run. 5.10.1: `origin` is the sender's address, and `invokeid` is set
    // only for an event from an invoked child, so the root's grandchild's
    // event carries none.
    let grandchild = ecmascript_child(
        "g0",
        r##"<state id="g0"><onentry><send event="hello" target="#_scxml_root"/></onentry></state>"##,
    );
    let child = ecmascript_child(
        "c0",
        r#"<state id="c0"><invoke id="grand" src="grandchild.scxml"/></state>"#,
    );
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<state id="s0"><invoke id="kid" src="child.scxml"/><transition event="hello" target="done"><log label="hello" expr="_event.origin + ' ' + typeof _event.invokeid"/></transition></state><final id="done"/>"#,
        ),
        &[],
        vec![("child.scxml", child), ("grandchild.scxml", grandchild)],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("hello".into(), "#_scxml_root.kid.grand undefined".into())]
    );
}

#[test]
fn a_child_reaches_its_parent_by_its_scxml_session_address_with_its_invoke_id() {
    // SCXML 5.10.1: an event from an invoked child carries its invoke id,
    // whichever address the child sent it to.
    let child = ecmascript_child(
        "c0",
        r##"<state id="c0"><onentry><send event="hello" target="#_scxml_root"/></onentry></state>"##,
    );
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<state id="s0"><invoke id="kid" src="child.scxml"/><transition event="hello" target="done"><log label="hello" expr="_event.origin + ' ' + _event.invokeid"/></transition></state><final id="done"/>"#,
        ),
        &[],
        vec![("child.scxml", child)],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("hello".into(), "#_scxml_root.kid kid".into())]
    );
}

#[test]
fn a_child_reaches_a_sibling_by_its_address_before_the_sibling_has_sent_anything() {
    // SCXML C.1, with every session of the run accessible: `a` sends to
    // its sibling at `root.b` (`<parent _sessionid>.<invokeid>`), which
    // has sent nothing, and `b` tells the root who it heard from.
    let a = ecmascript_child(
        "a0",
        r##"<state id="a0"><onentry><send event="hi" target="#_scxml_root.b"/></onentry></state>"##,
    );
    let b = ecmascript_child(
        "b0",
        r##"<state id="b0"><transition event="hi"><send event="heard" target="#_parent"><param name="from" expr="_event.origin"/></send></transition></state>"##,
    );
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<state id="s0"><invoke id="b" src="b.scxml"/><invoke id="a" src="a.scxml"/><transition event="heard" target="done"><log label="heard" expr="_event.data.from"/></transition></state><final id="done"/>"#,
        ),
        &[],
        vec![("a.scxml", a), ("b.scxml", b)],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [("heard".into(), "#_scxml_root.a".into())]
    );
}

#[test]
fn a_send_to_an_ended_or_unknown_session_of_the_run_raises_error_communication() {
    // SCXML C.1: "If the sending SCXML session specifies a session that
    // does not exist or is inaccessible, the SCXML Processor MUST place the
    // error error.communication on the internal event queue." `b` has
    // finished, and no session of the run is `root.nobody`.
    let a = ecmascript_child(
        "a0",
        r##"<state id="a0"><transition event="go" target="a1"><send event="hi" target="#_scxml_root.b"/></transition></state><state id="a1"><onentry><send event="hi" target="#_scxml_root.nobody"/></onentry><transition event="error.communication" target="a2"><send event="lost" target="#_parent"/></transition></state><state id="a2"><transition event="error.communication"><send event="lost" target="#_parent"/></transition></state>"##,
    );
    let b = ecmascript_child("b0", r#"<final id="b0"/>"#);
    let outcome = run_with(
        &host(),
        &ecmascript(
            r##"<datamodel><data id="lost" expr="0"/></datamodel><state id="s0"><invoke id="a" src="a.scxml"/><invoke id="b" src="b.scxml"/><transition event="done.invoke.b"><send event="go" target="#_a"/></transition><transition event="lost" cond="lost === 0"><assign location="lost" expr="1"/></transition><transition event="lost" cond="lost === 1" target="done"/></state><final id="done"/>"##,
        ),
        &[],
        vec![("a.scxml", a), ("b.scxml", b)],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

#[test]
fn a_delayed_send_to_another_session_is_delivered_at_its_time_or_cancelled() {
    // SCXML 6.2: "It MUST dispatch the message only when the delay
    // interval elapses"; 6.3: `<cancel>` stops a delayed send not yet
    // dispatched. `a` sends `early` and `late` to its sibling `b`, both
    // delayed, and cancels `late`; `b` hears `early` only.
    let a = ecmascript_child(
        "a0",
        r##"<state id="a0"><onentry><send event="early" target="#_scxml_root.b" delay="10ms"/><send id="late" event="late" target="#_scxml_root.b" delay="20ms"/><cancel sendid="late"/></onentry></state>"##,
    );
    let b = ecmascript_child(
        "b0",
        r##"<state id="b0"><transition event="early"><send event="heard" target="#_parent"><param name="name" expr="_event.name"/></send><send event="settle" delay="30ms"/></transition><transition event="late"><send event="heard" target="#_parent"><param name="name" expr="_event.name"/></send></transition><transition event="settle"><send event="settled" target="#_parent"/></transition></state>"##,
    );
    let outcome = run_with(
        &host(),
        &ecmascript(
            r#"<state id="s0"><invoke id="b" src="b.scxml"/><invoke id="a" src="a.scxml"/><transition event="heard"><log label="heard" expr="_event.data.name"/></transition><transition event="settled" target="done"/></state><final id="done"/>"#,
        ),
        &[],
        vec![("a.scxml", a), ("b.scxml", b)],
        RUN_COMMANDS,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("heard".into(), "early".into())]);
}

#[test]
fn event_descriptor_tokens_match_case_sensitively() {
    // SCXML 3.12.1: "An event descriptor matches an event name if its string
    // of tokens is an exact match or a prefix of the set of tokens in the
    // event's name. In all cases, the token matching is case sensitive."
    let outcome = run(&scxml(
        r#"<state id="s0"><onentry><raise event="Foo.Bar"/></onentry><transition event="foo" target="fail"/><transition event="Foo.bar" target="fail"/><transition event="Foo" target="s1"/></state><state id="s1"><onentry><raise event="foo.bar"/></onentry><transition event="Foo" target="fail"/><transition event="foo.bar" target="done"/></state><final id="done"/><final id="fail"/>"#,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

#[test]
fn error_error_dot_and_error_dot_star_are_equivalent_descriptors() {
    // SCXML 3.12.1: "a transition with 'event' of "error", one with
    // "error.", and one with "error.*" are functionally equivalent since
    // they each match exactly the same set of event names". Each form alone
    // matches error.execution and error.communication and not errors.x.
    for descriptor in ["error", "error.", "error.*"] {
        let outcome = run(&scxml(&format!(
            r#"<state id="s0"><onentry><raise event="errors.x"/><raise event="error.execution"/></onentry><transition event="{descriptor}" target="s1"/><transition event="*" target="fail"/></state><state id="s1"><onentry><raise event="error.communication.detail"/></onentry><transition event="{descriptor}" target="done"/><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
        )));
        assert_eq!(
            outcome.configuration,
            ["fail"],
            "{descriptor}: errors.x is not an error event"
        );
        let outcome = run(&scxml(&format!(
            r#"<state id="s0"><onentry><raise event="error.execution"/></onentry><transition event="{descriptor}" target="s1"/><transition event="*" target="fail"/></state><state id="s1"><onentry><raise event="error.communication.detail"/></onentry><transition event="{descriptor}" target="done"/><transition event="*" target="fail"/></state><final id="done"/><final id="fail"/>"#
        )));
        assert_eq!(outcome.configuration, ["done"], "{descriptor}: {outcome:?}");
    }
}

#[test]
fn a_parallel_region_that_is_itself_parallel_completes_the_outer_parallel() {
    // Appendix D isInFinalState: "Return true if s is a compound <state>
    // and one of its children is an active <final> state (i.e. is a member
    // of the current configuration), or if s is a <parallel> state and
    // isInFinalState is true of all its children." 3.7: on entering the
    // final child of `c`, "if the parent <state> is a child of a <parallel>
    // element, and all of the <parallel>'s other children are also in
    // final states, the Processor MUST generate the event done.state.id
    // where id is the id of the <parallel> element". `outer`'s other child
    // `inner` is itself a parallel whose regions are final, so it is in a
    // final state and done.state.outer follows done.state.c.
    let outcome = run_with(
        &host(),
        &scxml(
            r#"<parallel id="s0"><parallel id="inner"><state id="a"><final id="af"/></state><state id="b"><final id="bf"/></state></parallel><state id="c"><state id="c1"><transition event="go" target="cf"/></state><final id="cf"/></state><transition event="done.state.c"><log label="c-done"/></transition><transition event="done.state.s0" target="done"><log label="outer-done"/></transition></parallel><final id="done"/>"#,
        ),
        &["go".to_owned()],
        Vec::new(),
        64,
    )
    .expect("the run completes");
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(labels(&outcome), ["c-done", "outer-done"]);
}

#[test]
fn data_sent_between_sessions_is_a_copy() {
    // SCXML C.1: "the Processor MUST ensure that changes to the transmitted
    // data in the receiving session do not affect the data in the sending
    // session and vice-versa." The parent sends an object to its child; the
    // child copies it out of the read-only `_event` (5.10), changes its copy,
    // and returns it; the parent's object keeps its value, and the parent's
    // later change never reaches the child's.
    let outcome = run(&ecmascript(
        r##"<datamodel><data id="obj" expr="({ n: 1 })"/></datamodel><state id="s0"><invoke id="kid"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="k0"><datamodel><data id="mine"/></datamodel><state id="k0"><transition event="take"><assign location="mine" expr="Object.assign({}, _event.data.obj)"/><assign location="mine.n" expr="2"/><send event="back" target="#_parent"><param name="obj" expr="mine"/></send></transition><transition event="check" cond="mine.n === 2"><send event="kid-intact" target="#_parent"/></transition></state></scxml></content></invoke><transition event="done.invoke" target="fail"/><transition event="back" cond="obj.n === 1 &amp;&amp; _event.data.obj.n === 2"><assign location="obj.n" expr="3"/><send event="check" target="#_kid"/></transition><transition event="kid-intact" cond="obj.n === 3" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"##,
    ).replace(
        r#"<state id="s0"><invoke id="kid">"#,
        r#"<state id="s0"><onentry><send event="start" delay="1ms"/></onentry><invoke id="kid">"#,
    ).replace(
        r#"<transition event="done.invoke" target="fail"/>"#,
        r##"<transition event="done.invoke" target="fail"/><transition event="start"><send event="take" target="#_kid" namelist="obj"/></transition>"##,
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

#[test]
fn internal_dtd_entities_expand_in_the_running_document() {
    // SCXML A.2: "the XML parser MUST be able to parse and process all
    // well-formed XML constructs defined within [XML] and [XMLNames]". XML
    // 1.0 §5.1: a non-validating processor "MUST use the information in
    // those declarations to ... include the replacement text of internal
    // entities".
    let outcome = run(&format!(
        "<!DOCTYPE scxml [<!ENTITY next \"done\"><!ENTITY label \"reached &next;\">]>{}",
        ecmascript(
            r#"<state id="s0"><onentry><raise event="go"/></onentry><transition event="go" target="&next;"><log label="&label;" expr="'&next;'"/></transition></state><final id="done"/>"#
        )
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("reached done".into(), "done".into())]);
}

#[test]
fn internal_subset_markup_parameter_entities_and_defaults_run() {
    // XML 1.0 §5.1: a non-validating processor "MUST use the information
    // in those declarations to normalize attribute values, include the
    // replacement text of internal entities, and supply default attribute
    // values". The states come from an entity whose replacement text holds
    // markup (§4.4.2), the log label from an entity a parameter entity
    // declares (§4.4.8), and the transition's `event` and the document's
    // namespace from attribute-list defaults.
    let outcome = run(&format!(
        "<!DOCTYPE scxml [<!ENTITY % labels \"<!ENTITY label 'reached'>\"> %labels; <!ENTITY states \"<state id='s0'><onentry><raise event='go'/></onentry><transition target='done'><log label='&label;' expr='1 + 1'/></transition></state><final id='done'/>\"><!ATTLIST scxml xmlns CDATA #FIXED 'http://www.w3.org/2005/07/scxml' version CDATA '1.0'><!ATTLIST transition event NMTOKENS '  go  '>]>{}",
        r#"<scxml datamodel="ecmascript" initial="s0">&states;</scxml>"#
    ));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(records(&outcome), [("reached".into(), "2".into())]);
}

#[test]
fn an_xpath_session_binds_an_ecmascript_array_as_a_space_normalized_literal() {
    // SCXML C.1 raises error.communication only when "the receiving
    // Processor cannot handle the data format contained in the message", and
    // "If the data models in the sending and receiving sessions are
    // different, the mapping between them is platform-specific." SCXML 1.0
    // CR 2014-03-13 §C.3.7.1 maps content that is neither key-value pairs
    // nor a valid XML document: "Otherwise, the Processor MUST treat the
    // content as a space-normalized string literal and insert it as the
    // content of _event's <data> child." The ECMAScript child sends an
    // array; the XPath parent handles it and sees its JSON text.
    let outcome = run(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="xpath" initial="s0"><state id="s0"><invoke id="kid" type="scxml"><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="k"><state id="k"><onentry><send event="list" target="#_parent"><content expr="[1, 2]"/></send><send event="after" target="#_parent"/></onentry></state></scxml></content></invoke><transition event="list" cond="$_event/data = '[1,2]'" target="s1"/><transition event="*" target="fail"><log label="unexpected" expr="$_event/name"/></transition></state><state id="s1"><transition event="after" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="$_event/name"/></transition></state><final id="done"/><final id="fail"/></scxml>"##,
    );
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

/// The `BasicHTTP` processor type URI (SCXML C.2).
const BASICHTTP_TYPE: &str = "http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor";

#[test]
fn a_delayed_basichttp_send_posts_only_when_its_delay_elapses() {
    // SCXML 6.2: "It MUST dispatch the message only when the delay interval
    // elapses." The POST to the session's own location is made at 50ms, so
    // the SCXML send due at 10ms arrives first; the message is posted as
    // evaluated at the `<send>` (6.2: "The Processor MUST evaluate all
    // arguments to <send> when the <send> element is evaluated, and not when
    // the message is actually dispatched.").
    let outcome = run(&ecmascript(&format!(
        r#"<datamodel><data id="n" expr="7"/></datamodel><state id="s0"><onentry><send id="post" type="{BASICHTTP_TYPE}" targetexpr="_ioprocessors['basichttp'].location" event="posted" delay="50ms" namelist="n"/><assign location="n" expr="8"/><send event="first" delay="10ms"/></onentry><transition event="first" target="s1"><log label="first"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><transition event="posted" target="done"><log label="posted" expr="_event.data.n + ' ' + _event.origintype"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert_eq!(
        records(&outcome),
        [
            ("first".into(), String::new()),
            ("posted".into(), format!("7 {BASICHTTP_TYPE}")),
        ]
    );
}

#[test]
fn a_cancelled_delayed_basichttp_send_is_never_posted() {
    // SCXML 6.3: "The <cancel> element is used to cancel a delayed <send>
    // event."
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><onentry><send id="post" type="{BASICHTTP_TYPE}" targetexpr="_ioprocessors['basichttp'].location" event="posted" delay="50ms"/><send event="first" delay="10ms"/></onentry><transition event="first" target="s1"><cancel sendid="post"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><onentry><send event="end" delay="100ms"/></onentry><transition event="end" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert!(records(&outcome).is_empty(), "{outcome:?}");
}

#[test]
fn a_terminated_session_discards_its_delayed_basichttp_send() {
    // SCXML 6.2: "If the SCXML session terminates before the delay interval
    // has elapsed, the SCXML Processor MUST discard the message without
    // attempting to deliver it." The child finishes at once; its POST to the
    // parent's location, due at 50ms, never arrives.
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><invoke id="kid" type="scxml"><param name="loc" expr="_ioprocessors['basichttp'].location"/><content><scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="k"><datamodel><data id="loc"/></datamodel><state id="k"><onentry><send id="post" type="{BASICHTTP_TYPE}" targetexpr="loc" event="posted" delay="50ms"/></onentry><transition target="kf"/></state><final id="kf"/></scxml></content></invoke><transition event="done.invoke.kid" target="s1"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><onentry><send event="end" delay="100ms"/></onentry><transition event="end" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
    assert!(records(&outcome).is_empty(), "{outcome:?}");
}

/// A loopback target matches a session's location in its WHATWG canonical
/// form, as the browser host matches it (`new URL(target).href`): the
/// default port and a `.` segment address the session itself.
#[test]
fn loopback_targets_match_locations_in_their_canonical_form() {
    for target in [
        format!("{LOOPBACK_ORIGIN}:80/sessions/root/basichttp"),
        format!("{LOOPBACK_ORIGIN}/sessions/./root/basichttp"),
        "HTTP://BASICHTTP.LOOPBACK.INVALID/sessions/root/basichttp".to_owned(),
    ] {
        let outcome = run(&ecmascript(&format!(
            r#"<state id="s0"><onentry><send type="{BASICHTTP_TYPE}" target="{target}" event="self"/></onentry><transition event="self" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
        )));
        assert_eq!(outcome.configuration, ["done"], "{target}: {outcome:?}");
    }
}

#[test]
fn a_due_basichttp_send_that_cannot_be_dispatched_raises_error_communication_when_due() {
    // SCXML 6.2.4: "If it is unable to dispatch the message, the Processor
    // MUST place the error error.communication on the internal event
    // queue", carrying the failed send's id (5.10.1) as a platform event.
    // Nobody answers the loopback location at 50ms (404), and a batch run
    // has no outbound transport for any other address; the SCXML send due
    // at 10ms arrives first, so the failure is the due dispatch's.
    for target in [
        format!("{LOOPBACK_ORIGIN}/sessions/nobody/basichttp"),
        "http://127.0.0.1:9/receiver".to_owned(),
    ] {
        let outcome = run(&ecmascript(&format!(
            r#"<state id="s0"><onentry><send id="post" type="{BASICHTTP_TYPE}" target="{target}" event="posted" delay="50ms"/><send event="first" delay="10ms"/></onentry><transition event="first" target="s1"><log label="first"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><state id="s1"><transition event="error.communication" target="done"><log label="error" expr="_event.sendid + ':' + _event.type"/></transition><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
        )));
        assert_eq!(outcome.configuration, ["done"], "{target}: {outcome:?}");
        assert_eq!(
            records(&outcome),
            [
                ("first".into(), String::new()),
                ("error".into(), "post:platform".into()),
            ],
            "{target}"
        );
    }
}

#[test]
fn an_invalid_delayed_basichttp_send_raises_error_execution_at_the_send() {
    // SCXML 6.2: "The Processor MUST evaluate all arguments to <send> when
    // the <send> element is evaluated, and not when the message is actually
    // dispatched."; 6.2.4: an invalid target raises error.execution. The
    // processor checks the delayed send before it is scheduled.
    let outcome = run(&ecmascript(&format!(
        r#"<state id="s0"><onentry><send id="post" type="{BASICHTTP_TYPE}" target="ftp://example.test/" event="posted" delay="50ms"/></onentry><transition event="error.execution" target="done"/><transition event="*" target="fail"><log label="unexpected" expr="_event.name"/></transition></state><final id="done"/><final id="fail"/>"#
    )));
    assert_eq!(outcome.configuration, ["done"], "{outcome:?}");
}

const SELF_INVOKING_CHILD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c0"><state id="c0"><transition cond="_sessionid.length &gt; 1019" target="bottom"/><invoke type="scxml" src="child.scxml"/><transition event="done.invoke" target="up"/></state><final id="bottom"/><final id="up"/></scxml>"#;

/// A document that invokes itself until its own `_sessionid` is 1,020 bytes
/// or longer, with no invoke limits: SCXML 6.4 sets no depth, and the host
/// refuses no `<invoke>` of its own. Each
/// child starts from the host's loop once its invoking session suspended,
/// not inside that session's call, so the chain takes no native stack per
/// level. A child's `_sessionid` is `<parent _sessionid>.<invokeid>`, so
/// each level adds its generated invoke id (`.c0.0`) to the id: the chain
/// nests until the deepest level's id is 1,024 bytes, which leaves `c0`
/// before its `<invoke>` runs, and every level's `done.invoke` climbs back
/// to the root, never a stack overflow. The whole
/// chain starts inside the root's start, one host call of the fuel budget
/// (the document itself is compiled once, apart from it), so every level's
/// start must fit that budget.
#[test]
fn self_invocation_nests_without_a_depth_limit() {
    const BUDGET: u64 = 4_000;
    let root = scxml(
        r#"<state id="s0"><invoke type="scxml" src="child.scxml"/><transition event="done.invoke" target="done"/><transition event="error.execution" target="done"><log label="refused"/></transition></state><final id="done"/>"#,
    );
    let documents = || vec![("child.scxml", SELF_INVOKING_CHILD.to_owned())];
    // Ten million fuel per level: every level's start fits.
    let fueled = host_with(&WasmtimeHostConfig {
        store: WasmtimeStoreConfig {
            fuel: 10_000_000 * BUDGET,
            ..WasmtimeStoreConfig::default()
        },
        ..WasmtimeHostConfig::default()
    });
    let outcome = run_with(&fueled, &root, &[], documents(), BUDGET)
        .expect("the chain ends at its own condition");
    assert_eq!(outcome.configuration, ["done"]);
    assert!(labels(&outcome).is_empty(), "the root's own invoke started");
    // `root.s0.0` (9 bytes), then `.c0.0` (5 bytes) per level: the 203rd
    // level is 1,024 bytes.
    let deepest = format!("root.s0.0{}", ".c0.0".repeat(203));
    assert_eq!(deepest.len(), 1_024);
    let sessions: std::collections::BTreeSet<&str> = outcome
        .trace
        .iter()
        .map(|entry| entry.session.as_str())
        .collect();
    assert_eq!(sessions.len(), 1 + 1 + 203, "the root and every level");
    assert!(sessions.contains(deepest.as_str()));
}

/// Serves one `<script src>` of ECMAScript text.
struct ScriptText(Vec<u8>);

impl ScriptResolver for ScriptText {
    fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
        if uri == "https://example.test/lib.js" {
            Ok(ResolvedScript {
                media_type: "text/javascript".to_owned(),
                bytes: self.0.clone(),
            })
        } else {
            Err(ScriptResolveError::NotFound)
        }
    }
}

/// An embedder whose one `<script src>` resolves to `text`, trusted.
struct Scripted(Vec<u8>);

impl SessionEmbedder for Scripted {
    fn document(&self, _kind: DocumentKind, _uri: &str) -> Option<EmbeddedDocument> {
        None
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        let digest = format!("{:x}", Sha256::digest(&self.0));
        MachineScripts::runtime(
            Box::new(ScriptText(self.0.clone())),
            ScriptPolicy::new([TrustedComponentDigest::from_hex(&digest).expect("canonical")]),
        )
    }
}

/// The host bounds no `<script src>` text: the session's ECMAScript data
/// model decides, as in the browser host
/// (hosts/scxml-host-browser/test/resources.test.mjs), which bounds no
/// script length either: 65 KiB of text loads and runs.
#[test]
fn script_src_text_is_bounded_only_by_the_data_model() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="a"><datamodel><data id="loaded" expr="0"/></datamodel><script src="https://example.test/lib.js"/><state id="a"><transition event="error.execution" target="refused"/><transition cond="loaded === 2" target="pass"/></state><final id="pass"/><final id="refused"/></scxml>"#;
    let (ir, data_model) = lower(source);
    let configuration = |padding: usize| {
        let text = format!("loaded = 2;{}", " ".repeat(padding)).into_bytes();
        host()
            .run(&SessionRun {
                machine_ir: &ir,
                data_model: data_model.as_deref(),
                session_id: "root",
                events: &[],
                external_client: None,
                embedder: Rc::new(Scripted(text)),
                max_commands: 64,
                snapshots: SnapshotRequest::none(),
            })
            .expect("the run completes")
            .configuration
    };
    assert_eq!(configuration(65 * 1024), ["pass"]);
}
