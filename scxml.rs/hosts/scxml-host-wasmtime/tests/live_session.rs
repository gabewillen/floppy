//! Live sessions of the Wasmtime session host (`SessionHost::open`) run
//! real documents through the shipped components: events processed one at
//! a time, delayed sends fired on a wall, manual, or logical clock, streamed
//! outputs, graceful close, and record/replay.
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

use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use scxml_host_wasmtime::scxml_snapshot::Options;
use scxml_host_wasmtime::{
    Applied, DocumentKind, EmbeddedDocument, InputRecord, LiveClock, LiveHttp, LiveSession,
    LiveStatus, LogEntry, MachineScripts, ManualClock, ReleaseComponents, RunFailure,
    ScriptedEvent, SessionEmbedder, SessionHost, SessionOpen, SessionOutput, SessionRun,
    SnapshotPoints, SnapshotRequest, TelemetryProviders, Terminal, TrustedManifestDigest,
    WallClock, WasmtimeHostConfig,
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
            let release = target.join("wasm-artifacts/live-session");
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

fn lower(source: &str) -> (Vec<u8>, Option<String>) {
    let document =
        scxml_syntax::parse_xml(scxml_syntax::SourceText::new(SOURCE, "live.scxml", source))
            .expect("fixture parses");
    let ir = scxml_compiler_core::lower(&document).expect("fixture lowers");
    let data_model = ir.data_model.clone();
    (ir.to_json().expect("fixture IR encodes"), data_model)
}

fn scxml(data_model: &str, body: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="{data_model}" initial="s0">{body}</scxml>"#
    )
}

/// Whether a live session records its inputs.
#[derive(Clone, Copy, PartialEq)]
enum Recording {
    On,
    Off,
}

struct Opened {
    ir: Vec<u8>,
    data_model: Option<String>,
}

impl Opened {
    fn new(source: &str) -> Self {
        let (ir, data_model) = lower(source);
        Self { ir, data_model }
    }

    fn open(&self, host: &SessionHost, clock: LiveClock, recording: Recording) -> LiveSession {
        self.open_with(host, clock, recording, Vec::new(), 256)
            .expect("the session opens")
    }

    fn open_with(
        &self,
        host: &SessionHost,
        clock: LiveClock,
        recording: Recording,
        documents: Vec<(&'static str, String)>,
        max_commands: u64,
    ) -> Result<LiveSession, scxml_host_wasmtime::SessionHostError> {
        host.open(SessionOpen {
            machine_ir: &self.ir,
            data_model: self.data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(Documents(documents)),
            max_commands,
            clock,
            record: recording == Recording::On,
            replay: None,
            http: LiveHttp::default(),
        })
    }
}

fn log_labels(outputs: &[SessionOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => Some(entry.label.clone().unwrap_or_default()),
            SessionOutput::Done { .. } => None,
        })
        .collect()
}

const DELAYED: &str = r#"<state id="s0"><onentry><send event="tick" delay="100ms" id="t"/><log label="scheduled"/></onentry><transition event="tick" target="done"><log label="ticked"/></transition><transition event="ping"><log label="ping"/></transition></state><final id="done"/>"#;

#[test]
fn a_logical_session_matches_the_batch_run() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><transition event="go" target="s1"><assign location="n" expr="n + 1"/><log label="go" expr="n"/></transition></state><state id="s1"><transition event="go" target="s0"><assign location="n" expr="n + 10"/></transition><transition event="stop" target="done"/></state><final id="done"/>"#,
    );
    let opened = Opened::new(&source);
    let events = ["go", "go", "go"];
    let scripted: Vec<ScriptedEvent> = events.iter().copied().map(ScriptedEvent::named).collect();
    let batch = host
        .run(&SessionRun {
            machine_ir: &opened.ir,
            data_model: opened.data_model.as_deref(),
            session_id: "root",
            events: &scripted,
            external_client: None,
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 256,
            snapshots: SnapshotRequest {
                points: SnapshotPoints::Every,
                options: Options::default(),
            },
        })
        .expect("the batch run completes");
    let mut live = opened.open(&host, LiveClock::Logical, Recording::Off);
    for event in events {
        let step = live.send(&ScriptedEvent::named(event)).expect("send");
        assert_eq!(step.status, LiveStatus::Running);
        assert_eq!(step.at_ms, 0);
        assert_eq!(step.commands, 1, "each step charges its own budget");
    }
    live.run_until_idle().expect("nothing is pending");
    let snapshot = live.snapshot(&Options::default()).expect("snapshot");
    let end = batch.snapshots.last().expect("the end checkpoint");
    assert_eq!(
        snapshot, end.bytes,
        "live and batch end in the same document"
    );
    assert_eq!(
        live.configuration().expect("configuration"),
        batch.configuration
    );
    let outputs = live.take_outputs();
    assert_eq!(log_labels(&outputs), ["go", "go"]);
    let texts: Vec<_> = outputs
        .iter()
        .map(|output| match output {
            SessionOutput::Log { entry, .. } => entry.text.clone(),
            other @ SessionOutput::Done { .. } => panic!("unexpected output {other:?}"),
        })
        .collect();
    let batch_texts: Vec<_> = batch.root_logs.iter().map(|log| log.text.clone()).collect();
    assert_eq!(texts, batch_texts);
    let report = live.close().expect("close");
    assert_eq!(report.status, LiveStatus::Running);
    assert_eq!(report.cancelled_sends, 0);
}

#[test]
fn events_carry_data_and_invalid_or_late_events_leave_the_session_as_it_was() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<state id="s0"><transition event="go" target="done"><log label="data" expr="JSON.stringify(_event.data)"/></transition></state><final id="done"><donedata><content expr="'bye'"/></donedata></final>"#,
    );
    let mut live = Opened::new(&source).open(&host, LiveClock::Logical, Recording::Off);
    let invalid = live
        .send(&ScriptedEvent::named("not a name"))
        .expect_err("an invalid event name");
    assert!(matches!(
        invalid.failure,
        RunFailure::Harness {
            code: "event-name-invalid",
            ..
        }
    ));
    assert_eq!(live.status(), LiveStatus::Running);
    let data = scxml_types::DataValue::canonical_json_object(r#"{"answer":42}"#).expect("json");
    let step = live
        .send(&ScriptedEvent {
            data,
            ..ScriptedEvent::named("go")
        })
        .expect("go");
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(step.next_deadline, None);
    assert_eq!(
        live.take_outputs(),
        [
            SessionOutput::Log {
                session: "root".into(),
                parent: None,
                entry: LogEntry {
                    label: Some("data".into()),
                    text: r#"{"answer":42}"#.into(),
                },
            },
            // SCXML 5.5: the root's <donedata> goes to the environment.
            SessionOutput::Done {
                data: Some(scxml_types::DataValue::String("bye".into()))
            },
        ]
    );
    let late = live
        .send(&ScriptedEvent::named("go"))
        .expect_err("an event after the root finished");
    assert!(matches!(
        late.failure,
        RunFailure::Harness {
            code: "event-after-finish",
            ..
        }
    ));
    assert_eq!(live.status(), LiveStatus::Finished);
    assert!(live.take_outputs().is_empty(), "done streams once");
    live.close().expect("close");
}

#[test]
fn a_manual_clock_fires_delayed_sends_at_their_deadline() {
    let host = host();
    let clock = ManualClock::new();
    let mut live = Opened::new(&scxml("null", DELAYED)).open(
        &host,
        LiveClock::Timed(Box::new(clock.clone())),
        Recording::Off,
    );
    assert_eq!(log_labels(&live.take_outputs()), ["scheduled"]);
    assert_eq!(live.next_deadline().expect("deadline"), Some(100));
    clock.set(40);
    let step = live.send(&ScriptedEvent::named("ping")).expect("ping");
    assert_eq!(step.at_ms, 40, "the event is processed at its arrival time");
    assert_eq!(step.next_deadline, Some(100));
    assert_eq!(live.now_ms().expect("now"), 40);
    clock.set(99);
    let step = live.advance().expect("nothing is due yet");
    assert_eq!(step.status, LiveStatus::Running);
    assert_eq!(log_labels(&live.take_outputs()), ["ping"]);
    clock.set(100);
    let step = live.advance().expect("the deadline fires");
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(step.at_ms, 100);
    assert_eq!(log_labels(&live.take_outputs()), ["ticked"]);
    let timed = live
        .advance_to(200)
        .expect_err("a timed clock decides the time");
    assert!(matches!(
        timed.failure,
        RunFailure::Harness {
            code: "clock-timed",
            ..
        }
    ));
    live.close().expect("close");
}

#[test]
fn an_overdue_deadline_fires_before_a_later_event() {
    let host = host();
    let clock = ManualClock::new();
    let mut live = Opened::new(&scxml("null", DELAYED)).open(
        &host,
        LiveClock::Timed(Box::new(clock.clone())),
        Recording::Off,
    );
    clock.set(150);
    let late = live
        .send(&ScriptedEvent::named("ping"))
        .expect_err("the timer at 100 finished the root first");
    assert!(matches!(
        late.failure,
        RunFailure::Harness {
            code: "event-after-finish",
            ..
        }
    ));
    assert_eq!(live.status(), LiveStatus::Finished);
    assert_eq!(log_labels(&live.take_outputs()), ["scheduled", "ticked"]);
    live.close().expect("close");
}

#[test]
fn a_wall_clock_fires_a_real_delayed_send() {
    let host = host();
    let source = scxml(
        "null",
        r#"<state id="s0"><onentry><send event="tick" delay="30ms"/></onentry><transition event="tick" target="done"/></state><final id="done"/>"#,
    );
    let started = Instant::now();
    let clock = WallClock::start();
    let mut live =
        Opened::new(&source).open(&host, LiveClock::Timed(Box::new(clock)), Recording::Off);
    let deadline = live
        .next_deadline()
        .expect("deadline")
        .expect("the tick is scheduled");
    assert_eq!(deadline, 30);
    // The embedder's loop: wait for the deadline, then advance.
    let mut steps = 0;
    while live.status() == LiveStatus::Running {
        steps += 1;
        assert!(steps < 1_000, "the loop terminates");
        let Some(deadline) = live.next_deadline().expect("deadline") else {
            break;
        };
        let now = live.now_ms().expect("now");
        std::thread::sleep(Duration::from_millis(deadline.saturating_sub(now).max(1)));
        live.advance().expect("advance");
    }
    let elapsed = started.elapsed();
    assert_eq!(live.status(), LiveStatus::Finished);
    assert!(
        elapsed >= Duration::from_millis(30),
        "the send fired no earlier than its delay: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "the send fired near its delay: {elapsed:?}"
    );
    live.close().expect("close");
}

#[test]
fn a_recorded_timed_session_replays_byte_for_byte_on_the_logical_clock() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<datamodel><data id="n" expr="0"/></datamodel><state id="s0"><onentry><send event="tick" delay="100ms" id="t"/></onentry><transition event="ping"><assign location="n" expr="n + 1"/><send event="tock" delay="30ms"/><log label="ping" expr="n"/></transition><transition event="tock"><log label="tock"/></transition><transition event="tick" target="s1"/></state><state id="s1"><transition event="ping" target="done"/></state><final id="done"/>"#,
    );
    let opened = Opened::new(&source);
    let clock = ManualClock::new();
    let mut live = opened.open(
        &host,
        LiveClock::Timed(Box::new(clock.clone())),
        Recording::On,
    );
    let options = Options::default();
    let mut live_snapshots = Vec::new();
    let mut live_outputs = live.take_outputs();
    clock.set(10);
    live.send(&ScriptedEvent::named("ping")).expect("ping");
    live_snapshots.push(live.snapshot(&options).expect("snapshot"));
    clock.set(45);
    live.advance().expect("tock fires");
    live_snapshots.push(live.snapshot(&options).expect("snapshot"));
    clock.set(120);
    live.advance().expect("tick fires");
    clock.set(130);
    live.send(&ScriptedEvent::named("ping"))
        .expect("final ping");
    live_snapshots.push(live.snapshot(&options).expect("snapshot"));
    live_outputs.extend(live.take_outputs());
    let records = live.take_recorded();
    assert!(
        matches!(records.first(), Some(InputRecord::Start { .. })),
        "the clock's start is the first record"
    );
    assert_eq!(
        records.get(1),
        Some(&InputRecord::Event {
            at_ms: 10,
            event: ScriptedEvent::named("ping"),
        })
    );
    assert!(records.contains(&InputRecord::Advance { to_ms: 45 }));
    live.close().expect("close");

    let mut replay = opened.open(&host, LiveClock::Logical, Recording::Off);
    let mut replay_outputs = replay.take_outputs();
    let mut replay_snapshots = Vec::new();
    for record in &records {
        if let Applied::Snapshot(bytes) = replay.apply(record).expect("the record applies") {
            replay_snapshots.push(bytes);
        }
    }
    replay_outputs.extend(replay.take_outputs());
    assert_eq!(replay_snapshots.len(), 3);
    assert_eq!(
        replay_snapshots, live_snapshots,
        "snapshots replay byte for byte"
    );
    assert_eq!(replay_outputs, live_outputs, "outputs replay in order");
    assert_eq!(replay.status(), LiveStatus::Finished);
    replay.close().expect("close");
}

/// The start of the batch clock: 2015-09-01T00:00:00Z.
const BATCH_START_EPOCH_MS: u64 = 1_441_065_600_000;

/// ECMA-262 3rd edition 15.9.3.3: `new Date()` is "the time value (UTC)
/// identifying the current time": the session clock's start epoch plus its
/// logical time. A batch run starts at the fixed batch epoch; a timed live
/// session at the wall-clock time it opened, recorded as its first input;
/// a replay of that recording at the recorded start.
#[test]
fn date_reads_the_session_clock_from_its_start_epoch() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<state id="s0"><onentry><log label="start" expr="Date.now()"/><send event="tick" delay="100ms"/></onentry><transition event="ping"><log label="ping" expr="new Date().getTime()"/></transition><transition event="tick" target="done"><log label="tick" expr="Date.now()"/></transition></state><final id="done"/>"#,
    );
    let opened = Opened::new(&source);
    let batch = host
        .run(&SessionRun {
            machine_ir: &opened.ir,
            data_model: opened.data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 256,
            snapshots: SnapshotRequest {
                points: SnapshotPoints::None,
                options: Options::default(),
            },
        })
        .expect("the batch run completes");
    let texts: Vec<_> = batch.root_logs.iter().map(|log| log.text.clone()).collect();
    assert_eq!(
        texts,
        [
            BATCH_START_EPOCH_MS.to_string(),
            (BATCH_START_EPOCH_MS + 100).to_string()
        ]
    );

    let wall = || {
        u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("fits")
    };
    let before = wall();
    let clock = ManualClock::new();
    let mut live = opened.open(
        &host,
        LiveClock::Timed(Box::new(clock.clone())),
        Recording::On,
    );
    let after = wall();
    clock.set(10);
    live.send(&ScriptedEvent::named("ping")).expect("ping");
    let outputs = live.take_outputs();
    let records = live.take_recorded();
    let Some(InputRecord::Start { epoch_ms }) = records.first().cloned() else {
        panic!("the clock's start is the first record: {records:?}");
    };
    assert!(
        (before..=after).contains(&epoch_ms),
        "a timed session starts at the wall clock"
    );
    let live_texts: Vec<_> = outputs
        .iter()
        .filter_map(|output| match output {
            SessionOutput::Log { entry, .. } => Some(entry.text.clone()),
            SessionOutput::Done { .. } => None,
        })
        .collect();
    assert_eq!(
        live_texts,
        [epoch_ms.to_string(), (epoch_ms + 10).to_string()]
    );
    live.close().expect("close");

    let mut replay = host
        .open(SessionOpen {
            machine_ir: &opened.ir,
            data_model: opened.data_model.as_deref(),
            session_id: "root",
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 256,
            clock: LiveClock::Logical,
            record: false,
            replay: Some(&records),
            http: LiveHttp::default(),
        })
        .expect("the replay opens");
    let mut replay_outputs = replay.take_outputs();
    for record in &records {
        replay.apply(record).expect("the record applies");
    }
    replay_outputs.extend(replay.take_outputs());
    assert_eq!(
        replay_outputs, outputs,
        "the replay reads the recorded times"
    );
    replay.close().expect("close");
}

#[test]
fn close_exits_the_root_cancels_pending_sends_and_children() {
    let host = host();
    let child = scxml(
        "null",
        r#"<state id="s0"><onentry><send event="later" delay="1000ms"/></onentry></state>"#,
    );
    let source = scxml(
        "null",
        r#"<state id="s0"><onentry><send event="later" delay="500ms"/><send event="later2" delay="600ms"/></onentry><onexit><log label="exit"/></onexit><invoke type="scxml" src="child.scxml"/></state>"#,
    );
    let live = Opened::new(&source)
        .open_with(
            &host,
            LiveClock::Logical,
            Recording::Off,
            vec![("child.scxml", child)],
            256,
        )
        .expect("the session opens");
    let report = live.close().expect("close");
    assert_eq!(report.status, LiveStatus::Running);
    assert_eq!(report.cancelled_sends, 3, "two root sends and the child's");
    assert_eq!(report.cancelled_children, 1);
    assert_eq!(log_labels(&report.outputs), ["exit"]);
}

#[test]
fn each_step_has_its_own_budget_and_a_failed_step_fails_the_session() {
    let host = host();
    let source = scxml(
        "null",
        r#"<state id="s0"><transition event="go" target="s1"/><transition event="spin" target="s0"><raise event="spin"/></transition></state><state id="s1"><transition event="go" target="s0"/></state>"#,
    );
    let opened = Opened::new(&source);
    let mut live = opened
        .open_with(&host, LiveClock::Logical, Recording::Off, Vec::new(), 2)
        .expect("the session opens");
    for _ in 0..8 {
        live.send(&ScriptedEvent::named("go"))
            .expect("every step starts with a fresh budget");
    }
    let open = opened
        .open_with(&host, LiveClock::Logical, Recording::Off, Vec::new(), 0)
        .expect_err("a zero budget is rejected");
    assert!(matches!(
        open.failure,
        RunFailure::Harness {
            code: "command-budget-invalid",
            ..
        }
    ));
    let failed = live
        .send(&ScriptedEvent::named("spin"))
        .expect_err("an endless step fails");
    assert!(
        matches!(
            failed.failure,
            RunFailure::CommandBudget | RunFailure::CommandBound | RunFailure::Machine { .. }
        ),
        "{failed}"
    );
    assert_eq!(live.status(), LiveStatus::Failed);
    let after = live
        .send(&ScriptedEvent::named("go"))
        .expect_err("a failed session accepts nothing");
    assert!(matches!(
        after.failure,
        RunFailure::Harness {
            code: "session-failed",
            ..
        }
    ));
    let report = live.close().expect("a failed session still closes");
    assert_eq!(report.status, LiveStatus::Failed);
}

#[test]
fn basichttp_sends_outside_the_loopback_without_a_network_are_undeliverable() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<state id="s0"><onentry><send event="notify" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor" target="http://hooks.example.invalid/agent"><param name="answer" expr="42"/></send></onentry><transition event="error.communication" target="lost"/><transition event="go" target="done"/></state><final id="lost"/><final id="done"/>"#,
    );
    let opened = Opened::new(&source);
    let mut live = opened.open(&host, LiveClock::Logical, Recording::Off);
    // A live session without `LiveHttp` has no outbound transport: the send
    // fails at once, exactly as in a batch run.
    assert_eq!(live.status(), LiveStatus::Finished);
    assert!(!live.communication_pending());
    assert_eq!(live.take_outputs(), [SessionOutput::Done { data: None }]);
    let report = live.close().expect("close");
    assert_eq!(report.status, LiveStatus::Finished);

    // A batch run has no embedder route: the send is undeliverable.
    let batch = host
        .run(&SessionRun {
            machine_ir: &opened.ir,
            data_model: opened.data_model.as_deref(),
            session_id: "root",
            events: &[],
            external_client: None,
            embedder: Rc::new(Documents(Vec::new())),
            max_commands: 256,
            snapshots: SnapshotRequest::none(),
        })
        .expect("the batch run completes");
    assert_eq!(batch.terminal, Terminal::Finished);
    assert_eq!(batch.configuration, ["lost"]);
}

#[test]
fn a_finished_root_returns_its_donedata_to_the_environment() {
    // SCXML 5.5: "In other cases (namely when the <final> element is a child
    // of <scxml> and the state machine has not been triggered by <invoke>),
    // the SCXML Processor SHOULD return the data to the environment in an
    // implementation-dependent manner." The live session reports it on its
    // `Done` output.
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<state id="s0"><transition event="go" target="done"/></state><final id="done"><donedata><param name="answer" expr="42"/></donedata></final>"#,
    );
    let opened = Opened::new(&source);
    let mut live = opened.open(&host, LiveClock::Logical, Recording::Off);
    live.send(&ScriptedEvent::named("go")).expect("go");
    assert_eq!(live.status(), LiveStatus::Finished);
    let done: Vec<_> = live
        .take_outputs()
        .into_iter()
        .filter_map(|output| match output {
            SessionOutput::Done { data } => Some(data),
            SessionOutput::Log { .. } => None,
        })
        .collect();
    let [Some(scxml_types::DataValue::JsonObject(object))] = done.as_slice() else {
        panic!("one Done with the donedata object: {done:?}");
    };
    let object: serde_json::Value = serde_json::from_str(object).expect("canonical JSON");
    assert_eq!(object, serde_json::json!({ "answer": 42.0 }));
    live.close().expect("close");

    // Without <donedata> there is nothing to return.
    let plain = Opened::new(&scxml("ecmascript", r#"<final id="s0"/>"#));
    let mut live = plain.open(&host, LiveClock::Logical, Recording::Off);
    assert_eq!(live.take_outputs(), [SessionOutput::Done { data: None }]);
    live.close().expect("close");
}

/// The host's own input meets the root document's contracts: an event
/// whose data a transition handling it rejects (`event:schema`,
/// `accepts-event-data`) is rejected and leaves the session as it was; the
/// same event with accepted data is delivered.
#[test]
fn input_the_root_document_does_not_accept_is_rejected() {
    let host = host();
    let source = scxml(
        "ecmascript",
        r#"<state id="s0"><transition xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" event="go" event:schema='{"type": "object", "required": ["n"]}' target="done"><log label="data" expr="JSON.stringify(_event.data)"/></transition></state><final id="done"/>"#,
    );
    let mut live = Opened::new(&source).open(&host, LiveClock::Logical, Recording::Off);
    let rejected = live
        .send(&ScriptedEvent {
            data: scxml_types::DataValue::canonical_json_object(r#"{"m":1}"#).expect("json"),
            ..ScriptedEvent::named("go")
        })
        .expect_err("the root's transition does not accept the data");
    assert!(
        matches!(
            rejected.failure,
            RunFailure::Machine {
                code: "invalid-input",
                ..
            }
        ),
        "{:?}",
        rejected.failure
    );
    assert_eq!(live.status(), LiveStatus::Running);
    assert!(log_labels(&live.take_outputs()).is_empty());
    let step = live
        .send(&ScriptedEvent {
            data: scxml_types::DataValue::canonical_json_object(r#"{"n":1}"#).expect("json"),
            ..ScriptedEvent::named("go")
        })
        .expect("go");
    assert_eq!(step.status, LiveStatus::Finished);
    assert_eq!(log_labels(&live.take_outputs()), ["data"]);
}
