//! Invoked sessions that invoke in turn (SCXML 1.0 section 6.4), run
//! through the shipped components under the Wasmtime session host: events
//! between a grandchild, its invoking session, and the root; `done.invoke`
//! at each level; `<finalize>` and autoforward across levels; the cancel
//! cascade; the embedder's invoke limits; snapshots of nested invocations
//! (and of sessions beyond the snapshot document's bounds); and replay of a
//! recorded nested session. The W3C IRP has no nested-invocation case, so
//! these are the project's own proofs.
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

use scxml_host_wasmtime::scxml_snapshot::{Options, SnapshotErrorCode, decode};
use scxml_host_wasmtime::{
    Applied, DocumentKind, EmbeddedDocument, HostError, InvokeLimits, LiveClock, LiveHttp,
    LiveStatus, MachineScripts, ManualClock, ReleaseComponents, RunFailure, ScriptedEvent,
    SessionEmbedder, SessionHost, SessionHostError, SessionOpen, SessionOutcome, SessionOutput,
    SessionRun, SnapshotFailure, SnapshotPoints, SnapshotRequest, TelemetryProviders, Terminal,
    TraceObservation, TrustedManifestDigest, WasmtimeHostConfig, WasmtimeStoreConfig,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const COMPONENTS: [&str; 8] = [
    "scxml_machine",
    "scxml_compiler",
    "scxml_data_model_ecmascript",
    "scxml_data_model_xpath",
    "scxml_data_model_null",
    "scxml_ioprocessor_scxml",
    "scxml_ioprocessor_basichttp",
    "scxml_clock",
];

fn components() -> &'static ReleaseComponents {
    static COMPONENTS_CELL: OnceLock<ReleaseComponents> = OnceLock::new();
    COMPONENTS_CELL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("host crate has a workspace parent");
        let release = prebuilt::release_dir().unwrap_or_else(|| {
            let target = workspace.join("target/session-host-proof");
            let release = target.join("wasm-artifacts/nested-invocation");
            let status = Command::new("sh")
                .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
                .arg("--artifacts-dir")
                .arg(&release)
                .args(COMPONENTS)
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
        ReleaseComponents::load(&release, trusted)
            .unwrap_or_else(|error| panic!("the release components load: {error}"))
    })
}

fn host() -> SessionHost {
    SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig::default(),
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

/// Whether a live session records its inputs.
#[derive(Clone, Copy, PartialEq)]
enum Recording {
    On,
    Off,
}

fn scxml(initial: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="{initial}">{body}</scxml>"#
    )
}

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "nested.scxml",
        source,
    ))
    .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn run(
    host: &SessionHost,
    root: &str,
    documents: Vec<(&'static str, String)>,
    snapshots: SnapshotRequest,
) -> SessionOutcome {
    run_with_budget(host, root, documents, snapshots, 1_024)
}

fn run_with_budget(
    host: &SessionHost,
    root: &str,
    documents: Vec<(&'static str, String)>,
    snapshots: SnapshotRequest,
    max_commands: u64,
) -> SessionOutcome {
    let (ir, data_model) = lower(root);
    host.run(&SessionRun {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        events: &[],
        external_client: None,
        embedder: Rc::new(Documents(documents)),
        max_commands,
        snapshots,
    })
    .unwrap_or_else(|error| panic!("the run completes: {error}"))
}

/// Every `<log>` label of every session, in the order the host observed
/// them, as `session:label`.
fn logs(outcome: &SessionOutcome) -> Vec<String> {
    outcome
        .trace
        .iter()
        .filter_map(|entry| match &entry.observation {
            TraceObservation::Log(log) => Some(format!(
                "{}:{}",
                entry.session,
                log.label.clone().unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

/// The child document: it invokes the grandchild, relays the grandchild's
/// `up` to its parent, forwards `down` to the grandchild, and finishes on
/// the grandchild's `done.invoke`.
fn child() -> String {
    scxml(
        "c0",
        r##"<state id="c0">
              <invoke id="gc" type="scxml" src="grandchild.scxml"/>
              <transition event="up"><log label="child-got-up"/><send target="#_parent" event="relay"/></transition>
              <transition event="down"><log label="child-got-down"/><send target="#_gc" event="deep"/></transition>
              <transition event="done.invoke.gc" target="cdone"><log label="child-got-done"/></transition>
            </state>
            <final id="cdone"/>"##,
    )
}

fn grandchild() -> String {
    scxml(
        "g0",
        r##"<state id="g0">
              <onentry><send target="#_parent" event="up"/></onentry>
              <transition event="deep" target="gdone"><log label="grandchild-got-deep"/></transition>
            </state>
            <final id="gdone"/>"##,
    )
}

#[test]
fn a_grandchild_messages_its_parent_and_done_invoke_reaches_each_level() {
    let root = scxml(
        "r0",
        r##"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="relay"><log label="root-got-relay"/><send target="#_child" event="down"/></transition>
              <transition event="done.invoke.child" target="rdone"><log label="root-got-done"/></transition>
            </state>
            <final id="rdone"/>"##,
    );
    let outcome = run(
        &host(),
        &root,
        vec![("child.scxml", child()), ("grandchild.scxml", grandchild())],
        SnapshotRequest::none(),
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    let sessions: Vec<String> = logs(&outcome)
        .iter()
        .map(|log| {
            log.split_once(':')
                .map(|(_, label)| label.to_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        sessions,
        [
            "child-got-up",
            "root-got-relay",
            "child-got-down",
            "grandchild-got-deep",
            "child-got-done",
            "root-got-done",
        ]
    );
    // The grandchild is its own session, invoked by the child.
    let grandchild = outcome
        .trace
        .iter()
        .find(|entry| {
            matches!(&entry.observation, TraceObservation::Log(log)
                if log.label.as_deref() == Some("grandchild-got-deep"))
        })
        .expect("the grandchild logged");
    let child = outcome
        .trace
        .iter()
        .find(|entry| {
            matches!(&entry.observation, TraceObservation::Log(log)
                if log.label.as_deref() == Some("child-got-up"))
        })
        .expect("the child logged");
    assert_eq!(grandchild.parent.as_deref(), Some(child.session.as_str()));
    assert_eq!(child.parent.as_deref(), Some("root"));
}

#[test]
fn cancelling_a_session_cancels_the_sessions_it_invoked_and_ignores_their_sends() {
    let child = scxml(
        "c0",
        r#"<state id="c0">
              <invoke id="gc" type="scxml" src="grandchild.scxml"/>
              <onexit><log label="child-exit"/></onexit>
              <transition event="gc-exiting"><log label="child-heard-grandchild"/></transition>
            </state>"#,
    );
    let grandchild = scxml(
        "g0",
        r##"<state id="g0">
              <onexit><log label="grandchild-exit"/><send target="#_parent" event="gc-exiting"/></onexit>
            </state>"##,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <onentry><send event="stop" delay="10ms"/></onentry>
              <onexit><log label="root-exit"/></onexit>
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="stop" target="r1"/>
            </state>
            <state id="r1">
              <onentry><send event="quiet" delay="10ms"/></onentry>
              <transition event="quiet" target="rdone"/>
              <transition event="gc-exiting" target="rdone"><log label="root-heard-grandchild"/></transition>
            </state>
            <final id="rdone"/>"#,
    );
    let outcome = run(
        &host(),
        &root,
        vec![("child.scxml", child), ("grandchild.scxml", grandchild)],
        SnapshotRequest::none(),
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    let labels: Vec<String> = logs(&outcome)
        .iter()
        .map(|log| {
            log.split_once(':')
                .map(|(_, label)| label.to_owned())
                .unwrap_or_default()
        })
        .collect();
    // exitInterpreter order: each state's <onexit>, then its invocations
    // are cancelled; nothing a cancelled session sends is delivered.
    assert_eq!(labels, ["root-exit", "child-exit", "grandchild-exit"]);
}

#[test]
fn finalize_and_autoforward_work_across_levels() {
    let child = scxml(
        "c0",
        r#"<state id="c0">
              <invoke id="gc" type="scxml" src="grandchild.scxml" autoforward="true">
                <finalize><log label="child-finalize"/></finalize>
              </invoke>
              <transition event="pong" target="cdone"><log label="child-got-pong"/></transition>
            </state>
            <final id="cdone"/>"#,
    );
    let grandchild = scxml(
        "g0",
        r##"<state id="g0">
              <transition event="ping"><log label="grandchild-got-ping"/><send target="#_parent" event="pong"/></transition>
            </state>"##,
    );
    let root = scxml(
        "r0",
        r##"<state id="r0">
              <onentry><send event="go" delay="5ms"/></onentry>
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="go"><send target="#_child" event="ping"/></transition>
              <transition event="done.invoke.child" target="rdone"><log label="root-got-done"/></transition>
            </state>
            <final id="rdone"/>"##,
    );
    let outcome = run(
        &host(),
        &root,
        vec![("child.scxml", child), ("grandchild.scxml", grandchild)],
        SnapshotRequest::none(),
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    let labels: Vec<String> = logs(&outcome)
        .iter()
        .map(|log| {
            log.split_once(':')
                .map(|(_, label)| label.to_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        labels,
        [
            "grandchild-got-ping",
            "child-finalize",
            "child-got-pong",
            "root-got-done",
        ]
    );
}

#[test]
fn an_invocation_beyond_the_depth_bound_raises_error_execution_in_its_invoking_session() {
    let child = scxml(
        "c0",
        r#"<state id="c0">
              <invoke id="gc" type="scxml" src="grandchild.scxml"/>
              <transition event="error.execution" target="cdone"><log label="child-bounded"/></transition>
            </state>
            <final id="cdone"/>"#,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="done.invoke.child" target="rdone"/>
            </state>
            <final id="rdone"/>"#,
    );
    let host = host()
        .with_invoke_limits(InvokeLimits {
            max_invoke_depth: Some(1),
            max_invoked_sessions: Some(8),
        })
        .expect("valid bounds");
    let outcome = run(
        &host,
        &root,
        vec![("child.scxml", child), ("grandchild.scxml", grandchild())],
        SnapshotRequest::none(),
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(logs(&outcome).len(), 1);
    assert!(logs(&outcome)[0].ends_with(":child-bounded"));
}

/// A chain of `depth` documents: `level1.scxml` invokes `level2.scxml`, and
/// so on; the last waits in `c0`.
fn waiting_chain(depth: usize) -> Vec<(&'static str, String)> {
    (1..=depth)
        .map(|level| {
            let body = if level == depth {
                r#"<state id="c0"/>"#.to_owned()
            } else {
                format!(
                    r#"<state id="c0"><invoke id="k" type="scxml" src="level{}.scxml"/></state>"#,
                    level + 1
                )
            };
            let uri: &'static str = Box::leak(format!("level{level}.scxml").into_boxed_str());
            (uri, scxml("c0", &body))
        })
        .collect()
}

fn depth_exceeded(failure: &SnapshotFailure) -> bool {
    matches!(failure, SnapshotFailure::Codec(error) if error.code == SnapshotErrorCode::DepthExceeded)
}

/// The native host imposes no invoke-depth limit, so a session can nest
/// deeper than a `scxml.session-snapshot/v2` document represents (32
/// levels). Each snapshot of it fails with `depth-exceeded` for that
/// snapshot alone: the batch run and the live session go on.
#[test]
fn a_snapshot_beyond_the_document_bounds_fails_alone_and_the_session_goes_on() {
    const DEPTH: usize = 40;
    let root = scxml(
        "r0",
        r#"<state id="r0"><invoke id="k" type="scxml" src="level1.scxml"/><transition event="go" target="rdone"/></state><final id="rdone"/>"#,
    );
    let outcome = run(
        &host(),
        &root,
        waiting_chain(DEPTH),
        SnapshotRequest {
            points: SnapshotPoints::Every,
            options: Options::default(),
        },
    );
    assert_eq!(outcome.terminal, Terminal::Running);
    let sessions: std::collections::BTreeSet<&str> = outcome
        .trace
        .iter()
        .map(|entry| entry.session.as_str())
        .collect();
    assert_eq!(sessions.len(), 1 + DEPTH, "the root and every level ran");
    assert!(outcome.snapshots.is_empty(), "{:?}", outcome.snapshots);
    assert!(!outcome.snapshot_failures.is_empty());
    assert!(
        outcome
            .snapshot_failures
            .iter()
            .all(|failed| depth_exceeded(&failed.failure)),
        "{:?}",
        outcome.snapshot_failures
    );

    let (ir, data_model) = lower(&root);
    let mut live = host()
        .open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(Documents(waiting_chain(DEPTH))),
            max_commands: 1_024,
            clock: LiveClock::Logical,
            record: false,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens");
    let error = live
        .snapshot(&Options::default())
        .expect_err("the snapshot is beyond the document bounds");
    assert!(
        matches!(&error.failure, RunFailure::Snapshot(failure) if depth_exceeded(failure)),
        "{error}"
    );
    let step = live
        .send(&ScriptedEvent::named("go"))
        .expect("the session goes on after the failed snapshot");
    assert_eq!(step.status, LiveStatus::Finished);
    live.close().expect("close");
}

#[test]
fn snapshots_show_every_invoked_session_and_each_external_queue() {
    let child = scxml(
        "c0",
        r#"<state id="c0">
              <invoke id="gc" type="scxml" src="grandchild.scxml"/>
            </state>"#,
    );
    let grandchild = scxml(
        "g0",
        r##"<state id="g0"><onentry><send target="#_parent" event="hello"/></onentry></state>"##,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0"><invoke id="child" type="scxml" src="child.scxml"/></state>"#,
    );
    let outcome = run(
        &host(),
        &root,
        vec![("child.scxml", child), ("grandchild.scxml", grandchild)],
        SnapshotRequest {
            points: SnapshotPoints::At(vec![0]),
            options: Options::default(),
        },
    );
    let [snapshot] = outcome.snapshots.as_slice() else {
        panic!("one snapshot");
    };
    let document = decode(&snapshot.bytes).expect("the document decodes");
    let root = &document.snapshot.root;
    let child = &root.invocations.as_ref().expect("invocations")[0];
    assert_eq!(child.invoke_id, "child");
    let grandchild = &child.session.invocations.as_ref().expect("nested")[0];
    assert_eq!(grandchild.invoke_id, "gc");
    assert_eq!(
        grandchild
            .session
            .invoked_by
            .as_ref()
            .map(|by| by.parent_session_id.as_str()),
        Some(child.session.session_id.as_str())
    );
    // Right after the root's start, the grandchild's `hello` waits on the
    // child's external queue, stamped with the grandchild's invoke id.
    let queued = &child.session.queues.as_ref().expect("queues").external;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].name, "hello");
    assert_eq!(queued[0].invokeid.as_deref(), Some("gc"));
    let shown = scxml_host_wasmtime::scxml_snapshot::render_show(
        &document,
        scxml_host_wasmtime::scxml_snapshot::Color::Never,
    );
    assert!(shown.contains("session child/gc"), "{shown}");
}

#[test]
fn a_recorded_nested_session_replays_byte_for_byte() {
    let child = scxml(
        "c0",
        r##"<state id="c0">
              <onentry><send event="tick" delay="20ms"/></onentry>
              <invoke id="gc" type="scxml" src="grandchild.scxml"/>
              <transition event="tick"><send target="#_gc" event="deep"/></transition>
              <transition event="up"><send target="#_parent" event="relay"/></transition>
              <transition event="done.invoke.gc" target="cdone"/>
            </state>
            <final id="cdone"/>"##,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="relay"><log label="relay"/></transition>
              <transition event="ping"><log label="ping"/></transition>
              <transition event="done.invoke.child" target="rdone"/>
            </state>
            <final id="rdone"/>"#,
    );
    let (ir, data_model) = lower(&root);
    let documents = || {
        Rc::new(Documents(vec![
            ("child.scxml", child.clone()),
            ("grandchild.scxml", grandchild()),
        ]))
    };
    let host = host();
    let open = |clock, recording: Recording| {
        host.open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            embedder: documents(),
            max_commands: 1_024,
            clock,
            record: recording == Recording::On,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens")
    };
    let clock = ManualClock::new();
    let mut live = open(LiveClock::Timed(Box::new(clock.clone())), Recording::On);
    let options = Options::default();
    let mut snapshots = vec![live.snapshot(&options).expect("snapshot")];
    clock.set(5);
    live.send(&ScriptedEvent::named("ping")).expect("ping");
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    clock.set(25);
    let step = live.advance().expect("the child's tick fires");
    assert_eq!(step.status, LiveStatus::Finished);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    let outputs = live.take_outputs();
    assert!(outputs.contains(&SessionOutput::Done { data: None }));
    let records = live.take_recorded();
    live.close().expect("close");

    let mut replay = open(LiveClock::Logical, Recording::Off);
    let mut replayed = Vec::new();
    for record in &records {
        if let Applied::Snapshot(bytes) = replay.apply(record).expect("the record applies") {
            replayed.push(bytes);
        }
    }
    assert_eq!(replayed, snapshots, "nested snapshots replay byte for byte");
    assert_eq!(replay.take_outputs(), outputs);
    replay.close().expect("close");
}

/// A recorded run in which sessions reach each other by
/// `#_scxml_<sessionid>` (a child its sibling, at once and when a delayed
/// send falls due, and a grandchild the root) replays to the same
/// snapshots and outputs: a delivery between sessions is a pure function of
/// the run, which replay reproduces.
#[test]
fn a_recorded_run_with_deliveries_between_sessions_replays_byte_for_byte() {
    let a = scxml(
        "a0",
        r##"<state id="a0">
              <onentry><send event="hi" target="#_scxml_root.b"/><send event="later" target="#_scxml_root.b" delay="20ms"/></onentry>
              <invoke id="g" type="scxml" src="grandchild.scxml"/>
            </state>"##,
    );
    let b = scxml(
        "b0",
        r##"<state id="b0">
              <transition event="hi"><send target="#_parent" event="heard"/></transition>
              <transition event="later" target="bdone"/>
            </state>
            <final id="bdone"/>"##,
    );
    let grandchild = scxml(
        "g0",
        r##"<state id="g0"><onentry><send event="deep" target="#_scxml_root"/></onentry></state>"##,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="b" type="scxml" src="b.scxml"/>
              <invoke id="a" type="scxml" src="a.scxml"/>
              <transition event="heard"><log label="heard"/></transition>
              <transition event="deep"><log label="deep"/></transition>
              <transition event="done.invoke.b" target="rdone"/>
            </state>
            <final id="rdone"/>"#,
    );
    let (ir, data_model) = lower(&root);
    let documents = || {
        Rc::new(Documents(vec![
            ("a.scxml", a.clone()),
            ("b.scxml", b.clone()),
            ("grandchild.scxml", grandchild.clone()),
        ]))
    };
    let host = host();
    let open = |clock, recording: Recording| {
        host.open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            embedder: documents(),
            max_commands: 1_024,
            clock,
            record: recording == Recording::On,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens")
    };
    let clock = ManualClock::new();
    let mut live = open(LiveClock::Timed(Box::new(clock.clone())), Recording::On);
    let options = Options::default();
    let mut snapshots = vec![live.snapshot(&options).expect("snapshot")];
    clock.set(25);
    let step = live
        .advance()
        .expect("the delayed send to the sibling falls due");
    assert_eq!(step.status, LiveStatus::Finished);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    let outputs = live.take_outputs();
    let mut labels: Vec<&str> = outputs
        .iter()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => entry.label.as_deref(),
            SessionOutput::Done { .. } => None,
        })
        .collect();
    labels.sort_unstable();
    assert_eq!(labels, ["deep", "heard"], "{outputs:?}");
    let records = live.take_recorded();
    live.close().expect("close");

    let mut replay = open(LiveClock::Logical, Recording::Off);
    let mut replayed = Vec::new();
    for record in &records {
        if let Applied::Snapshot(bytes) = replay.apply(record).expect("the record applies") {
            replayed.push(bytes);
        }
    }
    assert_eq!(replayed, snapshots, "the snapshots replay byte for byte");
    assert_eq!(replay.take_outputs(), outputs);
    replay.close().expect("close");
}

/// Depth of the deep-chain proofs: the deepest chain of invoke id `k` the
/// 1024-byte name bound admits. A child's `_sessionid` is
/// `<parent _sessionid>.<invokeid>`, so level `n` under the root `root` is
/// `root` and `n` times `.k`: 4 + 2n bytes.
const DEEP: usize = (scxml_types::MAX_NAME_BYTES - "root".len()) / 2;

/// Every distinct session that recorded anything in the run's trace.
fn traced_sessions(outcome: &SessionOutcome) -> usize {
    outcome
        .trace
        .iter()
        .map(|entry| entry.session.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// Fuel of one host call that starts or cancels a whole [`DEEP`] chain: ten
/// million per level (a level's start compiles its own document, loads its
/// session, and runs its start). The whole nested chain starts inside the
/// root's start, and the root's exit cancels a chain inside one command, so
/// each is one host call of the fuel budget.
const DEEP_CHAIN_FUEL: u64 = 10_000_000 * DEEP as u64;

/// A chain of [`DEEP`] sessions that each start the next inside their own
/// start (the invoke ends the start's macrostep, SCXML 6.4) runs from the
/// host's loop, not nested calls: the chain starts, a snapshot of it fails
/// at once with the document format's bounds error, the root's cancel
/// reaches every level (each exits as its invoking session's cancel reaches
/// it), and the run drops them all, on this debug test thread. The chain
/// starts in one host call, so the host is configured with
/// [`DEEP_CHAIN_FUEL`].
#[test]
fn a_nested_start_chain_to_the_name_bound_starts_cancels_and_drops() {
    let root = scxml(
        "r0",
        r#"<state id="r0"><onentry><send event="stop" delay="1ms"/></onentry><invoke id="k" type="scxml" src="level1.scxml"/><transition event="stop" target="rdone"><log label="stopped"/></transition></state><final id="rdone"/>"#,
    );
    let outcome = run_with_budget(
        &host_with_fuel(DEEP_CHAIN_FUEL),
        &root,
        // One level more than the bound admits: its `<invoke>` fails.
        waiting_chain(DEEP + 1),
        SnapshotRequest {
            points: SnapshotPoints::At(vec![0]),
            options: Options::default(),
        },
        u64::MAX,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(logs(&outcome), ["root:stopped"]);
    assert_eq!(traced_sessions(&outcome), 1 + DEEP, "every level ran");
    // Each level's start, then its cancel, ran one command.
    let commands_per_session = outcome.commands / u64::try_from(1 + DEEP).expect("small");
    assert!(commands_per_session >= 2, "{} commands", outcome.commands);
    let [failed] = outcome.snapshot_failures.as_slice() else {
        panic!("{:?}", outcome.snapshot_failures);
    };
    assert!(
        matches!(&failed.failure, SnapshotFailure::Codec(error) if error.code == SnapshotErrorCode::BoundsExceeded),
        "{:?}",
        failed.failure
    );
}

/// The child of the deferred chain: it invokes the next level only on a
/// later delayed event, so each start is its own step of the run.
const DEFERRED_CHILD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="c0"><state id="c0"><onentry><send event="go" delay="1ms"/></onentry><transition event="go" target="c1"/></state><state id="c1"><invoke type="scxml" src="child.scxml"/></state></scxml>"#;

/// A chain that grows one level per millisecond, one command per level,
/// until the next level's `_sessionid` would exceed the 1024-byte name
/// bound (its `<invoke>` raises `error.execution`): `root.r0.0` and then
/// `.c1.0` per level, 204 levels. Then the root leaves its invoking state:
/// the cancel reaches every level, deepest last, and the whole chain is
/// dropped, on this debug test thread. The cancel of the whole chain is one
/// host call, so the host is configured with [`DEEP_CHAIN_FUEL`].
#[test]
fn a_deferred_chain_to_the_name_bound_cancels_and_drops() {
    let stop_ms = DEEP + 10;
    let root = scxml(
        "r0",
        &format!(
            r#"<state id="r0"><onentry><send event="stop" delay="{stop_ms}ms"/></onentry><invoke type="scxml" src="child.scxml"/><transition event="stop" target="rdone"><log label="stopped"/></transition></state><final id="rdone"/>"#
        ),
    );
    let outcome = run_with_budget(
        &host_with_fuel(DEEP_CHAIN_FUEL),
        &root,
        vec![("child.scxml", DEFERRED_CHILD.to_owned())],
        SnapshotRequest::none(),
        u64::MAX,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(logs(&outcome), ["root:stopped"]);
    assert_eq!(traced_sessions(&outcome), 1 + 204);
}

/// Fuel of one host call in the fuel-sharing proof.
const SHARED_FUEL: u64 = 1_000_000_000;

/// A host whose host call runs on `fuel`.
fn host_with_fuel(fuel: u64) -> SessionHost {
    SessionHost::new(
        &components().session_components(),
        &WasmtimeHostConfig {
            store: WasmtimeStoreConfig {
                fuel,
                ..WasmtimeStoreConfig::default()
            },
            ..WasmtimeHostConfig::default()
        },
        &TelemetryProviders::new(),
    )
    .expect("the verified components link")
}

/// An ECMAScript child whose start runs a script costing about 0.6 of
/// [`SHARED_FUEL`] (seven calls of a 50,000-iteration loop; eleven exceed
/// it alone), then waits.
const BURNING_CHILD: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c0"><state id="c0"><onentry><script>function burn(){var x=0;for(var i=0;i&lt;50000;i++){x+=i}return x}var y=0;for(var j=0;j&lt;7;j++){y+=burn()}</script></onentry></state></scxml>"#;

/// The result of a root that starts `invokes` burning children in its
/// start, on a host whose host call runs on [`SHARED_FUEL`].
fn burning_invocations(invokes: usize) -> Result<SessionOutcome, SessionHostError> {
    let body = r#"<invoke type="scxml" src="burn.scxml"/>"#.repeat(invokes);
    let (ir, data_model) = lower(&scxml("r0", &format!(r#"<state id="r0">{body}</state>"#)));
    host_with_fuel(SHARED_FUEL).run(&SessionRun {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        events: &[],
        external_client: None,
        embedder: Rc::new(Documents(vec![("burn.scxml", BURNING_CHILD.to_owned())])),
        max_commands: 64,
        snapshots: SnapshotRequest::none(),
    })
}

/// One host call spends one fuel budget, including every child start,
/// cancel, and continuation the host carries out for the invocation
/// requests the call deferred: the root's start invokes two children whose
/// starts each cost about 0.6 of the budget, so the start runs out of fuel,
/// exactly as the same work done inside one uninterrupted call would; one
/// such child fits.
#[test]
fn the_child_starts_a_call_defers_spend_that_call_s_fuel() {
    let one = burning_invocations(1).expect("one child fits one host call's fuel");
    assert_eq!(one.terminal, Terminal::Running);
    let error = burning_invocations(2).expect_err("two children exceed one host call's fuel");
    assert!(out_of_fuel(&error.failure), "{:?}", error.failure);
}

/// Whether `failure` is a Wasm trap for running out of fuel.
fn out_of_fuel(failure: &RunFailure) -> bool {
    matches!(
        failure,
        RunFailure::Host(HostError::Wasmtime(trap))
            if trap.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel)
    )
}

/// An event a live session's root takes to a state invoking two burning
/// children exceeds the send's fuel, which traps the root in the
/// continuation of its call: the send fails with that trap, not with a
/// later call into the trapped root's instance, which Wasmtime refuses
/// ("cannot enter component instance").
#[test]
fn a_send_that_runs_out_of_fuel_fails_with_the_trap() {
    let body = r#"<invoke type="scxml" src="burn.scxml"/>"#.repeat(2);
    let (ir, data_model) = lower(&scxml(
        "r0",
        &format!(
            r#"<state id="r0"><transition event="go" target="r1"/></state><state id="r1">{body}</state>"#
        ),
    ));
    let mut live = host_with_fuel(SHARED_FUEL)
        .open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(Documents(vec![("burn.scxml", BURNING_CHILD.to_owned())])),
            max_commands: 64,
            clock: LiveClock::Logical,
            record: false,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens");
    let error = live
        .send(&ScriptedEvent::named("go"))
        .expect_err("two children exceed one send's fuel");
    assert!(out_of_fuel(&error.failure), "{:?}", error.failure);
    assert_eq!(live.status(), LiveStatus::Failed);
}

/// A null-data-model document of `states` states, numbered from `first`
/// so documents built from different ranges differ: cheap to start,
/// costly to compile.
fn wide_document(first: usize, states: usize) -> String {
    let mut body = String::new();
    for state in first..first + states {
        let next = if state + 1 == first + states {
            first
        } else {
            state + 1
        };
        let _ = write!(
            body,
            r#"<state id="w{state}"><transition event="next" target="w{next}"/></state>"#
        );
    }
    scxml(&format!("w{first}"), &body)
}

/// The result of a root that starts `documents` distinct wide `<invoke
/// src>` documents in its start, on a host whose host call runs on
/// `fuel`.
fn wide_invocations(documents: usize, fuel: u64) -> Result<SessionOutcome, SessionHostError> {
    let mut invokes = String::new();
    for document in 0..documents {
        let _ = write!(
            invokes,
            r#"<invoke type="scxml" src="wide{document}.scxml"/>"#
        );
    }
    let (ir, data_model) = lower(&scxml(
        "r0",
        &format!(r#"<state id="r0">{invokes}</state>"#),
    ));
    let served: Vec<(&'static str, String)> = (0..documents)
        .map(|document| {
            let uri: &'static str = Box::leak(format!("wide{document}.scxml").into_boxed_str());
            (uri, wide_document(document * WIDE_STATES, WIDE_STATES))
        })
        .collect();
    host_with_fuel(fuel).run(&SessionRun {
        machine_ir: &ir,
        data_model: data_model.as_deref(),
        session_id: "root",
        events: &[],
        external_client: None,
        embedder: Rc::new(Documents(served)),
        max_commands: 64,
        snapshots: SnapshotRequest::none(),
    })
}

/// States of each document [`wide_invocations`] serves.
const WIDE_STATES: usize = 200;

/// Fuel of one host call in the compile proof: one wide document's start,
/// compile included, fits it; four compiles do not.
const COMPILE_FUEL: u64 = 120_000_000;

/// A document the embedder serves is compiled on a fuel budget of its own,
/// not the invoking call's: a root whose start invokes four wide documents,
/// whose compiles together exceed the call's fuel, starts them all.
#[test]
fn the_documents_a_call_compiles_spend_their_own_fuel() {
    let one = wide_invocations(1, COMPILE_FUEL).expect("one wide child fits");
    assert_eq!(one.terminal, Terminal::Running);
    let four = wide_invocations(4, COMPILE_FUEL).expect("the compiles spend their own fuel");
    assert_eq!(four.terminal, Terminal::Running);
}

/// A chain of `depth` levels that each send `up` to their invoking
/// session on entry and invoke the next level; every session logs each
/// `up` it receives.
fn reporting_chain(depth: usize) -> Vec<(&'static str, String)> {
    (1..=depth)
        .map(|level| {
            let invoke = if level == depth {
                String::new()
            } else {
                format!(
                    r#"<invoke id="k" type="scxml" src="level{}.scxml"/>"#,
                    level + 1
                )
            };
            let body = format!(
                r##"<state id="c0"><onentry><send event="up" target="#_parent"/></onentry>{invoke}<transition event="up"><log label="up"/></transition></state>"##
            );
            let uri: &'static str = Box::leak(format!("level{level}.scxml").into_boxed_str());
            (uri, scxml("c0", &body))
        })
        .collect()
}

/// A [`DEEP`] chain starts inside the root's start with every level's `up`
/// to its invoking session queued at once: every `up` is delivered, and the
/// run takes exactly four commands a level (and two for the root), so no
/// queued item is taken more than once. The order the scheduler takes them
/// in, and its cost per item, are proven in `crates/scxml-scheduler`.
#[test]
fn a_chain_to_the_name_bound_with_every_level_queued_drains_in_bounded_commands() {
    let root = scxml(
        "r0",
        r#"<state id="r0"><onentry><send event="stop" delay="1ms"/></onentry><invoke id="k" type="scxml" src="level1.scxml"/><transition event="up"><log label="up"/></transition><transition event="stop" target="rdone"/></state><final id="rdone"/>"#,
    );
    let outcome = run_with_budget(
        &host_with_fuel(DEEP_CHAIN_FUEL),
        &root,
        reporting_chain(DEEP),
        SnapshotRequest::none(),
        10 * DEEP as u64,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    let ups = logs(&outcome)
        .iter()
        .filter(|log| log.ends_with(":up"))
        .count();
    assert_eq!(ups, DEEP, "every level's up reached its invoking session");
    assert_eq!(outcome.commands, 4 * DEEP as u64 + 2);
}

/// XML Base: an `<invoke src>` resolves against the `<invoke>`'s in-scope
/// `xml:base` (each resolved against its parent's), so the embedder is asked
/// for the URI the bases name, not the bare `src`.
#[test]
fn xml_base_changes_the_resolution_of_a_relative_invoke_src() {
    let root = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="null" initial="r0" xml:base="https://example.org/machines/"><state id="r0" xml:base="sub/"><invoke id="child" type="scxml" src="../children/child.scxml"/><transition event="done.invoke.child" target="rdone"><log label="root-got-done"/></transition></state><final id="rdone"/></scxml>"#;
    let leaf = scxml(
        "c0",
        r#"<state id="c0"><onentry><log label="child-ran"/></onentry><transition target="cdone"/></state><final id="cdone"/>"#,
    );
    let outcome = run(
        &host(),
        root,
        vec![(
            "https://example.org/machines/children/child.scxml",
            leaf.clone(),
        )],
        SnapshotRequest::none(),
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert!(
        logs(&outcome).iter().any(|log| log.ends_with(":child-ran")),
        "{:?}",
        logs(&outcome)
    );
    // Without the bases the same `src` names another document, which the
    // embedder does not have: the invocation fails and the root never
    // finishes.
    let unbased = root
        .replace(r#" xml:base="https://example.org/machines/""#, "")
        .replace(r#" xml:base="sub/""#, "");
    let outcome = run(
        &host(),
        &unbased,
        vec![("https://example.org/machines/children/child.scxml", leaf)],
        SnapshotRequest::none(),
    );
    assert!(
        !logs(&outcome).iter().any(|log| log.ends_with(":child-ran")),
        "{:?}",
        logs(&outcome)
    );
}

/// A data-model package's component is the data model of every session whose
/// `datamodel` names it: `datamodels/example` repackages the null data
/// model's component under the name `example`.
#[test]
fn a_document_runs_on_a_data_model_package() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let _ = components();
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        workspace.join("target/session-host-proof/wasm-artifacts/nested-invocation")
    });
    let bytes = std::fs::read(release.join("scxml_data_model_null.virtualized.wasm"))
        .expect("the release null data model is readable");
    let component = scxml_host_wasmtime::authenticate_data_model_component(
        &bytes,
        scxml_host_wasmtime::TrustedComponentDigest::from_hex(&format!(
            "{:x}",
            Sha256::digest(&bytes)
        ))
        .expect("digest is canonical"),
    )
    .expect("the component authenticates");
    let manifest = scxml_validator_core::DataModelPackageManifest::from_json(
        &std::fs::read(workspace.join("datamodels/example/package/example.data-model.json"))
            .expect("the example manifest is readable"),
    )
    .expect("the example manifest parses");
    let host = host()
        .with_data_model_components(
            scxml_host_wasmtime::DataModelComponents::new()
                .bind(&manifest.datamodel, &component)
                .expect("the package binds"),
        )
        .expect("the host takes the data models");
    let root = format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:datamodel="{}" version="1.0" datamodel="example" datamodel:src="{}" initial="a"><state id="a"><transition cond="In('a')" target="done"><log label="in-a"/></transition></state><final id="done"/></scxml>"#,
        scxml_types::DATAMODEL_NAMESPACE_URI,
        manifest.data_model_uri
    );
    let outcome = run(&host, &root, Vec::new(), SnapshotRequest::none());
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(logs(&outcome), ["root:in-a"]);
    // A built-in name is never a package's.
    assert!(
        scxml_host_wasmtime::DataModelComponents::new()
            .bind("null", &component)
            .is_err()
    );
}

/// One document's schema files: each reference and the file's text.
type SchemaFiles = Vec<(&'static str, &'static str)>;

/// Documents keyed by URI, each with the schema files its embedder read for
/// it ([`EmbeddedDocument::event_schemas`]); no scripts.
struct DocumentsWithSchemas(Vec<(&'static str, String, SchemaFiles)>);

impl SessionEmbedder for DocumentsWithSchemas {
    fn document(&self, _kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        let (_, text, schemas) = self.0.iter().find(|(key, _, _)| *key == uri)?;
        let mut document = EmbeddedDocument::from(text.clone());
        document.event_schemas = schemas
            .iter()
            .map(|(reference, bytes)| ((*reference).to_owned(), bytes.as_bytes().to_vec()))
            .collect();
        Some(document)
    }

    fn scripts(&self, _base_uri: Option<&str>) -> MachineScripts {
        MachineScripts::none()
    }
}

/// An `<invoke src>` document whose transitions reference `event:schema`
/// files compiles with the files its embedder supplies, and its sessions
/// run; without them (and for an inline `<content>` document, which never
/// gets them) the schema reference rejects it and its `<invoke>` raises
/// `error.execution` in the invoking session.
#[test]
fn an_invoke_src_document_compiles_with_the_schema_files_its_embedder_supplies() {
    const EVENT: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";
    let child = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" initial="c0">
              <state id="c0">
                <onentry><send target="#_parent" event="ready"/></onentry>
                <transition event="go" event:schema="schemas/go.json#/$defs/Go" target="cdone"><log label="child-got-go"/></transition>
              </state>
              <final id="cdone"/>
            </scxml>"##
    );
    let root = |invoke: &str| {
        format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="r0">
                  <state id="r0">
                    {invoke}
                    <transition event="ready"><send target="#_child" event="go"><content>{{"n": 1}}</content></send></transition>
                    <transition event="done.invoke.child" target="pass"/>
                    <transition event="error.execution" target="fail"/>
                  </state>
                  <final id="pass"><onentry><log label="pass"/></onentry></final>
                  <final id="fail"><onentry><log label="fail"/></onentry></final>
                </scxml>"##
        )
    };
    let schema = r#"{"$defs": {"Go": {"type": "object", "required": ["n"]}}}"#;
    let run_with = |root: &str, documents: DocumentsWithSchemas| {
        let (ir, data_model) = lower(root);
        host()
            .run(&SessionRun {
                machine_ir: &ir,
                data_model: data_model.as_deref(),
                session_id: "root",
                events: &[],
                external_client: None,
                embedder: Rc::new(documents),
                max_commands: 1_024,
                snapshots: SnapshotRequest::none(),
            })
            .unwrap_or_else(|error| panic!("the run completes: {error}"))
    };
    let labels = |outcome: &SessionOutcome| {
        logs(outcome)
            .into_iter()
            .map(|log| log.split_once(':').map(|(_, label)| label.to_owned()))
            .collect::<Option<Vec<_>>>()
            .expect("every log is session:label")
    };
    let src = r#"<invoke id="child" type="scxml" src="child.scx"/>"#;

    let supplied = run_with(
        &root(src),
        DocumentsWithSchemas(vec![(
            "child.scx",
            child.clone(),
            vec![("schemas/go.json", schema)],
        )]),
    );
    assert_eq!(supplied.terminal, Terminal::Finished);
    assert_eq!(labels(&supplied), ["child-got-go", "pass"]);

    let missing = run_with(
        &root(src),
        DocumentsWithSchemas(vec![("child.scx", child.clone(), Vec::new())]),
    );
    assert_eq!(missing.terminal, Terminal::Finished);
    assert_eq!(
        labels(&missing),
        ["fail"],
        "the reference rejects the child"
    );

    // An inline `<content>` document gets no schema files, even when an
    // `<invoke src>` document loaded before it carried some; the same
    // document with the schema inline runs.
    let inline = |child: &str| {
        format!(r#"<invoke id="child" type="scxml"><content>{child}</content></invoke>"#)
    };
    let with_src_schemas = || {
        DocumentsWithSchemas(vec![(
            "child.scx",
            child.clone(),
            vec![("schemas/go.json", schema)],
        )])
    };
    let content = run_with(&root(&inline(&child)), with_src_schemas());
    assert_eq!(content.terminal, Terminal::Finished);
    assert_eq!(labels(&content), ["fail"], "inline content gets no files");
    let inline_schema = child.replace(
        "schemas/go.json#/$defs/Go",
        r"{&quot;type&quot;: &quot;object&quot;, &quot;required&quot;: [&quot;n&quot;]}",
    );
    let content = run_with(&root(&inline(&inline_schema)), with_src_schemas());
    assert_eq!(content.terminal, Terminal::Finished);
    assert_eq!(labels(&content), ["child-got-go", "pass"]);
}

/// An `<invoke src>` document keeps its own schema files' contracts at run
/// time: an event it sends itself with data a transition's file schema
/// rejects is not dispatched and raises `error.execution` in the child, and
/// accepted data is delivered; the invoking session's `#_child` send is
/// judged by the same file schema.
#[test]
fn an_invoke_src_document_checks_the_events_it_sends_itself_against_its_schema_files() {
    const EVENT: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";
    let child = |data: &str| {
        format!(
            r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" initial="c0">
              <state id="c0">
                <onentry><send event="self"><content>{data}</content></send></onentry>
                <transition event="self" event:schema="schemas/go.json#/$defs/Go" target="c1"><log label="child-accepted"/></transition>
                <transition event="error.execution" target="c1"><log label="child-rejected"/></transition>
              </state>
              <state id="c1">
                <onentry><send target="#_parent" event="ready"/></onentry>
                <transition event="go" event:schema="schemas/go.json#/$defs/Go" target="cdone"><log label="child-got-go"/></transition>
              </state>
              <final id="cdone"/>
            </scxml>"##
        )
    };
    let root = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="r0">
          <state id="r0">
            <invoke id="child" type="scxml" src="child.scx"/>
            <transition event="ready"><send target="#_child" event="go"><content>{"n": 2}</content></send></transition>
            <transition event="done.invoke.child" target="pass"/>
            <transition event="error.execution" target="fail"/>
          </state>
          <final id="pass"><onentry><log label="pass"/></onentry></final>
          <final id="fail"><onentry><log label="fail"/></onentry></final>
        </scxml>"##;
    let schema = r#"{"$defs": {"Go": {"type": "object", "required": ["n"]}}}"#;
    let (ir, data_model) = lower(root);
    for (data, expected) in [
        (r#"{"n": 1}"#, ["child-accepted", "child-got-go", "pass"]),
        (r#"{"m": 1}"#, ["child-rejected", "child-got-go", "pass"]),
    ] {
        let outcome = host()
            .run(&SessionRun {
                machine_ir: &ir,
                data_model: data_model.as_deref(),
                session_id: "root",
                events: &[],
                external_client: None,
                embedder: Rc::new(DocumentsWithSchemas(vec![(
                    "child.scx",
                    child(data),
                    vec![("schemas/go.json", schema)],
                )])),
                max_commands: 1_024,
                snapshots: SnapshotRequest::none(),
            })
            .unwrap_or_else(|error| panic!("the run completes: {error}"));
        assert_eq!(outcome.terminal, Terminal::Finished, "{data}");
        let labels = logs(&outcome)
            .into_iter()
            .map(|log| log.split_once(':').map(|(_, label)| label.to_owned()))
            .collect::<Option<Vec<_>>>()
            .expect("every log is session:label");
        assert_eq!(labels, expected, "{data}");
    }
}

/// A `#_parent` send has no payload bound (SCXML sets none): data and the
/// raw message it renders reach the parent whatever their size (a send of
/// about 48 KiB used to be the child's `error.execution`, its data and raw
/// message counted together against a 96 KiB event bound).
#[test]
fn a_large_send_reaches_the_parent() {
    let child = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="c0">
          <state id="c0">
            <onentry>
              <send target="#_parent" event="small"><param name="choices" expr="'x'.repeat(40000)"/></send>
              <send target="#_parent" event="large"><param name="choices" expr="'x'.repeat(60000) + 'x'.repeat(60000)"/></send>
            </onentry>
            <transition target="cdone"/>
            <transition event="error.execution" target="cdone"><log label="child-error"/></transition>
          </state>
          <final id="cdone"/>
        </scxml>"##;
    let root = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="r0">
          <state id="r0">
            <invoke id="child" type="scxml" src="child.scx"/>
            <transition event="small"><log label="small"/></transition>
            <transition event="large" cond="_event.data.choices.length === 120000"><log label="large"/></transition>
            <transition event="done.invoke.child" target="pass"/>
          </state>
          <final id="pass"><onentry><log label="pass"/></onentry></final>
        </scxml>"#;
    let (ir, data_model) = lower(root);
    let outcome = host()
        .run(&SessionRun {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(DocumentsWithSchemas(vec![(
                "child.scx",
                child.to_owned(),
                Vec::new(),
            )])),
            max_commands: 1_024,
            snapshots: SnapshotRequest::none(),
        })
        .unwrap_or_else(|error| panic!("the run completes: {error}"));
    assert_eq!(outcome.terminal, Terminal::Finished);
    let labels = logs(&outcome)
        .into_iter()
        .map(|log| log.split_once(':').map(|(_, label)| label.to_owned()))
        .collect::<Option<Vec<_>>>()
        .expect("every log is session:label");
    assert_eq!(labels, ["small", "large", "pass"]);
}

/// Events between sessions keep the receiving document's contracts: a
/// `#_parent` or `#_child` send whose data a transition of the receiving
/// document handling it rejects (`accepts-event-data`) is not routed and
/// raises `error.execution` in the sender; accepted data is routed; a
/// delayed one refused when it falls due is the sender's
/// `error.communication`.
#[test]
fn a_send_between_sessions_keeps_the_receiving_documents_contracts() {
    const EVENT: &str = "https://github.com/stateforward/scxml.rs/namespaces/event";
    let child = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" datamodel="ecmascript" initial="c0">
          <state id="c0">
            <onentry>
              <send target="#_parent" event="up"><content>"bad"</content></send>
              <log label="child-unreached"/>
            </onentry>
            <transition event="error.execution" target="c1"><log label="child-up-rejected"/></transition>
          </state>
          <state id="c1">
            <onentry>
              <send target="#_parent" event="up"><content>{{"n": 1}}</content></send>
              <send target="#_parent" event="later" delay="10ms"><content>"bad"</content></send>
            </onentry>
            <transition event="error.communication"><log label="child-later-undeliverable"/></transition>
            <transition event="down" event:schema='{{"type": "object", "required": ["n"]}}' target="cdone"><log label="child-got-down"/></transition>
          </state>
          <final id="cdone"/>
        </scxml>"##
    );
    let root = format!(
        r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="{EVENT}" version="1.0" datamodel="ecmascript" initial="r">
          <state id="r" initial="r0">
            <invoke id="child" type="scxml" src="child.scx"/>
            <state id="r0">
              <transition event="up" event:schema='{{"type": "object", "required": ["n"]}}'>
                <log label="root-got-up"/>
                <send target="#_child" event="down"><content>"bad"</content></send>
                <log label="root-unreached"/>
              </transition>
              <transition event="later" event:schema='{{"type": "object"}}'><log label="root-got-later"/></transition>
              <transition event="error.execution" target="r1"><log label="root-down-rejected"/></transition>
            </state>
            <state id="r1">
              <onentry><send target="#_child" event="down" delay="20ms"><content>{{"n": 3}}</content></send></onentry>
            </state>
            <transition event="done.invoke.child" target="pass"/>
          </state>
          <final id="pass"><onentry><log label="pass"/></onentry></final>
        </scxml>"##
    );
    let (ir, data_model) = lower(&root);
    let outcome = host()
        .run(&SessionRun {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(DocumentsWithSchemas(vec![(
                "child.scx",
                child.clone(),
                Vec::new(),
            )])),
            max_commands: 1_024,
            snapshots: SnapshotRequest::none(),
        })
        .unwrap_or_else(|error| panic!("the run completes: {error}"));
    assert_eq!(outcome.terminal, Terminal::Finished, "{:?}", logs(&outcome));
    let labels = logs(&outcome)
        .into_iter()
        .map(|log| log.split_once(':').map(|(_, label)| label.to_owned()))
        .collect::<Option<Vec<_>>>()
        .expect("every log is session:label");
    assert_eq!(
        labels,
        [
            "child-up-rejected",
            "root-got-up",
            "root-down-rejected",
            "child-later-undeliverable",
            "child-got-down",
            "pass"
        ]
    );
}

/// The command budget of a run with runaway sessions: each runaway call
/// charges the machine's 1 024 commands to it.
const RUNAWAY_COMMANDS: u64 = 100_000;

/// An invoked document whose state `loop` raises an event that re-enters
/// it, without end: a call that enters `loop` stops at the machine's
/// per-call command bound. `before` is the content of its enclosing state
/// `c0` ahead of the states `idle` (its initial state) and `loop`.
fn runaway(before: &str, idle: &str) -> String {
    scxml(
        "c0",
        &format!(
            r#"<state id="c0" initial="idle">
              {before}
              <onexit><log label="child-exit"/></onexit>
              <state id="idle">{idle}</state>
              <state id="loop">
                <onentry><raise event="again"/></onentry>
                <transition event="again" target="loop"/>
              </state>
            </state>"#
        ),
    )
}

/// A grandchild that tells its invoking session it started and logs when
/// it is cancelled.
fn cancelled_grandchild() -> String {
    scxml(
        "g0",
        r##"<state id="g0">
              <onentry><send target="#_parent" event="ready"/></onentry>
              <onexit><log label="grandchild-exit"/></onexit>
            </state>"##,
    )
}

/// The labels of `outcome`'s logs, without their sessions.
fn labels(outcome: &SessionOutcome) -> Vec<String> {
    logs(outcome)
        .iter()
        .map(|log| {
            log.split_once(':')
                .map(|(_, label)| label.to_owned())
                .unwrap_or_default()
        })
        .collect()
}

/// An invoked session whose start stops at its command bound (its initial
/// state raises an event that re-enters it forever) fails only its
/// `<invoke>`: it is cancelled (its exit runs) and dropped, the invoking
/// session receives `error.execution` and no `done.invoke`, and the run
/// goes on to its pass state (SCXML 6.4).
#[test]
fn an_invoked_session_whose_start_runs_away_fails_its_invoke_with_error_execution() {
    let child = scxml(
        "c0",
        r#"<state id="c0">
              <onentry><raise event="again"/></onentry>
              <onexit><log label="child-exit"/></onexit>
              <transition event="again" target="c0"/>
            </state>"#,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="error.execution" target="pass"><log label="root-got-error"/></transition>
              <transition event="done.invoke.child" target="fail"/>
            </state>
            <final id="pass"/>
            <final id="fail"/>"#,
    );
    let outcome = run_with_budget(
        &host(),
        &root,
        vec![("child.scxml", child)],
        SnapshotRequest::none(),
        RUNAWAY_COMMANDS,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    let labels = labels(&outcome);
    assert_eq!(labels.last().map(String::as_str), Some("root-got-error"));
    assert!(
        labels[..labels.len() - 1]
            .iter()
            .all(|label| label == "child-exit"),
        "{labels:?}"
    );
}

/// A session that stops at its command bound in its start after it
/// invoked a grandchild (a second `<invoke>` that fails takes it into the
/// loop) is cancelled as its invoking session's cancel would cancel it:
/// its exit runs, then the grandchild's, and its own invoking session
/// receives `error.execution`.
#[test]
fn a_runaway_start_cancels_the_sessions_the_runaway_invoked() {
    let child = runaway(
        r#"<invoke id="gc" type="scxml" src="grandchild.scxml"/>
           <invoke id="missing" type="scxml" src="missing.scxml"/>"#,
        r#"<transition event="error.execution" target="loop"/>"#,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="error.execution" target="pass"><log label="root-got-error"/></transition>
              <transition event="done.invoke.child" target="fail"/>
              <transition event="ready" target="fail"/>
            </state>
            <final id="pass"/>
            <final id="fail"/>"#,
    );
    let outcome = run_with_budget(
        &host(),
        &root,
        vec![
            ("child.scxml", child),
            ("grandchild.scxml", cancelled_grandchild()),
        ],
        SnapshotRequest::none(),
        RUNAWAY_COMMANDS,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    assert_eq!(
        labels(&outcome),
        ["child-exit", "grandchild-exit", "root-got-error"]
    );
}

/// A running invoked session that stops at its command bound later (on an
/// event from the grandchild it invoked) is ended: its cancel runs,
/// cancelling the grandchild, nothing it sends reaches its invoking
/// session, no `done.invoke` follows, and its invoking session receives
/// `error.communication.<invokeid>`, as for an invoked service that fails
/// after its start (no `<invoke>` start is pending to fail).
#[test]
fn a_running_invoked_session_that_runs_away_is_ended_with_error_communication() {
    let child = runaway(
        r#"<invoke id="gc" type="scxml" src="grandchild.scxml"/>"#,
        r##"<transition event="ready" target="loop"><send target="#_parent" event="looping"/></transition>"##,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="error.communication.child" target="r1"><log label="root-got-error"/></transition>
              <transition event="done.invoke.child" target="fail"/>
              <transition event="error.execution" target="fail"/>
            </state>
            <state id="r1">
              <onentry><send event="quiet" delay="10ms"/></onentry>
              <transition event="quiet" target="pass"/>
              <transition event="done.invoke.child" target="fail"/>
            </state>
            <final id="pass"/>
            <final id="fail"/>"#,
    );
    let outcome = run_with_budget(
        &host(),
        &root,
        vec![
            ("child.scxml", child),
            ("grandchild.scxml", cancelled_grandchild()),
        ],
        SnapshotRequest::none(),
        RUNAWAY_COMMANDS,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    // `looping` was sent inside the call that ran away: it is queued for
    // the root before the error, and the root ignores it.
    assert_eq!(
        labels(&outcome),
        ["child-exit", "grandchild-exit", "root-got-error"]
    );
}

/// The root session's own runaway still fails the run with
/// `RunFailure::CommandBound`, after an invoked session's runaway was
/// contained.
#[test]
fn the_root_s_own_runaway_still_fails_the_run_with_command_bound() {
    let child = runaway(
        "",
        r#"<onentry><raise event="go"/></onentry><transition event="go" target="loop"/>"#,
    );
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="error.execution" target="r1"/>
            </state>
            <state id="r1">
              <onentry><raise event="spin"/></onentry>
              <transition event="spin" target="r1"/>
            </state>"#,
    );
    let (ir, data_model) = lower(&root);
    let error = host()
        .run(&SessionRun {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(Documents(vec![("child.scxml", child)])),
            max_commands: RUNAWAY_COMMANDS,
            snapshots: SnapshotRequest::none(),
        })
        .expect_err("the root's runaway fails the run");
    assert!(
        matches!(error.failure, RunFailure::CommandBound),
        "{:?}",
        error.failure
    );
}

/// A recorded live session in which an invoked session runs away, at its
/// start and later on an event the root forwards, replays to the same
/// snapshots and outputs: the containment is a pure function of the run.
#[test]
fn a_recorded_run_with_runaway_invoked_sessions_replays_byte_for_byte() {
    let at_start = scxml(
        "c0",
        r#"<state id="c0">
              <onentry><raise event="again"/></onentry>
              <transition event="again" target="c0"/>
            </state>"#,
    );
    let later = runaway(
        r#"<invoke id="gc" type="scxml" src="grandchild.scxml"/>"#,
        r#"<transition event="go" target="loop"/>"#,
    );
    let root = scxml(
        "r0",
        r##"<state id="r0">
              <invoke id="first" type="scxml" src="at-start.scxml"/>
              <invoke id="second" type="scxml" src="later.scxml"/>
              <transition event="error.execution"><log label="first-failed"/></transition>
              <transition event="go"><send target="#_second" event="go"/></transition>
              <transition event="error.communication.second" target="pass"><log label="second-ended"/></transition>
            </state>
            <final id="pass"/>"##,
    );
    let (ir, data_model) = lower(&root);
    let documents = || {
        Rc::new(Documents(vec![
            ("at-start.scxml", at_start.clone()),
            ("later.scxml", later.clone()),
            ("grandchild.scxml", cancelled_grandchild()),
        ]))
    };
    let host = host();
    let open = |clock, recording: Recording| {
        host.open(SessionOpen {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            embedder: documents(),
            max_commands: RUNAWAY_COMMANDS,
            clock,
            record: recording == Recording::On,
            replay: None,
            http: LiveHttp::default(),
        })
        .expect("the session opens")
    };
    let clock = ManualClock::new();
    let mut live = open(LiveClock::Timed(Box::new(clock.clone())), Recording::On);
    let options = Options::default();
    let mut snapshots = vec![live.snapshot(&options).expect("snapshot")];
    clock.set(5);
    let step = live
        .send(&ScriptedEvent::named("go"))
        .expect("the second child's runaway is contained");
    assert_eq!(step.status, LiveStatus::Finished);
    snapshots.push(live.snapshot(&options).expect("snapshot"));
    let outputs = live.take_outputs();
    let labels: Vec<&str> = outputs
        .iter()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => entry.label.as_deref(),
            SessionOutput::Done { .. } => None,
        })
        .collect();
    assert_eq!(
        labels,
        [
            "first-failed",
            "child-exit",
            "grandchild-exit",
            "second-ended"
        ],
        "{outputs:?}"
    );
    let records = live.take_recorded();
    live.close().expect("close");

    let mut replay = open(LiveClock::Logical, Recording::Off);
    let mut replayed = Vec::new();
    for record in &records {
        if let Applied::Snapshot(bytes) = replay.apply(record).expect("the record applies") {
            replayed.push(bytes);
        }
    }
    assert_eq!(replayed, snapshots, "the snapshots replay byte for byte");
    assert_eq!(replay.take_outputs(), outputs);
    replay.close().expect("close");
}

/// An ECMAScript document whose eventless transition's cond throws on
/// every microstep (a syntax error): each evaluation is false and raises
/// `error.execution`, and the eventless transition is selected again,
/// without end, so a call that enters `a` stops at the machine's per-call
/// command bound (SCXML 5.9.1, 3.13).
const THROWING_COND: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="a"><transition cond="not x" target="b"/></state><final id="b"/></scxml>"#;

/// An invoked session whose eventless cond throws on every microstep
/// stops at its command bound in its start and fails only its `<invoke>`
/// with `error.execution`; the run goes on to its pass state.
#[test]
fn an_invoked_session_whose_eventless_cond_throws_fails_its_invoke_with_error_execution() {
    let root = scxml(
        "r0",
        r#"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="error.execution" target="pass"><log label="root-got-error"/></transition>
              <transition event="done.invoke.child" target="fail"/>
            </state>
            <final id="pass"/>
            <final id="fail"/>"#,
    );
    let outcome = run_with_budget(
        &host(),
        &root,
        vec![("child.scxml", THROWING_COND.to_owned())],
        SnapshotRequest::none(),
        RUNAWAY_COMMANDS,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    assert_eq!(labels(&outcome), ["root-got-error"]);
}

/// A running invoked session that enters an eventless cond that throws on
/// every microstep later (on an event the root sends it once it is
/// ready) is ended, and its
/// invoking session receives `error.communication.<invokeid>`.
#[test]
fn a_running_invoked_session_whose_eventless_cond_throws_is_ended_with_error_communication() {
    let child = r##"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><state id="idle"><onentry><send target="#_parent" event="ready"/></onentry><transition event="go" target="a"/></state><state id="a"><transition cond="not x" target="b"/></state><final id="b"/></scxml>"##;
    let root = scxml(
        "r0",
        r##"<state id="r0">
              <invoke id="child" type="scxml" src="child.scxml"/>
              <transition event="ready"><send event="go" target="#_child"/></transition>
              <transition event="error.communication.child" target="pass"><log label="root-got-error"/></transition>
              <transition event="done.invoke.child" target="fail"/>
              <transition event="error.execution" target="fail"/>
            </state>
            <final id="pass"/>
            <final id="fail"/>"##,
    );
    let outcome = run_with_budget(
        &host(),
        &root,
        vec![("child.scxml", child.to_owned())],
        SnapshotRequest::none(),
        RUNAWAY_COMMANDS,
    );
    assert_eq!(outcome.terminal, Terminal::Finished);
    assert_eq!(outcome.configuration, ["pass"]);
    assert_eq!(labels(&outcome), ["root-got-error"]);
}

/// The root session's own eventless cond that throws on every microstep
/// fails the run with `RunFailure::CommandBound`.
#[test]
fn a_root_whose_eventless_cond_throws_fails_the_run_with_command_bound() {
    let (ir, data_model) = lower(THROWING_COND);
    let error = host()
        .run(&SessionRun {
            machine_ir: &ir,
            data_model: data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: RUNAWAY_COMMANDS,
            snapshots: SnapshotRequest::none(),
        })
        .expect_err("the root's throwing cond fails the run");
    assert!(
        matches!(error.failure, RunFailure::CommandBound),
        "{:?}",
        error.failure
    );
}
