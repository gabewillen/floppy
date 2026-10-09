//! W3C trace propagation through every event route of the Wasmtime session
//! host, proven on real documents run through the shipped components with
//! an in-memory OpenTelemetry pipeline.
//!
//! The machine opens one `execute` span per effect batch (one per engine
//! command here: no fixture uses `<if>` or `<foreach>`). Recording spans in
//! start order and pairing them with the run trace's `Command` records
//! attributes each batch to its session; each test then pins, per batch,
//! which span is its parent. A batch that processes an event must be the
//! child of the batch that emitted it, whatever route the event took.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]
#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use opentelemetry::trace::{Span as _, SpanId, TraceId};
use opentelemetry::{Context, KeyValue, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{SdkTracerProvider, Span, SpanData, SpanProcessor};
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, DocumentKind, EmbeddedDocument, MachineScripts,
    ScriptedEvent, SessionEmbedder, SessionHost, SessionHostComponents, SessionOutcome, SessionRun,
    TelemetryProviders, Terminal, TraceObservation, TrustedManifestDigest, WasmtimeHostConfig,
    authenticate_component, basichttp_types, loopback_location,
};
use sha2::{Digest as _, Sha256};

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
            let status = Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .args(COMPONENTS.iter().map(|(_, name)| *name))
                .current_dir(workspace)
                .env("CARGO_TARGET_DIR", &target)
                .status()
                .expect("the capability script runs");
            assert!(status.success(), "the release capability proof must pass");
            target.join("wasm-artifacts/release")
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

/// Records every finished span with the order in which it started.
#[derive(Debug, Default)]
struct StartOrder {
    next: AtomicI64,
    finished: Arc<Mutex<Vec<SpanData>>>,
}

const ORDER_KEY: &str = "test.start-order";

impl SpanProcessor for StartOrder {
    fn on_start(&self, span: &mut Span, _cx: &Context) {
        let order = self.next.fetch_add(1, Ordering::SeqCst);
        span.set_attribute(KeyValue::new(ORDER_KEY, order));
    }

    fn on_end(&self, span: SpanData) {
        self.finished
            .lock()
            .expect("the span record is never poisoned")
            .push(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
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

    fn scripts(&self) -> MachineScripts {
        MachineScripts::none()
    }
}

fn scxml(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="s0">{body}</scxml>"#
    )
}

/// One machine effect batch: its session and its span identity.
#[derive(Debug)]
struct Batch {
    session: String,
    span: SpanId,
    parent: SpanId,
    trace: TraceId,
}

/// One traced run.
struct Traced {
    outcome: SessionOutcome,
    run_span: SpanId,
    run_trace: TraceId,
    batches: Vec<Batch>,
}

impl Traced {
    /// Each batch as `session<-parent`, where the parent is `run` (the
    /// session-run span), `#k` (batch k), `remote` (the caller's carrier),
    /// or `other`.
    fn lineage(&self, remote: Option<SpanId>) -> Vec<String> {
        self.batches
            .iter()
            .map(|batch| {
                let parent = if batch.parent == self.run_span {
                    "run".to_owned()
                } else if Some(batch.parent) == remote {
                    "remote".to_owned()
                } else if let Some(index) = self
                    .batches
                    .iter()
                    .position(|other| other.span == batch.parent)
                {
                    format!("#{index}")
                } else {
                    "other".to_owned()
                };
                format!("{}<-{parent}", batch.session)
            })
            .collect()
    }
}

fn attribute<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| &attribute.value)
}

/// The machine's batch spans, in start order, from every finished span in
/// end order.
///
/// Each batch ends with its `execute` span, then the machine records an
/// `execute` outcome point signal and, for a completed batch, an event
/// point signal (both zero-length spans under the batch's own parent).
/// Nothing else ends in between, so in end order every batch span is
/// followed by exactly those signals.
fn batch_spans(spans: &[SpanData]) -> Vec<&SpanData> {
    let executes: Vec<&SpanData> = spans
        .iter()
        .filter(|span| {
            span.name == "execute"
                && attribute(span, "scxml.module") == Some(&Value::from("machine-component"))
        })
        .collect();
    let mut batches = Vec::new();
    let mut index = 0;
    while let Some(batch) = executes.get(index) {
        let signal = executes
            .get(index + 1)
            .expect("an outcome signal follows every batch span");
        assert!(
            signal.events.is_empty() && signal.parent_span_id == batch.parent_span_id,
            "the outcome signal shares the batch's parent"
        );
        index += 2;
        if executes
            .get(index)
            .is_some_and(|event| !event.events.is_empty())
        {
            index += 1;
        }
        batches.push(*batch);
    }
    batches.sort_by_key(|span| match attribute(span, ORDER_KEY) {
        Some(Value::I64(order)) => *order,
        _ => i64::MAX,
    });
    batches
}

struct Run<'a> {
    source: String,
    events: Vec<ScriptedEvent>,
    documents: Vec<(&'static str, String)>,
    external_client: Option<&'a dyn Fn(&str) -> basichttp_types::HttpRequest>,
}

impl Run<'_> {
    fn new(source: String) -> Self {
        Self {
            source,
            events: Vec::new(),
            documents: Vec::new(),
            external_client: None,
        }
    }
}

fn run_traced(run: &Run<'_>) -> Traced {
    let finished = Arc::new(Mutex::new(Vec::new()));
    let tracer = SdkTracerProvider::builder()
        .with_span_processor(StartOrder {
            next: AtomicI64::new(0),
            finished: Arc::clone(&finished),
        })
        .build();
    let providers = TelemetryProviders::from_providers(tracer, SdkMeterProvider::builder().build());
    let all = components();
    let host = SessionHost::new(
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
        &WasmtimeHostConfig::default(),
        &providers,
    )
    .expect("the verified components link");
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "trace.scxml",
        run.source.as_str(),
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let bytes = ir.to_json().expect("fixture IR encodes");
    let outcome = host
        .run(&SessionRun {
            machine_ir: &bytes,
            data_model: ir.data_model.as_deref(),
            session_id: "root",
            events: &run.events,
            external_client: run.external_client,
            embedder: Rc::new(Documents(run.documents.clone())),
            max_commands: 256,
            snapshots: scxml_host_wasmtime::SnapshotRequest::none(),
        })
        .expect("the run completes");
    drop(host);
    // In end order.
    let spans = std::mem::take(&mut *finished.lock().expect("never poisoned"));
    let run_span = spans
        .iter()
        .find(|span| span.name == "session-run")
        .expect("the run span is recorded");
    let (run_span, run_trace) = (
        run_span.span_context.span_id(),
        run_span.span_context.trace_id(),
    );
    let sessions: Vec<String> = outcome
        .trace
        .iter()
        .filter(|entry| entry.observation == TraceObservation::Command)
        .map(|entry| entry.session.clone())
        .collect();
    let executes = batch_spans(&spans);
    assert_eq!(
        executes.len(),
        sessions.len(),
        "one batch span per engine command"
    );
    let batches = sessions
        .into_iter()
        .zip(executes)
        .map(|(session, span)| Batch {
            session: if session == "root" {
                session
            } else {
                "child".to_owned()
            },
            span: span.span_context.span_id(),
            parent: span.parent_span_id,
            trace: span.span_context.trace_id(),
        })
        .collect();
    Traced {
        outcome,
        run_span,
        run_trace,
        batches,
    }
}

/// Every batch of a run without a remote carrier is in the run's trace.
fn assert_one_trace(traced: &Traced) {
    assert!(
        traced
            .batches
            .iter()
            .all(|batch| batch.trace == traced.run_trace),
        "every batch joins the run trace"
    );
}

#[test]
fn raised_event_is_processed_under_the_raising_batch() {
    let traced = run_traced(&Run::new(scxml(
        r#"<state id="s0"><onentry><raise event="r"/></onentry><transition event="r" target="done"/></state><final id="done"/>"#,
    )));
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(None), ["root<-run", "root<-#0"]);
    assert_one_trace(&traced);
}

#[test]
fn immediate_and_delayed_self_sends_are_processed_under_the_sending_batch() {
    let traced = run_traced(&Run::new(scxml(
        r#"<state id="s0"><onentry><send event="now"/><send event="later" delay="5ms"/></onentry><transition event="now" target="s1"/></state><state id="s1"><transition event="later" target="done"/></state><final id="done"/>"#,
    )));
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    // start; `now` (immediate); `later` (routed when due on the clock).
    assert_eq!(traced.lineage(None), ["root<-run", "root<-#0", "root<-#0"]);
    assert_one_trace(&traced);
}

#[test]
fn send_errors_are_processed_under_the_failing_batch() {
    let traced = run_traced(&Run::new(scxml(
        r##"<state id="s0"><onentry><send event="lost" target="#_scxml_nobody"/></onentry><onentry><send event="bad" target="nowhere"/></onentry><transition event="error.communication" target="s1"/></state><state id="s1"><transition event="error.execution" target="done"/></state><final id="done"/>"##,
    )));
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(None), ["root<-run", "root<-#0", "root<-#0"]);
    assert_one_trace(&traced);
}

/// A `BasicHTTP` loopback delivery carries the sending batch's context, so
/// the received event's batch is that batch's child, never a new root under
/// the run span.
#[test]
fn basichttp_loopback_delivery_is_processed_under_the_sending_batch() {
    let location = loopback_location("root");
    let traced = run_traced(&Run::new(scxml(&format!(
        r#"<state id="s0"><onentry><send event="posted" type="basichttp" target="{location}"/></onentry><transition event="posted" target="done"/></state><final id="done"/>"#
    ))));
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(None), ["root<-run", "root<-#0"]);
    assert_one_trace(&traced);
}

const CHILD: &str = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="hello" target="#_parent"/></onentry><transition event="stop" target="cdone"/></state><final id="cdone"/></scxml>"##;

/// The child's start, `#_parent`, `#_<invokeid>`, and `done.invoke`: each
/// batch is the child of the batch that invoked, sent, or (for
/// `done.invoke`) completed it.
#[test]
fn events_between_invoking_and_invoked_sessions_keep_their_lineage() {
    let mut run = Run::new(scxml(
        r##"<state id="s0" initial="a"><invoke id="kid" type="scxml" src="child.scxml"/><state id="a"><transition event="hello" target="b"/></state><state id="b"><onentry><send event="stop" target="#_kid"/></onentry></state><transition event="done.invoke.kid" target="done"/></state><final id="done"/>"##,
    ));
    run.documents = vec![("child.scxml", CHILD.to_owned())];
    let traced = run_traced(&run);
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    // Root start and its invoking step; the child's start under the
    // invoking batch, never the run span; `hello` (#_parent)
    // under the child's start; `stop` (#_kid) under the root's `hello`
    // batch; `done.invoke.kid` under the child's finishing batch.
    assert_eq!(
        traced.lineage(None),
        [
            "root<-run",
            "root<-run",
            "child<-#1",
            "root<-#2",
            "child<-#3",
            "root<-#4"
        ]
    );
    assert_one_trace(&traced);
}

/// An autoforwarded copy is processed under the parent batch that
/// forwarded it (not under the carrier the event arrived with), which is
/// itself the child of the caller's carrier.
#[test]
fn autoforwarded_event_is_processed_under_the_forwarding_batch() {
    const WAITING: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><transition event="ping" target="cdone"/></state><final id="cdone"/></scxml>"#;
    let mut run = Run::new(scxml(
        r#"<state id="s0"><invoke id="kid" type="scxml" src="child.scxml" autoforward="true"/><transition event="ping"/><transition event="done.invoke.kid" target="done"/></state><final id="done"/>"#,
    ));
    run.documents = vec![("child.scxml", WAITING.to_owned())];
    let (context, trace, span) = remote_carrier();
    run.events = vec![ScriptedEvent {
        trace_context: Some(context),
        ..ScriptedEvent::named("ping")
    }];
    let traced = run_traced(&run);
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    // The child starts under the invoking batch (#1); the root's `ping`
    // batches are the caller carrier's children; the child processes the
    // forwarded copy under the forwarding batch (#3), and the root
    // processes `done.invoke.kid` under the child's finishing batch (#5).
    assert_eq!(
        traced.lineage(Some(span)),
        [
            "root<-run",
            "root<-run",
            "child<-#1",
            "root<-remote",
            "root<-remote",
            "child<-#3",
            "root<-#5",
            "root<-#5"
        ]
    );
    assert_eq!(
        traced.batches[5].trace, trace,
        "the caller's trace continues"
    );
}

fn remote_carrier() -> (scxml_types::TraceContext, TraceId, SpanId) {
    let trace = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").expect("trace id");
    let span = SpanId::from_hex("b7ad6b7169203331").expect("span id");
    let context = scxml_types::TraceContext::new(
        format!("00-{trace}-{span}-01"),
        Some("vendor=value".into()),
        Some("tenant=a".into()),
        scxml_types::BaggagePolicy::Propagate,
    )
    .expect("canonical caller carrier");
    (context, trace, span)
}

/// A caller-supplied carrier parents the batch that processes the event,
/// and the lineage continues in the caller's trace.
#[test]
fn scripted_event_carrier_parents_its_processing_batch() {
    let (context, trace, span) = remote_carrier();
    let mut run = Run::new(scxml(
        r#"<state id="s0"><transition event="go" target="s1"><raise event="next"/></transition></state><state id="s1"><transition event="next" target="done"/></state><final id="done"/>"#,
    ));
    run.events = vec![ScriptedEvent {
        trace_context: Some(context),
        ..ScriptedEvent::named("go")
    }];
    let traced = run_traced(&run);
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(
        traced.lineage(Some(span)),
        ["root<-run", "root<-remote", "root<-#1"]
    );
    assert_eq!(traced.batches[1].trace, trace);
    assert_eq!(traced.batches[2].trace, trace);
}

/// The external client's W3C `traceparent` header parents the batch that
/// processes its POST; a malformed header is ignored, never rejected.
#[test]
fn external_client_traceparent_parents_its_processing_batch() {
    let (_, trace, span) = remote_carrier();
    let request = |traceparent: String| {
        move |location: &str| basichttp_types::HttpRequest {
            method: "POST".into(),
            uri: location.to_owned(),
            headers: vec![
                basichttp_types::HttpHeader {
                    name: "_scxmleventname".into(),
                    value: "knock".into(),
                },
                basichttp_types::HttpHeader {
                    name: "TraceParent".into(),
                    value: traceparent.clone(),
                },
            ],
            body: Vec::new(),
        }
    };
    let source = scxml(
        r#"<state id="s0"><transition event="knock" target="done"/></state><final id="done"/>"#,
    );
    let valid = request(format!("00-{trace}-{span}-01"));
    let mut run = Run::new(source.clone());
    run.external_client = Some(&valid);
    let traced = run_traced(&run);
    assert_eq!(
        traced
            .outcome
            .http_response
            .as_ref()
            .map(|response| response.status),
        Some(200)
    );
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(Some(span)), ["root<-run", "root<-remote"]);
    assert_eq!(traced.batches[1].trace, trace);

    let malformed = request("00-not-a-traceparent".into());
    let mut run = Run::new(source);
    run.external_client = Some(&malformed);
    let traced = run_traced(&run);
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(None), ["root<-run", "root<-run"]);
    assert_one_trace(&traced);
}

fn ecmascript(body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s0">{body}</scxml>"#
    )
}

/// The `error.execution` a failed transition condition raises carries a
/// carrier, so its batch never starts under the run span. Selection runs
/// outside any batch: the error names the batch the machine call last
/// opened (the start batch here), whose effects the condition observed.
#[test]
fn eventless_condition_error_is_processed_under_the_preceding_batch() {
    let traced = run_traced(&Run::new(ecmascript(
        r#"<state id="s0"><transition cond="missing.member" target="never"/><transition event="error.execution" target="done"/></state><state id="never"/><final id="done"/>"#,
    )));
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    assert_eq!(traced.lineage(None), ["root<-run", "root<-#0"]);
    assert_one_trace(&traced);
}

/// Before a machine call opens its first batch, a failed condition names
/// the event under evaluation: the error's batch is the caller carrier's
/// child, in the caller's trace.
#[test]
fn condition_error_on_an_external_event_is_processed_under_its_carrier() {
    let (context, trace, span) = remote_carrier();
    let mut run = Run::new(ecmascript(
        r#"<state id="s0"><transition event="go" cond="missing.member" target="never"/><transition event="error.execution" target="done"/></state><state id="never"/><final id="done"/>"#,
    ));
    run.events = vec![ScriptedEvent {
        trace_context: Some(context),
        ..ScriptedEvent::named("go")
    }];
    let traced = run_traced(&run);
    assert_eq!(traced.outcome.terminal, Terminal::Finished);
    let lineage = traced.lineage(Some(span));
    assert_eq!(
        lineage.last().map(String::as_str),
        Some("root<-remote"),
        "{lineage:?}"
    );
    assert_eq!(traced.batches.last().map(|batch| batch.trace), Some(trace));
}
