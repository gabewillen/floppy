//! Generated `component-wasm` and runtime `ir-source` proofs.
//!
//! `scripts/verify-wasm-capabilities.sh scxml_machine` builds and
//! virtualizes the generic machine template, then uses the CLI to lower the
//! representative fixture and package it as `component-wasm` (validated,
//! import-allowlisted, and byte-deterministic). These tests execute those
//! exact bytes under Wasmtime:
//!
//! * packaged mode: the composed component, authenticated by the SHA-256 in
//!   the canonical compiler report;
//! * runtime mode: the generic template, with the host serving the exact
//!   lowered IR through `ir-source`;
//!
//! and require both modes to produce identical per-command traces (status,
//! configuration, effect count, ordered `<log>` records) and identical
//! ordered data-model capability calls. Correctness is decided by fixed
//! expected results, never by a second implementation of the SCXML
//! semantics: the engine inside the machine component is the only executor,
//! and every scenario states its expected trace. Hostile IR must fail closed
//! with typed codes, never trap or panic.
//!
//! The representative trace, and the ordered data-model capability calls
//! that produced it, are pinned in the shared fixture
//! `tests/fixtures/representative-machine.trace.json`. The browser host
//! (`hosts/scxml-host-browser`) replays the same events through the same
//! generic machine transpiled by jco and must reproduce that exact fixture,
//! so the two hosts cannot drift. Regenerate it after an intended semantic
//! change with `sh scripts/bless-fixtures.sh --trace-fixture` and review the
//! diff: the fixture is the expected result, not a recording to accept
//! unread.

#![cfg(feature = "wasmtime")]

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, OnceLock};

use scxml_compiler_core::{
    ArtifactKind as PhaseKind, COMPONENT_WASM_FILE_EXTENSION, LOWERED_IR_FILE_EXTENSION,
    LOWERED_IR_MEDIA_TYPE, MachineIr, PackageRecipe, PlannedArtifact,
};
use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_host_wasmtime::{
    ArtifactError, ArtifactKind, AuthenticatedComponent, ComponentSource, ComponentStatus,
    DataModelFailure, HostError, MachineDataModel, MachineIrSource, MachineLogRecord,
    MachineSessionTrace, TelemetryProviders, TrustedComponentDigest, TrustedManifestDigest,
    WasmtimeHostConfig, authenticate_component, authenticate_packaged_machine,
    execute_machine_session,
};
use scxml_reporting::{ArtifactPayload, CompilerStatus, decode_canonical};
use scxml_types::{AssignPlacement, AssignSource, DataValue, Event, SendId};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

/// Representative command sequence: enter the parallel state, advance both
/// regions (the right region's guard reads `In('l1')` before the microstep),
/// settle through an internal raise, record deep history on pause, restore
/// it on resume, then finish.
const EVENTS: [&str; 6] = ["begin", "step", "pause", "resume", "unknown", "finish"];

/// `_sessionid` the ECMAScript data model is initialized with, as recorded
/// in the shared trace fixture.
const SESSION_ID: &str = "machine-session";

/// Shared cross-host trace fixture, relative to the workspace root.
const TRACE_FIXTURE: &str = "tests/fixtures/representative-machine.trace.json";

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Artifacts {
    template: AuthenticatedComponent,
    template_sha256: String,
    template_bytes: Vec<u8>,
    ir: Vec<u8>,
    ir_recipe: PackageRecipe,
    packaged: AuthenticatedComponent,
    packaged_bytes: Vec<u8>,
}

fn artifacts() -> &'static Artifacts {
    static ARTIFACTS: OnceLock<Artifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(build_artifacts)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
}

fn build_artifacts() -> Artifacts {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("host crate has a workspace parent");
    let target = workspace.join("target/component-proof");
    let status = Command::new("sh")
        .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
        .arg("scxml_machine")
        .current_dir(workspace)
        // The Cargo target every host suite builds the release components
        // in, so they compile once; this suite's artifacts stay its own.
        .env(
            "CARGO_TARGET_DIR",
            workspace.join("target/session-host-proof"),
        )
        // This suite's own artifact directory: suites sharing the Cargo
        // target never rewrite a manifest another suite is reading.
        .env(
            "SCXML_WASM_ARTIFACTS_DIR",
            target.join("wasm-artifacts/machine-ir-modes"),
        )
        .status()
        .expect("the capability script runs");
    assert!(
        status.success(),
        "release capability and packaging proof must pass"
    );
    let release = target.join("wasm-artifacts/machine-ir-modes");
    let manifest = read(&release.join("manifest.tsv"));
    let template_bytes = read(&release.join("scxml_machine.virtualized.wasm"));
    let template = authenticate_component(
        &manifest,
        TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
            .expect("manifest digest is canonical"),
        ArtifactKind::Machine,
        "scxml_machine.virtualized.wasm",
        &template_bytes,
    )
    .expect("the generic machine template authenticates");
    let template_sha256 = format!("{:x}", Sha256::digest(&template_bytes));

    let packaged_dir: PathBuf = release.join("packaged");
    let ir = read(&packaged_dir.join("representative-machine.scxml-ir.json"));
    let ir_recipe: PackageRecipe = serde_json::from_slice(&read(
        &packaged_dir.join("representative-machine.scxml-ir.json.recipe.json"),
    ))
    .expect("lowered IR recipe parses");
    let packaged_bytes = read(&packaged_dir.join("representative-machine.component.wasm"));
    let report = read(&packaged_dir.join("representative-machine.component.report.json"));

    // The canonical compiler report is the trust anchor for the packaged
    // bytes: it records the phase, digest, template identity, and IR digest.
    let envelope = decode_canonical(&report).expect("the compiler report is canonical");
    let ArtifactPayload::Compiler { compiler } = envelope.payload else {
        panic!("the packaging report is a compiler report");
    };
    assert_eq!(compiler.status, CompilerStatus::Emitted);
    let output = compiler.output.expect("an emitted report carries output");
    assert_eq!(output.artifact_kind, "component-wasm");
    assert_eq!(output.file_extension, COMPONENT_WASM_FILE_EXTENSION);
    let recipe: PackageRecipe =
        serde_json::from_slice(&output.package_recipe).expect("report recipe parses");
    assert_eq!(
        recipe.template_sha256.as_deref(),
        Some(template_sha256.as_str()),
        "the component pins the exact authenticated template"
    );
    assert_eq!(
        recipe.ir_sha256,
        format!("{:x}", Sha256::digest(&ir)),
        "the component binds the exact lowered IR executed in runtime mode"
    );
    assert_eq!(recipe.ir_sha256, ir_recipe.ir_sha256);
    assert_eq!(recipe.next_phase, None);
    let packaged = authenticate_packaged_machine(
        &packaged_bytes,
        TrustedComponentDigest::from_hex(&output.sha256).expect("report digest is canonical"),
    )
    .expect("the packaged component matches its compiler report");
    Artifacts {
        template,
        template_sha256,
        template_bytes,
        ir,
        ir_recipe,
        packaged,
        packaged_bytes,
    }
}

// ---------------------------------------------------------------------------
// The ECMAScript data model every representative session links, recording
// each capability call the machine makes.

struct InQuery<'a>(&'a [String]);

impl ExecutionHost for InQuery<'_> {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(self.0.iter().any(|state| state == state_id))
    }
}

/// Ordered data-model capability calls of one session, shared with the test.
type CallLog = Rc<RefCell<Vec<serde_json::Value>>>;

struct Ecma {
    model: EcmaScriptDataModel,
    configuration: Vec<String>,
    calls: CallLog,
}

impl Ecma {
    fn new() -> Self {
        Self::recording(CallLog::default())
    }

    fn recording(calls: CallLog) -> Self {
        let mut model = EcmaScriptDataModel::new();
        model
            .initialize(
                "",
                SystemContext {
                    session_id: SESSION_ID.into(),
                    machine_name: Some("representative".into()),
                    io_processors: DataValue::canonical_json_object("{}")
                        .expect("empty I/O processor object is canonical"),
                },
            )
            .expect("the ECMAScript data model initializes");
        Self {
            model,
            configuration: Vec::new(),
            calls,
        }
    }

    /// Records one data-model capability call with the configuration its
    /// `In()` queries observed and the result the machine received.
    fn record<T>(
        &self,
        mut call: serde_json::Value,
        result: &Result<T, DataModelError>,
        ok: impl FnOnce(&T) -> serde_json::Value,
    ) {
        call["configuration"] = serde_json::json!(self.configuration);
        call["result"] = match result {
            Ok(value) => serde_json::json!({ "ok": ok(value) }),
            Err(error) => serde_json::json!({ "err": error_code(*error) }),
        };
        self.calls.borrow_mut().push(call);
    }
}

const fn component_failure(error: DataModelError) -> DataModelFailure {
    match error {
        DataModelError::InvalidInput | DataModelError::UnsupportedValue => {
            DataModelFailure::InvalidInput
        }
        DataModelError::Execution => DataModelFailure::Execution,
        DataModelError::NotInitialized | DataModelError::Host => DataModelFailure::Platform,
    }
}

/// Closed WIT `error-code` a data-model failure crosses the boundary as.
const fn error_code(error: DataModelError) -> &'static str {
    match component_failure(error) {
        DataModelFailure::InvalidInput => "invalid-input",
        DataModelFailure::Execution => "execution",
        DataModelFailure::Unavailable => "unavailable",
        DataModelFailure::Platform => "platform",
    }
}

/// WIT `data-value` in the jco JavaScript shape (`{tag, val}`).
fn wit_json_value(value: &DataValue) -> serde_json::Value {
    let (tag, val) = match value {
        DataValue::Absent => ("absent", None),
        DataValue::Undefined => ("undefined", None),
        DataValue::Null => ("null", None),
        DataValue::Bool(value) => ("boolean", Some(serde_json::json!(value))),
        DataValue::Number(value) => ("number", Some(serde_json::json!(value.get()))),
        DataValue::String(value) => ("text", Some(serde_json::json!(value))),
        DataValue::JsonArray(value) => ("json-array", Some(serde_json::json!(value))),
        DataValue::JsonObject(value) => ("json-object", Some(serde_json::json!(value))),
        DataValue::Xml(value) => ("xml", Some(serde_json::json!(value))),
    };
    val.map_or_else(
        || serde_json::json!({ "tag": tag }),
        |val| serde_json::json!({ "tag": tag, "val": val }),
    )
}

/// The `_event` a data-model call received (`null` for no-event calls).
fn event_json(event: Option<&Event>) -> serde_json::Value {
    event.map_or(serde_json::Value::Null, |event| {
        serde_json::json!({
            "name": event.name.as_str(),
            "event-type": match event.event_type {
                scxml_types::EventType::Internal => "internal",
                scxml_types::EventType::External => "external",
                scxml_types::EventType::Platform => "platform",
            },
            "send-id": event.send_id.as_ref().map(SendId::as_str),
            "origin": event.origin,
            "origin-type": event.origin_type,
            "invoke-id": event.invoke_id.as_ref().map(scxml_types::InvokeId::as_str),
            "data": wit_json_value(&event.data),
        })
    })
}

impl MachineDataModel for Ecma {
    fn update_active_configuration(
        &mut self,
        exited: &[String],
        entered: &[String],
    ) -> Result<(), DataModelFailure> {
        self.configuration.retain(|state| !exited.contains(state));
        self.configuration.extend_from_slice(entered);
        Ok(())
    }

    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        let result = match event {
            Some(event) => self.model.evaluate_condition(expression, event, &mut host),
            None => self
                .model
                .evaluate_condition_without_event(expression, &mut host),
        };
        self.record(
            serde_json::json!({
                "op": "evaluate-condition",
                "expression": expression,
                "event": event_json(event),
            }),
            &result,
            |value| serde_json::json!(value),
        );
        result.map_err(component_failure)
    }

    fn evaluate_value(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<DataValue, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        let result = match event {
            Some(event) => self.model.evaluate_value(expression, event, &mut host),
            None => self
                .model
                .evaluate_value_without_event(expression, &mut host),
        };
        self.record(
            serde_json::json!({
                "op": "evaluate-value",
                "expression": expression,
                "event": event_json(event),
            }),
            &result,
            wit_json_value,
        );
        result.map_err(component_failure)
    }

    fn assign(
        &mut self,
        location: &str,
        source: &AssignSource,
        placement: &AssignPlacement,
        event: Option<&Event>,
    ) -> Result<(), DataModelFailure> {
        // The data model's own assignment semantics decide the result; the
        // call is recorded in the shape the browser trace harness replays.
        let configuration = self.configuration.clone();
        let result = self.model.apply_assign(
            location,
            source,
            placement,
            event,
            &mut InQuery(&configuration),
        );
        let (key, assigned) = match source {
            AssignSource::Expression(expression) => ("expression", serde_json::json!(expression)),
            AssignSource::Value(value) => ("value", wit_json_value(value)),
            AssignSource::ForeachValue(value) => ("foreach-value", wit_json_value(value)),
        };
        let mut call = serde_json::json!({
            "op": "assign",
            "location": location,
            "event": event_json(event),
        });
        call[key] = assigned;
        self.record(call, &result, |()| serde_json::Value::Null);
        result.map_err(component_failure)
    }

    fn execute(&mut self, script: &str, event: Option<&Event>) -> Result<(), DataModelFailure> {
        let configuration = self.configuration.clone();
        let mut host = InQuery(&configuration);
        let result = match event {
            Some(event) => self.model.execute(script, event, &mut host),
            None => self.model.execute_without_event(script, &mut host),
        };
        self.record(
            serde_json::json!({
                "op": "execute",
                "script": script,
                "event": event_json(event),
            }),
            &result,
            |()| serde_json::Value::Null,
        );
        result.map_err(component_failure)
    }

    fn evaluate_log(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<String, DataModelFailure> {
        let mut host = InQuery(&self.configuration);
        let result = match event {
            Some(event) => self.model.evaluate_log(expression, event, &mut host),
            None => self.model.evaluate_log_without_event(expression, &mut host),
        };
        self.record(
            serde_json::json!({
                "op": "evaluate-log",
                "expression": expression,
                "event": event_json(event),
            }),
            &result,
            |text| serde_json::json!(text),
        );
        result.map_err(component_failure)
    }
}

/// Observable projection every scenario states its expected result in.
#[derive(Debug, PartialEq, Eq)]
struct ObservedStep {
    status: ComponentStatus,
    configuration: Vec<String>,
    logs: Vec<String>,
}

fn log_line(record: &MachineLogRecord) -> String {
    record.label.as_ref().map_or_else(
        || record.text.clone(),
        |label| format!("{label}: {}", record.text),
    )
}

fn observed(trace: &MachineSessionTrace) -> Vec<ObservedStep> {
    trace
        .results
        .iter()
        .map(|step| ObservedStep {
            status: step.status,
            configuration: step.active_configuration.clone(),
            logs: step.logs.iter().map(log_line).collect(),
        })
        .collect()
}

const fn status_name(status: ComponentStatus) -> &'static str {
    match status {
        ComponentStatus::Ready => "ready",
        ComponentStatus::Running => "running",
        ComponentStatus::Finished => "finished",
        ComponentStatus::Failed => "failed",
        ComponentStatus::ShutDown => "shut-down",
    }
}

/// The shared cross-host fixture document for one representative session.
fn trace_fixture(
    ir: &[u8],
    steps: &[ObservedStep],
    calls: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::json!({
        "schema": "stateforward.machine-trace/v1",
        "source": "tests/fixtures/representative-machine.scxml",
        "ir-sha256": format!("{:x}", Sha256::digest(ir)),
        "session-id": SESSION_ID,
        "events": EVENTS,
        "results": steps.iter().map(|step| serde_json::json!({
            "status": status_name(step.status),
            "configuration": step.configuration,
            "logs": step.logs,
        })).collect::<Vec<_>>(),
        "data-model-calls": calls,
    })
}

/// Compares one session with the checked-in expected fixture, or rewrites
/// the fixture when `SCXML_UPDATE_TRACE_FIXTURE=1`.
fn assert_shared_trace_fixture(ir: &[u8], steps: &[ObservedStep], calls: &[serde_json::Value]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(TRACE_FIXTURE);
    let actual = trace_fixture(ir, steps, calls);
    let mut rendered = serde_json::to_string_pretty(&actual).expect("fixture encodes");
    rendered.push('\n');
    if std::env::var_os("SCXML_UPDATE_TRACE_FIXTURE").is_some_and(|value| value == "1") {
        std::fs::write(&path, &rendered).expect("the shared trace fixture is writable");
    }
    let expected: serde_json::Value =
        serde_json::from_slice(&read(&path)).expect("the shared trace fixture is JSON");
    assert_eq!(
        actual, expected,
        "the machine component drifted from {TRACE_FIXTURE}; after an intended \
         change run `sh scripts/bless-fixtures.sh --trace-fixture` and review the \
         diff. Current:\n{rendered}"
    );
}

fn run(component: &AuthenticatedComponent, ir: MachineIrSource<'_>) -> MachineSessionTrace {
    run_recorded(component, ir).0
}

/// Runs the representative events and returns the trace plus the ordered
/// data-model calls behind it.
fn run_recorded(
    component: &AuthenticatedComponent,
    ir: MachineIrSource<'_>,
) -> (MachineSessionTrace, Vec<serde_json::Value>) {
    let calls = CallLog::default();
    let trace = execute_machine_session(
        ComponentSource::Authenticated(component),
        ir,
        &EVENTS,
        Box::new(Ecma::recording(Rc::clone(&calls))),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the machine session executes under Wasmtime");
    (trace, calls.take())
}

/// Packages canonical `ir` with the authenticated generic template through
/// the library packager (the path `scxml package` takes), so any fixture
/// also runs as a compile-time `component-wasm`.
fn package_ir(ir: &[u8]) -> AuthenticatedComponent {
    let artifacts = artifacts();
    let mut recipe = artifacts.ir_recipe.clone();
    recipe.ir_sha256 = format!("{:x}", Sha256::digest(ir));
    let lowered = PlannedArtifact {
        kind: PhaseKind::LoweredIr,
        media_type: LOWERED_IR_MEDIA_TYPE.into(),
        file_extension: LOWERED_IR_FILE_EXTENSION.into(),
        bytes: ir.to_vec(),
        recipe,
    };
    let template = scxml_packager::MachineTemplate {
        bytes: &artifacts.template_bytes,
        sha256: &artifacts.template_sha256,
    };
    let packaged =
        scxml_packager::package(&lowered, template, &[]).expect("the fixture IR packages");
    authenticate_packaged_machine(
        &packaged.bytes,
        TrustedComponentDigest::from_hex(&format!("{:x}", Sha256::digest(&packaged.bytes)))
            .expect("digest is canonical"),
    )
    .expect("the packaged fixture authenticates")
}

/// Runs `machine` in both IR supply modes, each with a fresh data model
/// from `data_model`, requires identical outcomes (every step's status,
/// configuration, effect count, and log records, or the same typed
/// failure), and returns the runtime-mode outcome.
fn run_both_modes(
    machine: &MachineIr,
    events: &[&str],
    data_model: impl Fn() -> Box<dyn MachineDataModel>,
) -> Result<MachineSessionTrace, HostError> {
    let artifacts = artifacts();
    let ir = machine
        .to_json()
        .expect("the fixture is valid canonical IR");
    let packaged = package_ir(&ir);
    let session = |component: &AuthenticatedComponent, source| {
        execute_machine_session(
            ComponentSource::Authenticated(component),
            source,
            events,
            data_model(),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
        )
    };
    let runtime = session(&artifacts.template, MachineIrSource::Runtime(&ir));
    let compiled = session(&packaged, MachineIrSource::Packaged);
    assert_eq!(
        format!("{:?}", compiled.as_ref().map(|trace| &trace.results)),
        format!("{:?}", runtime.as_ref().map(|trace| &trace.results)),
        "compile-time and runtime IR modes must behave identically"
    );
    runtime
}

#[test]
fn packaged_and_runtime_ir_modes_reproduce_the_expected_trace() {
    let _serial = serial();
    let artifacts = artifacts();
    let (packaged, packaged_calls) = run_recorded(&artifacts.packaged, MachineIrSource::Packaged);
    let (runtime, runtime_calls) =
        run_recorded(&artifacts.template, MachineIrSource::Runtime(&artifacts.ir));

    assert_eq!(
        packaged.results, runtime.results,
        "packaged component-wasm and runtime ir-source mode commit identical steps"
    );
    assert_eq!(
        packaged_calls, runtime_calls,
        "both modes make identical ordered data-model calls"
    );
    let steps = observed(&runtime);
    assert_shared_trace_fixture(&artifacts.ir, &steps, &runtime_calls);

    // The representative sequence exercises every targeted construct.
    let configurations = steps
        .iter()
        .map(|step| step.configuration.join(","))
        .collect::<Vec<_>>();
    assert_eq!(
        configurations,
        ["idle", "l1,r1", "l2,r2", "paused", "l2,r2", "l2,r2", "done",],
        "compound entry, parallel regions, history restore, and finish"
    );
    assert_eq!(
        steps.last().map(|step| step.status),
        Some(ComponentStatus::Finished)
    );
    let logs = steps
        .iter()
        .flat_map(|step| step.logs.clone())
        .collect::<Vec<_>>();
    assert!(
        logs.contains(&"enter: busy:fresh".to_owned()),
        "In() is evaluated: {logs:?}"
    );
    assert!(
        logs.contains(&"settled: both regions advanced".to_owned()),
        "internal raise: {logs:?}"
    );
    assert!(
        logs.contains(&"done: true".to_owned()),
        "final entry: {logs:?}"
    );
}

/// Two `<onentry>` blocks where the first fails recoverably (SCXML
/// 3.4/5.7/5.10): only that block aborts; the second still logs and raises,
/// so the machine reaches `pass`. Every surface gets this from the engine's
/// `Engine::run`, not from host code.
fn two_block_machine() -> MachineIr {
    use scxml_compiler_core::{
        ActionBlockIr, Binding, ExecutableContentIr, InitialIr, StateIr, StateKind, TransitionIr,
        TransitionType,
    };
    let log = |label: &str, expr: &str| ExecutableContentIr::Log {
        label: Some(label.into()),
        expr: Some(expr.into()),
    };
    let state = |document_order, id: &str, kind, on_entry, transitions| StateIr {
        document_order,
        id: Some(id.into()),
        kind,
        parent: None,
        children: Vec::new(),
        initial: None,
        onentry: on_entry,
        onexit: Vec::new(),
        transitions,
        data: Vec::new(),
        invokes: Vec::new(),
        donedata: None,
    };
    MachineIr {
        version: scxml_compiler_core::MACHINE_IR_VERSION,
        name: Some("two-block".into()),
        data_model: Some("ecmascript".into()),
        binding: Binding::Early,
        initial: InitialIr {
            document_order: 0,
            targets: vec![0],
            actions: Vec::new(),
        },
        states: vec![
            state(
                1,
                "s0",
                StateKind::Atomic,
                vec![
                    ActionBlockIr {
                        document_order: 2,
                        actions: vec![
                            log("first", "undefined.boom"),
                            log("first-unreachable", "'skipped'"),
                        ],
                    },
                    ActionBlockIr {
                        document_order: 5,
                        actions: vec![
                            log("second", "'ran'"),
                            ExecutableContentIr::Raise {
                                event: "go".into(),
                                event_schema: None,
                            },
                        ],
                    },
                ],
                vec![0],
            ),
            state(10, "pass", StateKind::Final, Vec::new(), Vec::new()),
        ],
        transitions: vec![TransitionIr {
            document_order: 9,
            source: 0,
            targets: vec![1],
            events: vec!["go".into()],
            cond: None,
            transition_type: TransitionType::External,
            actions: Vec::new(),
            event_schema: None,
        }],
        data: Vec::new(),
        scripts: Vec::new(),
        extension_namespaces: Vec::new(),
    }
}

#[test]
fn recoverable_block_failure_aborts_only_its_block_in_both_ir_modes() {
    let _serial = serial();
    let trace = run_both_modes(&two_block_machine(), &[], || Box::new(Ecma::new()))
        .expect("the two-block machine runs under Wasmtime");
    assert_eq!(
        observed(&trace),
        vec![ObservedStep {
            status: ComponentStatus::Finished,
            configuration: vec!["pass".to_owned()],
            logs: vec!["second: ran".to_owned()],
        }],
        "the machine component keeps later blocks after a recoverable failure"
    );
}

#[test]
fn library_packaging_reproduces_the_cli_component_bytes() {
    let artifacts = artifacts();
    let lowered = PlannedArtifact {
        kind: PhaseKind::LoweredIr,
        media_type: LOWERED_IR_MEDIA_TYPE.into(),
        file_extension: LOWERED_IR_FILE_EXTENSION.into(),
        bytes: artifacts.ir.clone(),
        recipe: artifacts.ir_recipe.clone(),
    };
    let template = scxml_packager::MachineTemplate {
        bytes: &artifacts.template_bytes,
        sha256: &artifacts.template_sha256,
    };
    let first =
        scxml_packager::package(&lowered, template, &[]).expect("library packaging succeeds");
    let second =
        scxml_packager::package(&lowered, template, &[]).expect("library packaging repeats");
    assert_eq!(first.bytes, second.bytes, "packaging is byte-deterministic");
    assert_eq!(
        first.bytes, artifacts.packaged_bytes,
        "library and CLI packaging emit identical component bytes"
    );
}

fn hostile_code(ir: &[u8]) -> String {
    let artifacts = artifacts();
    match execute_machine_session(
        ComponentSource::Authenticated(&artifacts.template),
        MachineIrSource::Runtime(ir),
        &EVENTS,
        Box::new(Ecma::new()),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    ) {
        Err(HostError::Component(code)) => code,
        other => panic!("hostile IR must yield a typed failed session, got {other:?}"),
    }
}

#[test]
fn hostile_runtime_ir_fails_closed_with_typed_codes() {
    let _serial = serial();
    let artifacts = artifacts();
    let value: serde_json::Value =
        serde_json::from_slice(&artifacts.ir).expect("lowered IR is JSON");

    let mut future = value.clone();
    future["version"] = serde_json::json!(scxml_compiler_core::MACHINE_IR_VERSION + 1);
    assert_eq!(
        hostile_code(&serde_json::to_vec(&future).expect("future IR encodes")),
        "unsupported-version"
    );

    for malformed in [&b""[..], b"\0asm", b"{\"version\": 4", b"[1, 2, 3]"] {
        assert_eq!(hostile_code(malformed), "invalid-input");
    }

    let mut dangling = value.clone();
    dangling["initial"]["targets"] = serde_json::json!([u32::MAX]);
    assert_eq!(
        hostile_code(&serde_json::to_vec(&dangling).expect("dangling IR encodes")),
        "invalid-input",
        "semantically invalid IR is rejected before engine use"
    );

    let mut unknown = value.clone();
    unknown["unexpected"] = serde_json::Value::Bool(true);
    assert_eq!(
        hostile_code(&serde_json::to_vec(&unknown).expect("unknown-field IR encodes")),
        "invalid-input"
    );

    // An id-less history state is valid Machine IR outside the engine
    // slice (lowering always generates an id, SCXML 3.14).
    let mut unsupported = common::example_machine();
    let template = unsupported.states[0].clone();
    unsupported.states[0].kind = scxml_compiler_core::StateKind::Compound;
    unsupported.states[0].children = vec![2, 3];
    unsupported.states[0].initial = Some(scxml_compiler_core::InitialIr {
        document_order: 4,
        targets: vec![3],
        actions: Vec::new(),
    });
    unsupported.states.push(scxml_compiler_core::StateIr {
        document_order: 5,
        id: None,
        kind: scxml_compiler_core::StateKind::ShallowHistory,
        parent: Some(0),
        transitions: Vec::new(),
        ..template.clone()
    });
    unsupported.states.push(scxml_compiler_core::StateIr {
        document_order: 6,
        id: Some("inner".into()),
        parent: Some(0),
        transitions: Vec::new(),
        ..template
    });
    let unsupported = unsupported
        .to_json()
        .expect("outside-slice IR is valid Machine IR");
    assert_eq!(
        hostile_code(&unsupported),
        "unavailable",
        "valid IR outside the engine slice is unavailable, not invalid"
    );

    // The native host serves Machine IR of any size; the wasm machine
    // applies its own 16 MiB bound (resource bounds are wasm-build-only).
    let oversized = vec![b' '; 16 * 1024 * 1024 + 1];
    assert_eq!(hostile_code(&oversized), "payload-too-large");
    let control = run(
        &artifacts.template,
        MachineIrSource::Runtime(&common::example_ir()),
    );
    assert_eq!(
        control.results.last().map(|step| step.status),
        Some(ComponentStatus::Finished),
        "the same template still runs other valid IR after hostile sessions"
    );
}

#[test]
fn ir_supply_mode_must_match_the_component_world() {
    let _serial = serial();
    let artifacts = artifacts();
    let mismatch = |component: &AuthenticatedComponent, ir| {
        execute_machine_session(
            ComponentSource::Authenticated(component),
            ir,
            &[],
            Box::new(Ecma::new()),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
        )
    };
    assert!(
        matches!(
            mismatch(&artifacts.template, MachineIrSource::Packaged),
            Err(HostError::Artifact(ArtifactError::IrSourceModeMismatch))
        ),
        "the generic template is never run as if packaged"
    );
    assert!(
        matches!(
            mismatch(&artifacts.packaged, MachineIrSource::Runtime(&artifacts.ir)),
            Err(HostError::Artifact(ArtifactError::IrSourceModeMismatch))
        ),
        "a packaged machine never silently ignores host-supplied IR"
    );
}

#[test]
fn packaged_authentication_fails_closed_on_tampering() {
    let artifacts = artifacts();
    let digest = TrustedComponentDigest::from_hex(&format!(
        "{:x}",
        Sha256::digest(&artifacts.packaged_bytes)
    ))
    .expect("digest is canonical");
    let mut tampered = artifacts.packaged_bytes.clone();
    let last = tampered.last_mut().expect("component is non-empty");
    *last ^= 1;
    assert!(matches!(
        authenticate_packaged_machine(&tampered, digest),
        Err(HostError::Artifact(ArtifactError::ArtifactDigestMismatch))
    ));
    // The empty slice fails the same size predicate a slice past the
    // 2 GiB − 1 ceiling (GitHub's release-asset limit) does; the crate's unit
    // tests prove that predicate's upper boundary without allocating 2 GiB.
    assert!(matches!(
        authenticate_packaged_machine(&[], digest),
        Err(HostError::Artifact(ArtifactError::ArtifactTooLarge))
    ));
    assert!(TrustedComponentDigest::from_hex("ABC").is_err());
}

// ---------------------------------------------------------------------------
// Guard and stable-configuration contract proofs (W3C Appendix D, SCXML 5.9/5.10, and
// the WIT `evaluate-condition` / `machine-api` contracts).

/// Deterministic scripted data model linked in both IR supply modes:
/// `flag` reads a boolean that `<assign location="flag">` sets
/// with the default placement, or `<assign location="flag:<placement>">`
/// with exactly that `XPath` placement (CR 2014-03-13 §C.3.6) (for example `flag:FirstChild`),
/// `fail:<code>` fails with that closed WIT code, `_event:<name>` holds when
/// `_event` is bound to `<name>`, and every value expression evaluates to its
/// own text.
#[derive(Default)]
struct ScriptedGuards {
    flag: bool,
}

impl ScriptedGuards {
    fn condition(&self, expression: &str, event: Option<&Event>) -> Result<bool, DataModelFailure> {
        match expression {
            "flag" => Ok(self.flag),
            "fail:invalid-input" => Err(DataModelFailure::InvalidInput),
            "fail:execution" => Err(DataModelFailure::Execution),
            "fail:unavailable" => Err(DataModelFailure::Unavailable),
            "fail:platform" => Err(DataModelFailure::Platform),
            other => Ok(other
                .strip_prefix("_event:")
                .is_some_and(|name| event.is_some_and(|event| event.name.as_str() == name))),
        }
    }

    fn assign(&mut self, location: &str, placement: &AssignPlacement) {
        let expected = match location.strip_prefix("flag:") {
            Some(placement) => placement,
            None if location == "flag" => "ReplaceChildren",
            None => return,
        };
        if format!("{placement:?}") == expected {
            self.flag = true;
        }
    }
}

impl MachineDataModel for ScriptedGuards {
    fn update_active_configuration(
        &mut self,
        _exited: &[String],
        _entered: &[String],
    ) -> Result<(), DataModelFailure> {
        Ok(())
    }

    fn evaluate_condition(
        &mut self,
        expression: &str,
        event: Option<&Event>,
    ) -> Result<bool, DataModelFailure> {
        self.condition(expression, event)
    }

    fn evaluate_value(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<DataValue, DataModelFailure> {
        Ok(DataValue::String(expression.to_owned()))
    }

    fn assign(
        &mut self,
        location: &str,
        _source: &AssignSource,
        placement: &AssignPlacement,
        _event: Option<&Event>,
    ) -> Result<(), DataModelFailure> {
        ScriptedGuards::assign(self, location, placement);
        Ok(())
    }

    fn execute(&mut self, _script: &str, _event: Option<&Event>) -> Result<(), DataModelFailure> {
        Ok(())
    }

    fn evaluate_log(
        &mut self,
        expression: &str,
        _event: Option<&Event>,
    ) -> Result<String, DataModelFailure> {
        Ok(expression.to_owned())
    }
}

mod guard_ir {
    use scxml_compiler_core::{
        ActionBlockIr, Binding, ExecutableContentIr, InitialIr, MachineIr, StateIr, StateKind,
        TransitionIr, TransitionType,
    };

    pub fn log(label: &str, text: &str) -> ExecutableContentIr {
        ExecutableContentIr::Log {
            label: Some(label.into()),
            expr: Some(text.into()),
        }
    }

    pub fn raise(event: &str) -> ExecutableContentIr {
        ExecutableContentIr::Raise {
            event: event.into(),
            event_schema: None,
        }
    }

    pub fn block(document_order: u32, actions: Vec<ExecutableContentIr>) -> Vec<ActionBlockIr> {
        vec![ActionBlockIr {
            document_order,
            actions,
        }]
    }

    pub fn state(
        document_order: u32,
        id: &str,
        kind: StateKind,
        on_entry: Vec<ActionBlockIr>,
        transitions: Vec<u32>,
    ) -> StateIr {
        StateIr {
            document_order,
            id: Some(id.into()),
            kind,
            parent: None,
            children: Vec::new(),
            initial: None,
            onentry: on_entry,
            onexit: Vec::new(),
            transitions,
            data: Vec::new(),
            invokes: Vec::new(),
            donedata: None,
        }
    }

    pub fn transition(
        document_order: u32,
        source: u32,
        targets: Vec<u32>,
        events: &[&str],
        condition: Option<&str>,
        actions: Vec<ExecutableContentIr>,
    ) -> TransitionIr {
        TransitionIr {
            document_order,
            source,
            targets,
            events: events.iter().map(|event| (*event).to_owned()).collect(),
            cond: condition.map(str::to_owned),
            transition_type: TransitionType::External,
            actions,
            event_schema: None,
        }
    }

    pub fn machine(states: Vec<StateIr>, transitions: Vec<TransitionIr>) -> MachineIr {
        MachineIr {
            version: scxml_compiler_core::MACHINE_IR_VERSION,
            name: Some("guard-contract".into()),
            data_model: Some("ecmascript".into()),
            binding: Binding::Early,
            initial: InitialIr {
                document_order: 0,
                targets: vec![0],
                actions: Vec::new(),
            },
            states,
            transitions,
            data: Vec::new(),
            scripts: Vec::new(),
            extension_namespaces: Vec::new(),
        }
    }
}

/// Runs `machine` with the scripted data model in both IR supply modes
/// (see [`run_both_modes`]) and projects the agreed trace.
fn run_scripted(machine: &MachineIr, events: &[&str]) -> Result<Vec<ObservedStep>, HostError> {
    run_both_modes(machine, events, || Box::new(ScriptedGuards::default()))
        .map(|trace| observed(&trace))
}

fn step(status: ComponentStatus, configuration: &[&str], logs: &[&str]) -> ObservedStep {
    ObservedStep {
        status,
        configuration: configuration
            .iter()
            .map(|state| (*state).to_owned())
            .collect(),
        logs: logs.iter().map(|log| (*log).to_owned()).collect(),
    }
}

#[test]
fn false_guarded_eventless_transition_waits_stable_under_wasmtime() {
    use guard_ir::{block, log, machine, state, transition};
    use scxml_compiler_core::{ExecutableContentIr, StateKind};
    let _serial = serial();
    // `wait` holds a false-guarded eventless transition; the machine must be
    // stable after start and after `noop`, then `go` sets the guard and
    // the next macrostep takes the eventless transition (W3C Appendix D).
    let fixture = machine(
        vec![
            state(1, "wait", StateKind::Atomic, Vec::new(), vec![0, 1, 2]),
            state(
                5,
                "done",
                StateKind::Final,
                block(6, vec![log("Outcome", "pass")]),
                Vec::new(),
            ),
        ],
        vec![
            transition(2, 0, vec![1], &[], Some("flag"), Vec::new()),
            transition(3, 0, Vec::new(), &["noop"], None, Vec::new()),
            transition(
                4,
                0,
                Vec::new(),
                &["go"],
                None,
                vec![ExecutableContentIr::Assign {
                    location: "flag".into(),
                    expr: Some("true".into()),
                    body: Vec::new(),
                    assign_type: None,
                    attr: None,
                }],
            ),
        ],
    );
    let expected = vec![
        step(ComponentStatus::Running, &["wait"], &[]),
        step(ComponentStatus::Running, &["wait"], &[]),
        step(ComponentStatus::Finished, &["done"], &["Outcome: pass"]),
    ];
    let trace = run_scripted(&fixture, &["noop", "go"])
        .expect("a waiting machine must stay stable and accept events under Wasmtime");
    assert_eq!(trace, expected);
}

#[test]
fn typed_assign_crosses_the_machine_capability_with_its_placement_under_wasmtime() {
    // SCXML 1.0 CR 2014-03-13 §C.3.6: the machine hands `<assign type attr>` to the data model
    // through `machine-capabilities.assign` instead of failing the session
    // closed; the scripted data model sets `flag` only when it receives
    // exactly the declared placement, in both IR supply modes.
    use guard_ir::{block, log, machine, state, transition};
    use scxml_compiler_core::{AssignTypeIr, ExecutableContentIr, StateKind, XmlNodeIr};
    let typed = |location: &str, assign_type, attr: Option<&str>| ExecutableContentIr::Assign {
        location: location.into(),
        expr: None,
        body: vec![XmlNodeIr::Text { value: "v".into() }],
        assign_type: Some(assign_type),
        attr: attr.map(Into::into),
    };
    for (location, assign_type, attr) in [
        ("flag:FirstChild", AssignTypeIr::Firstchild, None),
        (
            "flag:AddAttribute(\"n\")",
            AssignTypeIr::Addattribute,
            Some("n"),
        ),
    ] {
        let fixture = machine(
            vec![
                state(1, "wait", StateKind::Atomic, Vec::new(), vec![0, 1]),
                state(
                    5,
                    "done",
                    StateKind::Final,
                    block(6, vec![log("Outcome", "pass")]),
                    Vec::new(),
                ),
            ],
            vec![
                transition(2, 0, vec![1], &[], Some("flag"), Vec::new()),
                transition(
                    3,
                    0,
                    Vec::new(),
                    &["go"],
                    None,
                    vec![typed(location, assign_type, attr)],
                ),
            ],
        );
        let expected = vec![
            step(ComponentStatus::Running, &["wait"], &[]),
            step(ComponentStatus::Finished, &["done"], &["Outcome: pass"]),
        ];
        let trace = run_scripted(&fixture, &["go"])
            .expect("a typed assignment reaches the data model under Wasmtime");
        assert_eq!(trace, expected, "{location}");
    }
}

#[test]
fn eventless_guards_observe_each_ignored_internal_event_under_wasmtime() {
    use guard_ir::{block, log, machine, raise, state, transition};
    use scxml_compiler_core::StateKind;
    let _serial = serial();
    let fixture = machine(
        vec![
            state(
                1,
                "wait",
                StateKind::Atomic,
                block(2, vec![raise("noise"), raise("other")]),
                vec![0, 1],
            ),
            state(
                5,
                "done",
                StateKind::Final,
                block(6, vec![log("Outcome", "pass")]),
                Vec::new(),
            ),
            state(
                7,
                "fail",
                StateKind::Final,
                block(8, vec![log("Outcome", "fail")]),
                Vec::new(),
            ),
        ],
        vec![
            transition(3, 0, vec![1], &[], Some("_event:noise"), Vec::new()),
            transition(4, 0, vec![2], &["other"], None, Vec::new()),
        ],
    );
    let expected = vec![step(
        ComponentStatus::Finished,
        &["done"],
        &["Outcome: pass"],
    )];
    let trace = run_scripted(&fixture, &[]).expect("the machine settles under Wasmtime");
    assert_eq!(trace, expected);
}

/// `s0` entry runs `<if cond="fail:<code>">` then raises `go`; `go` is first
/// guarded by the same failing cond, then moves to `s1`, whose
/// `error.execution` transition reaches `pass`.
fn failing_cond_machine(code: &str) -> MachineIr {
    use guard_ir::{block, log, machine, raise, state, transition};
    use scxml_compiler_core::{ExecutableContentIr, StateKind};
    let condition = format!("fail:{code}");
    machine(
        vec![
            state(
                1,
                "s0",
                StateKind::Atomic,
                block(
                    2,
                    vec![
                        ExecutableContentIr::If {
                            cond: condition.clone(),
                            actions: vec![log("branch", "if")],
                            elseif: Vec::new(),
                            else_actions: Some(vec![log("branch", "else")]),
                        },
                        raise("go"),
                    ],
                ),
                vec![0, 1, 2],
            ),
            state(6, "s1", StateKind::Atomic, Vec::new(), vec![3]),
            state(
                8,
                "pass",
                StateKind::Final,
                block(9, vec![log("Outcome", "pass")]),
                Vec::new(),
            ),
            state(
                10,
                "fail",
                StateKind::Final,
                block(11, vec![log("Outcome", "fail")]),
                Vec::new(),
            ),
        ],
        vec![
            transition(3, 0, vec![3], &["go"], Some(&condition), Vec::new()),
            transition(4, 0, vec![1], &["go"], None, Vec::new()),
            transition(
                5,
                0,
                Vec::new(),
                &["error.execution"],
                None,
                vec![log("error", "s0")],
            ),
            transition(7, 1, vec![2], &["error.execution"], None, Vec::new()),
        ],
    )
}

#[test]
fn recoverable_cond_failures_raise_error_execution_under_wasmtime() {
    let _serial = serial();
    // WIT `evaluate-condition`: `invalid-input` and `execution` become SCXML
    // error.execution and the condition is false (SCXML 5.9/5.10), for both
    // `<if>` and transition conds; the session keeps running.
    for code in ["invalid-input", "execution"] {
        let fixture = failing_cond_machine(code);
        let expected = vec![step(
            ComponentStatus::Finished,
            &["pass"],
            &["branch: else", "error: s0", "Outcome: pass"],
        )];
        let trace = run_scripted(&fixture, &[])
            .unwrap_or_else(|error| panic!("a `{code}` cond must not fail the session: {error:?}"));
        assert_eq!(trace, expected, "{code}");
    }
}

#[test]
fn fatal_cond_failures_fail_the_session_with_their_own_code_under_wasmtime() {
    let _serial = serial();
    // `unavailable` and `platform` are not SCXML execution errors: the
    // session fails and reports the provider's closed code, not `execution`.
    for code in ["unavailable", "platform"] {
        match run_scripted(&failing_cond_machine(code), &[]) {
            Err(HostError::Component(reported)) => assert_eq!(reported, code),
            other => panic!("a `{code}` cond must fail the session with `{code}`, got {other:?}"),
        }
    }
}

/// Canonical IR of a parallel state with `regions` two-state regions that
/// all toggle on every `t` event.
fn wide_parallel_ir(regions: usize) -> Vec<u8> {
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="wide" datamodel="null"><parallel id="wide">"#,
    );
    for region in 0..regions {
        std::fmt::Write::write_fmt(
            &mut source,
            format_args!(
                r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><transition event="t" target="r{region}b"/></state><state id="r{region}b"><transition event="t" target="r{region}a"/></state></state>"#
            ),
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str("</parallel></scxml>");
    canonical_ir(source)
}

/// Canonical IR of one SCXML document.
fn canonical_ir(source: String) -> Vec<u8> {
    let document =
        scxml_syntax::parse_xml(scxml_syntax::SourceText::new(SOURCE, "wide.scxml", source))
            .expect("the wide parallel parses");
    scxml_compiler_core::lower(&document)
        .expect("the wide parallel lowers")
        .to_json()
        .expect("the wide parallel IR encodes")
}

/// Resource limits apply to the Wasm components only: the native host
/// imposes no bound of its own on one machine session. More than 1,024
/// events run, one step keeps more than 4,096 `<log>` records, and a log
/// text over 64 KiB is recorded whole.
#[test]
fn a_machine_session_has_no_host_imposed_event_or_log_bound() {
    let logs = r#"<log label="burst"/>"#.repeat(4_097);
    let long = "x".repeat(70_000);
    let ir = canonical_ir(format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="s" datamodel="null"><state id="s"><transition event="burst">{logs}</transition><transition event="long"><log label="long" expr="{long}"/></transition><transition event="t"/></state></scxml>"#
    ));
    let mut events = vec!["t"; 1_024];
    events.push("long");
    events.push("burst");
    let mut config = WasmtimeHostConfig::default();
    config.store.fuel = METERED_FUEL;
    let trace = execute_machine_session(
        ComponentSource::Authenticated(&artifacts().template),
        MachineIrSource::Runtime(&ir),
        &events,
        Box::new(ScriptedGuards::default()),
        &config,
        &TelemetryProviders::new(),
    )
    .expect("a session with 1,026 events and 4,097 logs in one step runs");
    assert_eq!(trace.results.len(), 1 + 1_026, "start, then every event");
    let long_step = &trace.results[trace.results.len() - 2];
    assert_eq!(long_step.logs.len(), 1);
    assert_eq!(long_step.logs[0].text.len(), 70_000);
    let last = trace.results.last().expect("the burst step");
    assert_eq!(last.logs.len(), 4_097);
    assert_eq!(last.active_configuration, ["s"]);
}

/// Fuel budget of one metered session, sized for measurement only.
const METERED_FUEL: u64 = 100_000_000_000;

/// Runs `events` on the generic machine with runtime `ir` under
/// [`METERED_FUEL`] and returns the last step's configuration and the
/// remaining fuel. Deterministic.
fn metered_session(ir: &[u8], events: &[&str]) -> (Vec<String>, u64) {
    let mut config = WasmtimeHostConfig::default();
    config.store.fuel = METERED_FUEL;
    let trace = execute_machine_session(
        ComponentSource::Authenticated(&artifacts().template),
        MachineIrSource::Runtime(ir),
        events,
        Box::new(ScriptedGuards::default()),
        &config,
        &TelemetryProviders::new(),
    )
    .expect("the wide parallel session runs");
    let configuration = trace
        .results
        .last()
        .map(|step| step.active_configuration.clone())
        .unwrap_or_default();
    (configuration, trace.remaining_fuel)
}

/// Asserts that doubling the region count at most ~doubles the fuel: the
/// integer form of `fuel(2N) / fuel(N) <= 2.5` for each adjacent pair.
fn assert_fuel_linear_in_regions(label: &str, fuel: &[(usize, u64)]) {
    eprintln!("machine component fuel, {label}: {fuel:?}");
    for pair in fuel.windows(2) {
        let ((narrow, narrow_fuel), (wide, wide_fuel)) = (pair[0], pair[1]);
        assert!(
            wide_fuel * 2 <= narrow_fuel * 5,
            "{label}: {wide} regions cost {wide_fuel} fuel vs {narrow_fuel} for {narrow}: {fuel:?}"
        );
    }
}

/// Fuel the shipped machine component consumes for one `t` event on a
/// `regions`-wide all-toggling parallel: the fuel of a session sending three
/// events minus that of one sending one, halved. Deterministic.
fn wide_parallel_event_fuel(regions: usize) -> u64 {
    let ir = wide_parallel_ir(regions);
    let toggled: Vec<String> = (0..regions).map(|region| format!("r{region}b")).collect();
    let (one, one_remaining) = metered_session(&ir, &["t"]);
    assert_eq!(one, toggled);
    let (three, three_remaining) = metered_session(&ir, &["t", "t", "t"]);
    assert_eq!(three, toggled);
    (one_remaining - three_remaining) / 2
}

/// One event on a parallel state costs the shipped machine fuel linear in its
/// region count: doubling the regions of an all-toggling parallel at most
/// ~doubles the fuel per event.
#[test]
fn wide_parallel_event_fuel_scales_linearly_with_regions() {
    let _serial = serial();
    let fuel: Vec<(usize, u64)> = [8, 16, 32, 64]
        .into_iter()
        .map(|regions| (regions, wide_parallel_event_fuel(regions)))
        .collect();
    assert_fuel_linear_in_regions("per toggle event", &fuel);
}

/// Canonical IR of `p` holding a deep history `h` (default `wide`) and a
/// `regions`-wide parallel `wide`; `out` leaves `p` for `idle`, whose `go`
/// re-enters `p` by default and `restore` through `h`.
fn wide_parallel_lifecycle_ir(regions: usize) -> Vec<u8> {
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p" datamodel="null"><state id="p" initial="wide"><history id="h" type="deep"><transition target="wide"/></history><parallel id="wide">"#,
    );
    for region in 0..regions {
        std::fmt::Write::write_fmt(
            &mut source,
            format_args!(
                r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"/><state id="r{region}b"/></state>"#
            ),
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str(
        r#"</parallel><transition event="out" target="idle"/></state><state id="idle"><transition event="go" target="p"/><transition event="restore" target="h"/></state></scxml>"#,
    );
    canonical_ir(source)
}

/// Entering a parallel state as a whole costs the shipped machine fuel linear
/// in its region count: a whole session started into it, and one default or
/// deep-history re-entry (the fuel of `out` then the entry event, minus that
/// of `out` alone).
#[test]
fn wide_parallel_entry_fuel_scales_linearly_with_regions() {
    let _serial = serial();
    let mut started = Vec::new();
    let mut default_entry = Vec::new();
    let mut restored = Vec::new();
    for regions in [8, 16, 32, 64] {
        let ir = wide_parallel_lifecycle_ir(regions);
        let leaves: Vec<String> = (0..regions).map(|region| format!("r{region}a")).collect();
        let (configuration, start_remaining) = metered_session(&ir, &[]);
        assert_eq!(configuration, leaves);
        let (configuration, out_remaining) = metered_session(&ir, &["out"]);
        assert_eq!(configuration, ["idle"]);
        let (configuration, go_remaining) = metered_session(&ir, &["out", "go"]);
        assert_eq!(configuration, leaves);
        let (configuration, restore_remaining) = metered_session(&ir, &["out", "restore"]);
        assert_eq!(configuration, leaves);
        started.push((regions, METERED_FUEL - start_remaining));
        default_entry.push((regions, out_remaining - go_remaining));
        restored.push((regions, out_remaining - restore_remaining));
    }
    assert_fuel_linear_in_regions("whole session started into the parallel", &started);
    assert_fuel_linear_in_regions("default entry", &default_entry);
    assert_fuel_linear_in_regions("deep-history restore", &restored);
}

/// Canonical IR of a `regions`-wide parallel whose regions carry
/// `<onentry>` log and `<onexit>` assign content: `t` toggles every region,
/// `finish` moves every region to its final state in one microstep.
fn wide_parallel_content_ir(regions: usize) -> Vec<u8> {
    let mut source = String::from(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="wide" datamodel="ecmascript"><parallel id="wide">"#,
    );
    for region in 0..regions {
        std::fmt::Write::write_fmt(
            &mut source,
            format_args!(
                r#"<state id="r{region}" initial="r{region}a"><state id="r{region}a"><onentry><log label="a" expr="x"/></onentry><onexit><assign location="x" expr="x + 1"/></onexit><transition event="t" target="r{region}b"/><transition event="finish" target="r{region}f"/></state><state id="r{region}b"><onentry><log label="b" expr="x"/></onentry><onexit><assign location="x" expr="x + 1"/></onexit><transition event="t" target="r{region}a"/></state><final id="r{region}f"><onentry><log label="f" expr="x"/></onentry></final></state>"#
            ),
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str("</parallel></scxml>");
    canonical_ir(source)
}

/// With `<onentry>`/`<onexit>` content in every region, one toggle event and
/// the event that finishes every region in one microstep (with the
/// `done.state` events it raises) cost the shipped machine fuel linear in the
/// region count, including the configuration each effect observes.
#[test]
fn wide_parallel_content_fuel_scales_linearly_with_regions() {
    let _serial = serial();
    let mut toggle = Vec::new();
    let mut finish = Vec::new();
    for regions in [8, 16, 32, 64] {
        let ir = wide_parallel_content_ir(regions);
        let leaves = |leaf: char| -> Vec<String> {
            (0..regions)
                .map(|region| format!("r{region}{leaf}"))
                .collect()
        };
        let (configuration, started) = metered_session(&ir, &[]);
        assert_eq!(configuration, leaves('a'));
        let (configuration, one) = metered_session(&ir, &["t"]);
        assert_eq!(configuration, leaves('b'));
        let (configuration, three) = metered_session(&ir, &["t", "t", "t"]);
        assert_eq!(configuration, leaves('b'));
        let (configuration, finished) = metered_session(&ir, &["finish"]);
        assert_eq!(configuration, leaves('f'));
        toggle.push((regions, (one - three) / 2));
        finish.push((regions, started - finished));
    }
    assert_fuel_linear_in_regions("toggle with entry/exit content", &toggle);
    assert_fuel_linear_in_regions("all regions final in one microstep", &finish);
}
