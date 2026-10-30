//! Virtualized data-model components linked as a machine session's data
//! model under Wasmtime.
//!
//! `scripts/verify-wasm-capabilities.sh` builds the release data-model and
//! machine guests and virtualizes them with wasi-virt, which denies every
//! ambient WASI capability (clocks included). These tests instantiate those
//! exact bytes through [`ComponentDataModel`] and prove that:
//!
//! * the ECMAScript component initializes and evaluates without any ambient
//!   clock (boa receives an injected deterministic clock);
//! * expressions evaluated with no current event (for example `<onentry>`
//!   run by `start`) see `_event` unbound, as SCXML 1.0 section 5.10
//!   requires, instead of failing or observing a fabricated event;
//! * a hostile nested-loop expression (a loop calling a looping function,
//!   each loop under the provider's per-loop bound) is stopped by the
//!   store's per-call fuel with the typed `Platform` failure, never a hang;
//! * hostile payloads stay contained typed failures that leave the
//!   instance usable, never a guest trap (`Platform`): runtime source past
//!   the parse bounds is refused (`Execution`) while bounded runtime source
//!   compiles, an optional-chain sequence is declined by the nesting scan
//!   (`InvalidInput`), and an `XPath` string amplification exhausts the
//!   evaluator's own fuel (`Execution`);
//! * the `XPath` component evaluates real `XPath` 1.0 over its declared tree
//!   (node-set functions, predicates, string-values), assigns node-set and
//!   scalar values, keeps system variables read-only, answers `In()` through
//!   the execution-host import, and binds `_event` only while one is current;
//! * steady-state `XPath` conditions, values, `In()`, every `<assign>`
//!   placement, and `<foreach>` item binding on a robot-style document each
//!   stay within a small per-call instruction fuel bound;
//! * the representative machine, run end to end on the component data model,
//!   reproduces the shared Wasmtime/browser trace fixture.

#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use scxml_host_wasmtime::DEFAULT_DATA_MODEL_CALL_FUEL;
use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, ComponentDataModel, ComponentSource, ComponentStatus,
    DataModelFailure, DataModelSystemValues, MachineDataModel, MachineIrSource, TelemetryProviders,
    TrustedManifestDigest, WasmtimeHostConfig, authenticate_component, execute_machine_session,
};
use scxml_types::{
    AssignPlacement, AssignSource, DataInitializer, DataValue, Event, EventName, FiniteNumber,
};
use sha2::{Digest as _, Sha256};

/// Shared cross-host trace fixture, relative to the workspace root.
const TRACE_FIXTURE: &str = "tests/fixtures/representative-machine.trace.json";

struct Artifacts {
    workspace: PathBuf,
    ecmascript: AuthenticatedComponent,
    null: AuthenticatedComponent,
    xpath: AuthenticatedComponent,
    machine: AuthenticatedComponent,
    representative_ir: Vec<u8>,
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
        .expect("host crate has a workspace parent")
        .to_path_buf();
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace.join("target/data-model-proof");
        let status = Command::new("sh")
            .arg(workspace.join("scripts/verify-wasm-capabilities.sh"))
            .args([
                "scxml_data_model_ecmascript",
                "scxml_data_model_null",
                "scxml_data_model_xpath",
                "scxml_machine",
            ])
            .current_dir(&workspace)
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
                target.join("wasm-artifacts/data-model-component"),
            )
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "data-model components must verify");
        target.join("wasm-artifacts/data-model-component")
    });
    let manifest = read(&release.join("manifest.tsv"));
    let trusted = TrustedManifestDigest::from_hex(&format!("{:x}", Sha256::digest(&manifest)))
        .expect("manifest digest is canonical");
    let authenticate = |kind, name: &str| {
        let path = format!("{name}.virtualized.wasm");
        authenticate_component(&manifest, trusted, kind, &path, &read(&release.join(&path)))
            .expect("the virtualized component authenticates")
    };
    Artifacts {
        ecmascript: authenticate(
            ArtifactKind::DataModelEcmascript,
            "scxml_data_model_ecmascript",
        ),
        null: authenticate(ArtifactKind::DataModelNull, "scxml_data_model_null"),
        xpath: authenticate(ArtifactKind::DataModelXpath, "scxml_data_model_xpath"),
        machine: authenticate(ArtifactKind::Machine, "scxml_machine"),
        representative_ir: read(&release.join("packaged/representative-machine.scxml-ir.json")),
        workspace,
    }
}

fn system(machine_name: &str) -> DataModelSystemValues {
    DataModelSystemValues {
        session_id: "machine-session".into(),
        machine_name: Some(machine_name.into()),
        io_processors: DataValue::canonical_json_object("{}")
            .expect("empty I/O processor object is canonical"),
    }
}

fn instantiate(component: &AuthenticatedComponent) -> ComponentDataModel {
    ComponentDataModel::instantiate(
        component,
        &[],
        &system("representative"),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the virtualized data-model component instantiates and initializes")
}

/// An `XPath` component that declares each id (`initialize`) and then values
/// it (`initialize-data`), exactly as a machine binds early data.
fn xpath_with_data(data: &[(&str, DataValue)]) -> ComponentDataModel {
    let declarations = data
        .iter()
        .map(|(id, _)| (*id).to_owned())
        .collect::<Vec<_>>();
    let mut xpath = ComponentDataModel::instantiate(
        &artifacts().xpath,
        &declarations,
        &system("representative"),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the XPath component initializes with declared data");
    for (id, value) in data {
        xpath
            .initialize_data(id, &DataInitializer::Value(value.clone()))
            .expect("the XPath component values declared data");
    }
    xpath
}

fn external(name: &str) -> Event {
    Event::external(EventName::new(name.to_owned()).expect("event name is valid"))
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("finite"))
}

#[test]
fn virtualized_ecmascript_data_model_initializes_and_evaluates_without_ambient_clocks() {
    let mut model = instantiate(&artifacts().ecmascript);
    assert_eq!(model.name(), Some("ecmascript"));
    let event = external("go");
    assert_eq!(model.evaluate_value("1 + 2", Some(&event)), Ok(number(3.0)));
    assert_eq!(
        model.evaluate_condition(
            "_event.name === 'go' && _sessionid === 'machine-session'",
            Some(&event)
        ),
        Ok(true)
    );
    model
        .update_active_configuration(&[], &["ready".to_owned()])
        .expect("configuration installs");
    assert_eq!(
        model.evaluate_condition("In('ready') && !In('done')", Some(&event)),
        Ok(true)
    );
    // Wall-clock time is not a data-model capability: `Date` reads the
    // session's clock through `execution-host.now-ms`, never an ambient
    // clock; a data model linked to no session reads time 0.
    assert_eq!(
        model.evaluate_value("typeof Date", Some(&event)),
        Ok(DataValue::String("function".into()))
    );
    assert_eq!(
        model.evaluate_value("Date.now()", Some(&event)),
        Ok(number(0.0))
    );
    assert_eq!(
        model.evaluate_value("new Date().getTime()", Some(&event)),
        Ok(number(0.0))
    );
    model
        .assign(
            "counter",
            &AssignSource::Value(number(4.0)),
            &AssignPlacement::ReplaceChildren,
            None,
        )
        .expect_err("undeclared locations are rejected");
    model
        .execute("var counter = 1; counter = counter + 1;", Some(&event))
        .expect("scripts execute");
    assert_eq!(
        model.evaluate_value("counter", Some(&event)),
        Ok(number(2.0))
    );
    // Evaluation is bounded: an unbounded loop is a typed failure (the
    // data model's loop bound or the per-call fuel bound), never a hang.
    assert!(model.execute("while (true) {}", Some(&event)).is_err());
}

/// Per-operation instruction fuel the ECMAScript component must stay under
/// once its realm is built: building a guarded realm costs about 82
/// million fuel, while the kept realm serves one operation in well under 2
/// million (a member write that re-registers an object is the costliest
/// case below).
const REUSED_REALM_CALL_FUEL: u64 = 5_000_000;

/// The component keeps one realm per instance across calls: conditions,
/// values, logs, assignments (bare roots, member writes, `<foreach>`
/// bindings), and simple scripts each run within a small fixed fuel bound,
/// independent of the fuel a realm build costs, and keep answering
/// correctly.
#[test]
fn ecmascript_operations_run_in_the_kept_realm_within_a_small_fuel_bound() {
    let declarations = ["battery", "items", "pose", "sum", "ticks"].map(str::to_owned);
    let mut model = ComponentDataModel::instantiate(
        &artifacts().ecmascript,
        &declarations,
        &system("robot"),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the ECMAScript component instantiates");
    for (id, expression) in [
        ("battery", "100"),
        ("items", "[1, 2, 3]"),
        ("pose", "({x: 0, y: 0})"),
        ("sum", "0"),
        ("ticks", "0"),
    ] {
        model
            .initialize_data(id, &DataInitializer::Expression(expression.into()))
            .expect("data initializes");
    }
    let event = external("tick");
    let expression = |text: &str| AssignSource::Expression(text.into());
    let replace = AssignPlacement::ReplaceChildren;
    // One unbounded call settles the realm after initialization.
    model
        .assign("ticks", &expression("ticks + 1"), &replace, Some(&event))
        .expect("the first assignment commits");
    let mut model = model
        .with_call_fuel(REUSED_REALM_CALL_FUEL)
        .expect("non-zero fuel is accepted");
    for round in 1..=20 {
        let ticks = f64::from(round) + 1.0;
        assert_eq!(
            model.evaluate_condition("battery > 20 && ticks >= 0", Some(&event)),
            Ok(true)
        );
        model
            .assign("ticks", &expression("ticks + 1"), &replace, Some(&event))
            .expect("a bare assignment stays within the bound");
        model
            .assign("pose.x", &expression("ticks * 2"), &replace, Some(&event))
            .expect("a member write stays within the bound");
        model
            .assign(
                "sum",
                &AssignSource::ForeachValue(number(ticks)),
                &replace,
                None,
            )
            .expect("a foreach binding stays within the bound");
        model
            .execute("battery = battery - 1; sum++", Some(&event))
            .expect("a simple script stays within the bound");
        assert_eq!(
            model.evaluate_value("[ticks, pose.x, sum, battery]", Some(&event)),
            Ok(DataValue::canonical_json_array(&format!(
                "[{ticks},{},{},{}]",
                ticks * 2.0,
                ticks + 1.0,
                100 - round
            ))
            .expect("canonical array"))
        );
        assert_eq!(
            model.evaluate_log("'ticks=' + ticks", Some(&event)),
            Ok(format!("ticks={ticks}"))
        );
    }
}

/// A loop that calls a looping function: the ECMAScript provider bounds no
/// loop's iterations, so only the store's per-call instruction fuel can
/// stop the multiplied total work.
fn nested_loops(outer: u32, inner: u32) -> String {
    format!(
        "(() => {{ function spin() {{ let n = 0; for (let j = 0; j < {inner}; j++) {{ n = n + 1; }} return n; }} \
         let total = 0; for (let i = 0; i < {outer}; i++) {{ total = total + spin(); }} return total; }})()"
    )
}

#[test]
fn hostile_nested_loop_expression_is_bounded_by_store_fuel_with_a_typed_failure() {
    let per_loop = 99_999;
    let event = external("go");
    let with_fuel = |fuel: u64| {
        ComponentDataModel::instantiate(
            &artifacts().ecmascript,
            &[],
            &system("representative"),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
        )
        .expect("the data model instantiates")
        .with_call_fuel(fuel)
        .expect("non-zero fuel is accepted")
    };

    // The program shape is legal: a small instance completes on the default
    // per-call fuel.
    let mut model = instantiate(&artifacts().ecmascript);
    assert_eq!(
        model.evaluate_value(&nested_loops(10, 10), Some(&event)),
        Ok(number(100.0))
    );

    // The total (~10^10 loop bodies) is far beyond the default per-call fuel: the call
    // traps on fuel exhaustion and surfaces as the typed `Platform` failure
    // instead of hanging the host.
    let started = std::time::Instant::now();
    assert_eq!(
        model.evaluate_value(&nested_loops(per_loop, per_loop), Some(&event)),
        Err(DataModelFailure::Platform)
    );
    eprintln!(
        "fuel-bounded hostile expression stopped after {:?}",
        started.elapsed()
    );
    // A trapped instance is poisoned: every later call fails closed without
    // re-entering the guest.
    assert_eq!(
        model.evaluate_value("1 + 2", Some(&event)),
        Err(DataModelFailure::Platform)
    );

    // Fuel, not wall-clock time, is what stops it: a
    // 1000 x 1000 instance traps on the default budget yet completes with
    // the exact value when the per-call budget is raised a hundredfold.
    assert_eq!(
        with_fuel(DEFAULT_DATA_MODEL_CALL_FUEL)
            .evaluate_value(&nested_loops(1000, 1000), Some(&event)),
        Err(DataModelFailure::Platform)
    );
    assert_eq!(
        with_fuel(DEFAULT_DATA_MODEL_CALL_FUEL * 100)
            .evaluate_value(&nested_loops(1000, 1000), Some(&event)),
        Ok(number(1_000_000.0))
    );
}

#[test]
fn onentry_expression_during_start_sees_an_unbound_event() {
    let mut model = instantiate(&artifacts().ecmascript);
    // No current event: `_event` is unbound (SCXML 1.0 section 5.10), never
    // a fabricated event and never an `unavailable` failure.
    assert_eq!(
        model.evaluate_condition("typeof _event === 'undefined'", None),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_value("_name", None),
        Ok(DataValue::String("representative".into()))
    );
    model
        .execute("var entered = true;", None)
        .expect("no-event scripts execute");
    assert_eq!(model.evaluate_condition("entered", None), Ok(true));

    // The null provider evaluates no value expression and runs no script
    // (SCXML B.1.4, B.1.5), with or without an event.
    let mut null = instantiate(&artifacts().null);
    assert_eq!(
        null.evaluate_value("null", None),
        Err(DataModelFailure::Execution)
    );
    assert_eq!(null.execute("", None), Err(DataModelFailure::Execution));
}

/// The null component answers its only condition, `In()`, through the
/// execution-host import (SCXML B.1.2) and evaluates no value expression
/// (B.1.4 "There is no value expression language.") under Wasmtime.
#[test]
fn null_component_answers_in_and_evaluates_no_values() {
    let mut null = instantiate(&artifacts().null);
    null.update_active_configuration(&[], &["ps1".to_owned()])
        .expect("the host installs a configuration");
    assert_eq!(null.evaluate_condition("In('ps1')", None), Ok(true));
    assert_eq!(null.evaluate_condition("In('s1')", None), Ok(false));
    // A delta removes exited states and adds entered ones.
    null.update_active_configuration(&["ps1".to_owned()], &["s1".to_owned()])
        .expect("the host updates the configuration");
    assert_eq!(null.evaluate_condition("In('ps1')", None), Ok(false));
    assert_eq!(null.evaluate_condition("In('s1')", None), Ok(true));
    assert!(
        null.evaluate_condition("true", None).is_err(),
        "the null data model's only condition is In()"
    );
    for expression in ["'pass'", "null", ""] {
        assert_eq!(
            null.evaluate_value(expression, None),
            Err(DataModelFailure::Execution),
            "{expression:?}"
        );
    }
}

/// SCXML B.1 and B.2 define only `replacechildren`: the ECMAScript and null
/// components answer every other `XPath` placement (CR 2014-03-13 §C.3.6) with
/// `execution` (the machine's `error.execution`) and change no data.
#[test]
fn ecmascript_and_null_components_reject_xpath_placements() {
    let mut model = instantiate(&artifacts().ecmascript);
    model
        .execute("var x = 1;", None)
        .expect("scripts declare variables");
    for placement in [
        AssignPlacement::FirstChild,
        AssignPlacement::Delete,
        AssignPlacement::AddAttribute("a".into()),
    ] {
        assert_eq!(
            model.assign("x", &AssignSource::Value(number(2.0)), &placement, None),
            Err(DataModelFailure::Execution),
            "{placement:?}"
        );
    }
    assert_eq!(model.evaluate_value("x", None), Ok(number(1.0)));
    model
        .assign(
            "x",
            &AssignSource::Value(number(3.0)),
            &AssignPlacement::ReplaceChildren,
            None,
        )
        .expect("the default placement assigns");
    assert_eq!(model.evaluate_value("x", None), Ok(number(3.0)));
    // An expression source is evaluated by the data model with `_event`
    // bound to the call's event.
    model
        .assign(
            "x",
            &AssignSource::Expression("_event.name + x".into()),
            &AssignPlacement::ReplaceChildren,
            Some(&external("go")),
        )
        .expect("an expression source assigns");
    assert_eq!(
        model.evaluate_value("x", None),
        Ok(DataValue::String("go3".into()))
    );
    assert_eq!(
        model.assign(
            "x",
            &AssignSource::Expression("_event.name".into()),
            &AssignPlacement::ReplaceChildren,
            None,
        ),
        Err(DataModelFailure::Execution),
        "`_event` is unbound without a current event"
    );

    let mut null = instantiate(&artifacts().null);
    assert_eq!(
        null.assign(
            "x",
            &AssignSource::Value(DataValue::Null),
            &AssignPlacement::LastChild,
            None
        ),
        Err(DataModelFailure::Execution)
    );
}

/// The `XPath` component evaluates, assigns, answers `In()` through the
/// execution-host import, binds `_event` only when an event is current,
/// and fails closed for scripts, all under Wasmtime.
#[test]
fn xpath_component_evaluates_assigns_and_binds_events() {
    let mut xpath = xpath_with_data(&[
        ("Var1", DataValue::String("1".into())),
        (
            "Var2",
            DataValue::Xml("<item>a</item><item>b</item>".into()),
        ),
    ]);
    assert_eq!(
        xpath.evaluate_value("$Var1 + 1", None),
        Ok(DataValue::Number(
            FiniteNumber::new(2.0).expect("two is finite")
        ))
    );
    assert_eq!(
        xpath.evaluate_value("$Var2/item[2]", None),
        Ok(DataValue::Xml("<item>b</item>".into()))
    );
    // Results only a real XPath 1.0 evaluator over the declared tree yields:
    // node-set counting, string-value concatenation, a positional
    // predicate, and XPath number formatting.
    assert_eq!(
        xpath.evaluate_value("count($Var2/item)", None),
        Ok(number(2.0))
    );
    assert_eq!(
        xpath.evaluate_value("string($Var2)", None),
        Ok(DataValue::String("ab".into()))
    );
    assert_eq!(
        xpath.evaluate_value("concat($Var2/item[last()], '-', 7 div 2)", None),
        Ok(DataValue::String("b-3.5".into()))
    );
    assert_eq!(
        xpath.evaluate_value("$Var1 +", None),
        Err(DataModelFailure::InvalidInput),
        "malformed XPath is typed invalid input, as for ECMAScript"
    );
    assert_eq!(
        xpath.evaluate_condition("$_name = 'representative'", None),
        Ok(true)
    );
    assert_eq!(
        xpath.evaluate_condition("$_event/name", None),
        Err(DataModelFailure::Execution),
        "_event is unbound without a current event"
    );
    let event = Event::external(EventName::new("go").expect("valid event name"));
    assert_eq!(
        xpath.evaluate_condition("$_event/name = 'go'", Some(&event)),
        Ok(true)
    );
    xpath
        .assign(
            "$Var1",
            &AssignSource::Value(DataValue::String("7".into())),
            &AssignPlacement::ReplaceChildren,
            None,
        )
        .expect("a declared location assigns");
    assert_eq!(xpath.evaluate_condition("$Var1 = 7", None), Ok(true));
    // A node-set value replaces the location's children with a deep copy.
    xpath
        .assign(
            "$Var2",
            &AssignSource::Value(DataValue::Xml(
                r#"<item k="x">c</item><item k="y">d</item>"#.into(),
            )),
            &AssignPlacement::ReplaceChildren,
            None,
        )
        .expect("a node-set value assigns");
    assert_eq!(
        xpath.evaluate_value("$Var2/item[@k = 'y']/text()", None),
        Ok(DataValue::Xml("d".into()))
    );
    assert_eq!(
        xpath.evaluate_value("count($Var2/item)", None),
        Ok(number(2.0))
    );
    assert_eq!(
        xpath.assign(
            "$_sessionid",
            &AssignSource::Value(DataValue::String("x".into())),
            &AssignPlacement::ReplaceChildren,
            None
        ),
        Err(DataModelFailure::Execution),
        "system variables are read-only"
    );
    xpath
        .update_active_configuration(&[], &["s1".to_owned()])
        .expect("the host installs a configuration");
    assert_eq!(xpath.evaluate_condition("In('s1')", None), Ok(true));
    assert_eq!(xpath.evaluate_condition("In('s2')", None), Ok(false));
    assert_eq!(xpath.execute("", None), Ok(()));
    assert_eq!(
        xpath.execute("x = 1", None),
        Err(DataModelFailure::Execution),
        "the XPath data model defines no script language"
    );
}

/// SCXML 5.4 and SCXML 1.0 CR 2014-03-13 §C.3.6 through the `XPath` component
/// under Wasmtime: every placement crosses the WIT boundary with its inline XML
/// value, and an expression source is evaluated inside the data model with
/// `_event` bound.
#[test]
fn xpath_component_applies_placements_and_expression_sources() {
    let mut xpath = xpath_with_data(&[
        ("Var1", DataValue::String("1".into())),
        (
            "Var2",
            DataValue::Xml("<item>a</item><item>b</item>".into()),
        ),
    ]);
    let event = Event::external(EventName::new("go").expect("valid event name"));
    for (location, value, placement) in [
        (
            "$Var2",
            DataValue::Xml("<item>z</item>".into()),
            AssignPlacement::FirstChild,
        ),
        (
            "$Var2/item[3]",
            DataValue::Xml("<item>c</item>".into()),
            AssignPlacement::NextSibling,
        ),
        ("$Var2/item[1]", DataValue::Absent, AssignPlacement::Delete),
        (
            "$Var2/item[1]",
            DataValue::String("x".into()),
            AssignPlacement::AddAttribute("mark".into()),
        ),
    ] {
        xpath
            .assign(location, &AssignSource::Value(value), &placement, None)
            .unwrap_or_else(|failure| panic!("{placement:?} assigns: {failure:?}"));
    }
    assert_eq!(
        xpath.evaluate_condition(
            "count($Var2/item) = 3 and $Var2/item[1] = 'a' and $Var2/item[3] = 'c' \
             and $Var2/item[1]/@mark = 'x'",
            None
        ),
        Ok(true),
        "firstchild, nextsibling, delete, and addattribute applied in order"
    );
    xpath
        .assign(
            "$Var1",
            &AssignSource::Expression("$_event/name".into()),
            &AssignPlacement::ReplaceChildren,
            Some(&event),
        )
        .expect("an expression source evaluates with `_event` bound");
    assert_eq!(xpath.evaluate_condition("$Var1 = 'go'", None), Ok(true));
}

#[test]
fn representative_machine_on_the_component_data_model_reproduces_the_shared_trace() {
    let artifacts = artifacts();
    let fixture: serde_json::Value =
        serde_json::from_slice(&read(&artifacts.workspace.join(TRACE_FIXTURE)))
            .expect("the shared trace fixture is JSON");
    assert_eq!(
        fixture["ir-sha256"],
        format!("{:x}", Sha256::digest(&artifacts.representative_ir)),
        "the fixture was recorded against this IR"
    );
    let events = fixture["events"]
        .as_array()
        .expect("fixture events")
        .iter()
        .map(|event| event.as_str().expect("event names are strings"))
        .collect::<Vec<_>>();
    let trace = execute_machine_session(
        ComponentSource::Authenticated(&artifacts.machine),
        MachineIrSource::Runtime(&artifacts.representative_ir),
        &events,
        Box::new(instantiate(&artifacts.ecmascript)),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the session runs on the component data model");
    let steps = trace
        .results
        .iter()
        .map(|step| {
            serde_json::json!({
                "status": match step.status {
                    ComponentStatus::Ready => "ready",
                    ComponentStatus::Running => "running",
                    ComponentStatus::Finished => "finished",
                    ComponentStatus::Failed => "failed",
                    ComponentStatus::ShutDown => "shut-down",
                },
                "configuration": step.active_configuration,
                "logs": step.logs.iter().map(|record| {
                    record.label.as_ref().map_or_else(|| record.text.clone(), |label| format!("{label}: {}", record.text))
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(serde_json::Value::Array(steps), fixture["results"]);
}

#[test]
fn non_data_model_artifacts_are_rejected() {
    let artifacts = artifacts();
    assert!(matches!(
        ComponentDataModel::instantiate(
            &artifacts.machine,
            &[],
            &system("representative"),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
        ),
        Err(scxml_host_wasmtime::HostError::Artifact(
            scxml_host_wasmtime::ArtifactError::ArtifactKindMismatch
        ))
    ));
}

/// Runtime-built source (`eval` and the `Function`, `GeneratorFunction`,
/// and `AsyncFunction` constructors) compiles within the data model's parse
/// bounds; deep payloads are refused with the typed `Execution` failure
/// before they reach Boa's parser, so the instance is never trapped.
#[test]
fn dynamic_code_payloads_fail_closed_inside_the_ecmascript_component() {
    let mut model = instantiate(&artifacts().ecmascript);
    let event = external("go");
    for (payload, expected) in [
        ("(()=>0).constructor('return 7')()", number(7.0)),
        (
            "(function*(){}).constructor('yield 3')().next().value",
            number(3.0),
        ),
        ("eval('1 + 2')", number(3.0)),
        (
            "typeof Object.getPrototypeOf(async function(){}).constructor('return 1')",
            DataValue::String("function".into()),
        ),
    ] {
        assert_eq!(
            model.evaluate_value(payload, Some(&event)),
            Ok(expected),
            "{payload}"
        );
    }
    for payload in [
        "(()=>0).constructor('return ' + '['.repeat(40000))()",
        "(()=>0).constructor('return ' + '('.repeat(40000) + '1' + ')'.repeat(20000))()",
        "eval('['.repeat(40000))",
        "Object.getPrototypeOf(async function*(){}).constructor('yield ' + '['.repeat(40000))",
    ] {
        assert_eq!(
            model.evaluate_value(payload, Some(&event)),
            Err(DataModelFailure::Execution),
            "{payload}"
        );
    }
    // The instance was never trapped: it keeps evaluating.
    assert_eq!(model.evaluate_value("1 + 2", Some(&event)), Ok(number(3.0)));
}

/// A 32 000-element sequence hidden in an optional-chain index is
/// declined by the nesting scan before Boa parses it.
#[test]
fn optional_chain_sequence_payload_is_declined_inside_the_ecmascript_component() {
    let mut model = instantiate(&artifacts().ecmascript);
    let event = external("go");
    let script = format!("var a=[];a?.[{}1]", "1,".repeat(32_000));
    // Declined for nesting, not size.
    assert_eq!(
        model.execute(&script, Some(&event)),
        Err(DataModelFailure::InvalidInput)
    );
    assert_eq!(model.evaluate_value("1 + 2", Some(&event)), Ok(number(3.0)));
}

/// A translate/concat string amplification evaluated per node
/// exhausts the `XPath` evaluator's own fuel (typed `Execution`) long before
/// the store's per-call instruction fuel, and leaves the instance usable.
#[test]
fn xpath_string_amplification_exhausts_evaluator_fuel_inside_the_component() {
    let mut xpath = xpath_with_data(&[
        ("d", DataValue::String("a".repeat(30_000))),
        ("y", DataValue::Xml("<x/>".repeat(8_000))),
    ]);
    let from: String = ('\u{4E00}'..).take(4_000).collect();
    let parts = vec!["$d"; 16].join(",");
    let expression =
        format!("count($y/x[string-length(translate(concat({parts}),'{from}a','b'))=0])");
    let started = std::time::Instant::now();
    assert_eq!(
        xpath.evaluate_value(&expression, None),
        Err(DataModelFailure::Execution)
    );
    eprintln!(
        "XPath amplification stopped by evaluator fuel after {:?}",
        started.elapsed()
    );
    assert_eq!(
        xpath.evaluate_value("count($y/x)", None),
        Ok(number(8_000.0))
    );
}

/// A robot-style `XPath` document: numeric sensor readings, counters,
/// thresholds, a pose, a list, and nested waypoints.
const ROBOT_DATA: [(&str, &str); 7] = [
    ("battery", "87.5"),
    ("ticks", "0"),
    ("thresholds", "<low>20</low><high>80</high>"),
    (
        "sensors",
        r#"<sensor id="lidar"><range>2.5</range><min>0.2</min></sensor><sensor id="sonar"><range>1.25</range><min>0.1</min></sensor><sensor id="bump"><range>0</range><min>0</min></sensor>"#,
    ),
    ("pose", "<x>0</x><y>0</y><theta>0</theta>"),
    (
        "items",
        "<item>1</item><item>2</item><item>3</item><item>4</item><item>5</item><item>6</item><item>7</item><item>8</item><item>9</item><item>10</item>",
    ),
    (
        "path",
        r#"<wp n="1"><x>1</x><y>2</y></wp><wp n="2"><x>3</x><y>4</y></wp><wp n="3"><x>5</x><y>6</y></wp><wp n="4"><x>7</x><y>8</y></wp><wp n="5"><x>9</x><y>10</y></wp>"#,
    ),
];

/// One data-model call under test.
type XPathCall = Box<dyn Fn(&mut ComponentDataModel, &Event) -> Result<(), DataModelFailure>>;

fn xpath_assign(
    location: &'static str,
    source: AssignSource,
    placement: AssignPlacement,
) -> XPathCall {
    Box::new(move |model, event| model.assign(location, &source, &placement, Some(event)))
}

fn xpath_markup(markup: &str) -> AssignSource {
    AssignSource::Value(DataValue::Xml(markup.into()))
}

fn xpath_delete(location: &'static str) -> XPathCall {
    xpath_assign(
        location,
        AssignSource::Value(DataValue::Absent),
        AssignPlacement::Delete,
    )
}

/// One bounded operation: label, per-call fuel bound, the call, and the
/// call (run on the default budget) that restores the document after it.
type XPathBound = (&'static str, u64, XPathCall, Option<XPathCall>);

/// Per-call instruction fuel bounds for steady-state `XPath` evaluations
/// on the robot document, 19 to 28 percent above the largest call the
/// flat-arena store measures over 200 rounds. The released per-node store
/// needed 31 000 fuel for the guard, 24 800 for `sum()`, and 12 500 for the
/// arithmetic value, so it fails this test; the flat arena needs about
/// 19 800, 12 500, and 9 200.
fn xpath_evaluation_bounds() -> Vec<XPathBound> {
    let guard = "$battery > $thresholds/low and $sensors/sensor[@id='lidar']/range > 0.5";
    vec![
        (
            "condition",
            24_000,
            Box::new(move |model: &mut ComponentDataModel, event: &Event| {
                assert_eq!(model.evaluate_condition(guard, Some(event)), Ok(true));
                Ok(())
            }),
            None,
        ),
        (
            "value-number",
            11_000,
            Box::new(|model: &mut ComponentDataModel, event: &Event| {
                model
                    .evaluate_value("$ticks + $pose/x * 2", Some(event))
                    .map(drop)
            }),
            None,
        ),
        (
            "value-sum",
            16_000,
            Box::new(|model: &mut ComponentDataModel, event: &Event| {
                assert_eq!(
                    model.evaluate_value("sum($items/item)", Some(event)),
                    Ok(number(55.0))
                );
                Ok(())
            }),
            None,
        ),
        (
            "value-node-set",
            16_500,
            Box::new(|model: &mut ComponentDataModel, event: &Event| {
                model.evaluate_value("$pose", Some(event)).map(drop)
            }),
            None,
        ),
        (
            "in",
            10_000,
            Box::new(|model: &mut ComponentDataModel, event: &Event| {
                assert_eq!(
                    model.evaluate_condition("In('patrol') and not(In('dock'))", Some(event)),
                    Ok(true)
                );
                Ok(())
            }),
            None,
        ),
    ]
}

/// Per-call instruction fuel bounds for steady-state `XPath` assignments
/// and `<foreach>` item binding, 15 to 20 percent above the largest call
/// the flat-arena store measures over 200 rounds.
fn xpath_assignment_bounds() -> Vec<XPathBound> {
    vec![
        (
            "assign-replacechildren",
            32_000,
            xpath_assign(
                "$ticks",
                AssignSource::Expression("$ticks + 1".into()),
                AssignPlacement::ReplaceChildren,
            ),
            None,
        ),
        (
            "assign-firstchild",
            48_000,
            xpath_assign(
                "$items",
                xpath_markup("<item>0</item>"),
                AssignPlacement::FirstChild,
            ),
            Some(xpath_delete("$items/item[1]")),
        ),
        (
            "assign-lastchild",
            48_000,
            xpath_assign(
                "$items",
                xpath_markup("<item>11</item>"),
                AssignPlacement::LastChild,
            ),
            Some(xpath_delete("$items/item[last()]")),
        ),
        (
            "assign-previoussibling",
            54_000,
            xpath_assign(
                "$items/item[3]",
                xpath_markup("<item>2.5</item>"),
                AssignPlacement::PreviousSibling,
            ),
            Some(xpath_delete("$items/item[3]")),
        ),
        (
            "assign-nextsibling",
            54_000,
            xpath_assign(
                "$items/item[3]",
                xpath_markup("<item>3.5</item>"),
                AssignPlacement::NextSibling,
            ),
            Some(xpath_delete("$items/item[4]")),
        ),
        (
            "assign-replace",
            54_000,
            xpath_assign(
                "$items/item[5]",
                xpath_markup("<item>5</item>"),
                AssignPlacement::Replace,
            ),
            None,
        ),
        (
            "assign-delete",
            14_500,
            xpath_assign(
                "$items/item[1]",
                AssignSource::Value(DataValue::Absent),
                AssignPlacement::Delete,
            ),
            Some(xpath_assign(
                "$items",
                xpath_markup("<item>1</item>"),
                AssignPlacement::FirstChild,
            )),
        ),
        (
            "assign-addattribute",
            60_000,
            xpath_assign(
                "$pose",
                AssignSource::Expression("$ticks".into()),
                AssignPlacement::AddAttribute("stamp".into()),
            ),
            None,
        ),
        (
            "foreach-item",
            60_000,
            xpath_assign(
                "wp",
                AssignSource::ForeachValue(DataValue::Xml(
                    r#"<wp n="3"><x>5</x><y>6</y></wp>"#.into(),
                )),
                AssignPlacement::ReplaceChildren,
            ),
            None,
        ),
    ]
}

/// Steady-state `XPath` operations on a robot-style document (conditions,
/// values, `In()`, every `<assign>` placement, and `<foreach>` item
/// binding) each run within a small fixed bound of Wasm instruction fuel
/// and keep answering correctly. The bound is on the Wasm each call
/// executed; a call that grows the component's memory (its heap creeps a
/// 64 KiB page every 40 to 130 calls) copies it from its copy allowance
/// (`memory_growth_copies_are_bounded_by_the_calls_copy_allowance`), which
/// takes no Wasm fuel.
#[test]
fn xpath_operations_run_within_a_small_fuel_bound() {
    let event = external("tick");
    let declarations = ROBOT_DATA.map(|(id, _)| id.to_owned());
    for (label, bound, call, undo) in xpath_evaluation_bounds()
        .into_iter()
        .chain(xpath_assignment_bounds())
    {
        let mut model = ComponentDataModel::instantiate(
            &artifacts().xpath,
            &declarations,
            &system("robot"),
            &WasmtimeHostConfig::default(),
            &TelemetryProviders::new(),
        )
        .expect("the XPath component instantiates");
        for (id, markup) in ROBOT_DATA {
            model
                .initialize_data(id, &DataInitializer::Value(DataValue::Xml(markup.into())))
                .expect("robot data initializes");
        }
        model
            .update_active_configuration(&[], &["run".to_owned(), "patrol".to_owned()])
            .expect("the configuration installs");
        // One round parses and caches the expressions.
        call(&mut model, &event).expect("the first call succeeds");
        if let Some(undo) = &undo {
            undo(&mut model, &event).expect("the first undo succeeds");
        }
        for round in 0..30 {
            assert_eq!(call(&mut model, &event), Ok(()), "{label} round {round}");
            let ran = model.last_call_fuel().wasm;
            assert!(
                ran <= bound,
                "{label} round {round} ran {ran} fuel, over {bound}"
            );
            if let Some(undo) = &undo {
                assert_eq!(
                    undo(&mut model, &event),
                    Ok(()),
                    "{label} undo round {round}"
                );
            }
        }
    }
}

/// Every linear memory is exactly its size and moves when it grows, so each
/// growth copies the whole memory in the host; one call may copy at most its
/// fuel times `MEMORY_GROWTH_COPY_BYTES_PER_FUEL` bytes, an allowance kept
/// apart from the Wasm fuel. An expression that keeps adding 60 KB strings
/// grows the memory a little at a time toward the store's 64 MiB; the call
/// stops after copying at most its allowance, and twice the fuel copies more
/// but still at most twice as much. A script that allocates 40,000 small
/// objects completes on the default fuel, and the Wasm fuel it runs is the same
/// however much fuel the call has, so copies take nothing from it; a loop that
/// does not grow the memory copies nothing. Each run prints its Wasm fuel, its
/// copied bytes and its time, so the host time per fuel of Wasm (the loop) and
/// per copied byte can be read off.
#[test]
fn memory_growth_copies_are_bounded_by_the_calls_copy_allowance() {
    use scxml_host_wasmtime::MEMORY_GROWTH_COPY_BYTES_PER_FUEL;
    const STRINGS: &str = "(() => { const s = 'x'.repeat(60000); const a = []; for (let i = 0; i < 5000; i++) a.push(s + i); return a.length; })()";
    const OBJECTS: &str = "(() => { const a = []; for (let i = 0; i < 40000; i++) a.push({ i, s: 'v' + i }); return a.length; })()";
    const LOOP: &str = "(() => { let x = 0, y = 0, z = 0; for (let i = 0; i < 90000; i++) { x = (x * 31 + i) % 7919; y = (y + x * i) % 104729; z = (z ^ (x + y)) & 65535; } return z; })()";
    let event = external("go");
    let run = |expression: &str, fuel: u64| {
        let mut model = instantiate(&artifacts().ecmascript)
            .with_call_fuel(fuel)
            .expect("non-zero fuel is accepted");
        let before = model.memory_growth_copied_bytes();
        let started = std::time::Instant::now();
        let result = model.evaluate_value(expression, Some(&event));
        let elapsed = started.elapsed();
        let copied = model.memory_growth_copied_bytes() - before;
        let spent = model.last_call_fuel();
        eprintln!(
            "{fuel} fuel: {:?}, {} Wasm fuel, {copied} bytes copied, {elapsed:?}",
            result.as_ref().map(|_| ()),
            spent.wasm,
        );
        assert_eq!(spent.memory_growth_bytes, copied, "every copy is counted");
        assert!(
            copied <= fuel * MEMORY_GROWTH_COPY_BYTES_PER_FUEL,
            "{copied} bytes copied on {fuel} fuel"
        );
        (result, copied, spent)
    };
    let fuel = DEFAULT_DATA_MODEL_CALL_FUEL;
    let (once, copied_once, _) = run(STRINGS, fuel);
    assert_eq!(
        once,
        Err(DataModelFailure::Platform),
        "the call runs out of its copy allowance"
    );
    let (twice, copied_twice, _) = run(STRINGS, 2 * fuel);
    assert_eq!(twice, Err(DataModelFailure::Platform));
    assert!(
        copied_once < copied_twice,
        "{copied_once} then {copied_twice} bytes copied"
    );
    let (objects, copied_objects, spent_objects) = run(OBJECTS, fuel);
    assert_eq!(objects, Ok(number(40_000.0)));
    assert!(copied_objects > 0, "the objects grow the memory");
    let (_, _, spent_objects_more_fuel) = run(OBJECTS, 4 * fuel);
    assert_eq!(
        spent_objects.wasm, spent_objects_more_fuel.wasm,
        "copies take no Wasm fuel"
    );
    let (looped, copied_loop, spent_loop) = run(LOOP, fuel);
    assert!(looped.is_ok(), "{looped:?}");
    assert_eq!(
        copied_loop, 0,
        "a loop that does not grow the memory copies nothing"
    );
    assert!(spent_loop.wasm > 0, "the loop runs Wasm");
}
