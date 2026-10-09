//! Data-model assignment semantics of the shipped components.
//!
//! Document-level tests run a compiled document through the virtualized
//! `scxml_machine` component under Wasmtime (runtime `ir-source` mode),
//! linked to the virtualized data-model component, and require the official
//! `Outcome` oracle result `pass`. The data model is initialized with the
//! declarative data each document's `<datamodel>` declares, as a machine
//! binds early data. Operation-level tests drive each data-model
//! component's WIT `assign` export directly and require the exact result.
//!
//! The components are built and verified by
//! `scripts/verify-wasm-capabilities.sh` into this suite's own artifact
//! directory (`target/data-model-proof/wasm-artifacts/data-model-semantics`),
//! sharing the Cargo target with `data_model_component.rs`.

#![cfg(feature = "wasmtime")]

mod prebuilt;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use scxml_host_wasmtime::{
    ArtifactKind, AuthenticatedComponent, ComponentDataModel, ComponentSource, DataModelFailure,
    DataModelSystemValues, MachineDataModel, MachineIrSource, TelemetryProviders,
    TrustedManifestDigest, WasmtimeHostConfig, authenticate_component, execute_machine_session,
};
use scxml_types::{
    AssignPlacement, AssignSource, DataInitializer, DataValue, Event, EventName, FiniteNumber,
};
use sha2::{Digest as _, Sha256};

/// The source id of the documents these tests parse.
const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

struct Artifacts {
    workspace: PathBuf,
    ecmascript: AuthenticatedComponent,
    xpath: AuthenticatedComponent,
    machine: AuthenticatedComponent,
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
    // Same artifact set and Cargo target as `data_model_component.rs`; this
    // suite's own artifact directory, so concurrent suites never rewrite a
    // manifest the other is reading.
    let release = prebuilt::release_dir().unwrap_or_else(|| {
        let target = workspace.join("target/data-model-proof");
        let release = target.join("wasm-artifacts/data-model-semantics");
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
            .env("SCXML_WASM_ARTIFACTS_DIR", &release)
            .status()
            .expect("the capability script runs");
        assert!(status.success(), "data-model components must verify");
        release
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
        xpath: authenticate(ArtifactKind::DataModelXpath, "scxml_data_model_xpath"),
        machine: authenticate(ArtifactKind::Machine, "scxml_machine"),
        workspace,
    }
}

/// The prepared official case `test<case>.scxml` for `datamodel`.
fn official_case(datamodel: &str, case: u32) -> String {
    let artifacts = artifacts()
        .workspace
        .join("tests/conformance/w3c-irp/prepared")
        .join(datamodel)
        .join("artifacts");
    let wanted = format!("test{case}.scxml");
    let mut found = Vec::new();
    for corpus in std::fs::read_dir(&artifacts).expect("the prepared corpus exists") {
        let corpus = corpus.expect("corpus entry").path();
        for group in std::fs::read_dir(&corpus).expect("corpus directory") {
            let path = group
                .expect("group entry")
                .path()
                .join("cases")
                .join(case.to_string())
                .join(&wanted);
            if path.is_file() {
                found.push(path);
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "exactly one prepared {datamodel} case {case}"
    );
    String::from_utf8(read(&found[0])).expect("prepared cases are UTF-8")
}

/// `Outcome` log values the machine component records on the component
/// data model initialized with `data`.
fn component_outcomes(
    source: &str,
    data_model: &AuthenticatedComponent,
    data: Data<'_>,
) -> Vec<String> {
    let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
        SOURCE,
        "semantics.scxml",
        source,
    ))
    .expect("the document parses");
    let ir = scxml_compiler_core::lower(&document)
        .expect("the document lowers")
        .to_json()
        .expect("the lowered IR is canonical");
    let model = component(data_model, data);
    let trace = execute_machine_session(
        ComponentSource::Authenticated(&artifacts().machine),
        MachineIrSource::Runtime(&ir),
        &[],
        Box::new(model),
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the machine component runs the document");
    trace
        .results
        .iter()
        .flat_map(|step| &step.logs)
        .filter(|record| record.label.as_deref() == Some("Outcome"))
        .map(|record| record.text.clone())
        .collect()
}

fn assert_passes(name: &str, source: &str, data_model: &AuthenticatedComponent, data: Data<'_>) {
    assert_eq!(
        component_outcomes(source, data_model, data),
        ["pass"],
        "{name}: the machine and data-model components pass"
    );
}

/// W3C 150, 151, and 152 (SCXML 4.6): `<foreach>` declares an undeclared
/// `item` and `index`, reuses declared ones, and rejects an illegal item
/// or array with `error.execution` before running the body.
#[test]
fn official_ecmascript_foreach_declaration_cases_pass() {
    let ecmascript = &artifacts().ecmascript;
    for case in [150, 151] {
        assert_passes(
            &format!("ecmascript {case}"),
            &official_case("ecmascript", case),
            ecmascript,
            &[
                ("Var1", Some(DataValue::Null)),
                ("Var2", Some(DataValue::Null)),
                ("Var3", Some(DataValue::JsonArray("[1,2,3]".into()))),
            ],
        );
    }
    assert_passes(
        "ecmascript 152",
        &official_case("ecmascript", 152),
        ecmascript,
        &[
            ("Var1", Some(number(0.0))),
            ("Var2", Some(DataValue::Null)),
            ("Var3", Some(DataValue::Null)),
            ("Var4", Some(number(7.0))),
            ("Var5", Some(DataValue::JsonArray("[1,2,3]".into()))),
        ],
    );
}

/// SCXML 1.0 CR 2014-03-13 §C.3.6: the value-less `delete` placement. Lowering admits an
/// `<assign>` with neither `expr` nor children only for `delete`; the
/// machine hands the data model the shared engine source (`absent`).
#[test]
fn xpath_value_less_delete_passes() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"
        initial="s0" datamodel="xpath">
      <datamodel><data id="Var1"><a xmlns=""/><b xmlns=""/></data></datamodel>
      <state id="s0">
        <onentry><assign location="$Var1/a" type="delete"/></onentry>
        <transition cond="count($Var1/*) = 1 and count($Var1/b) = 1" target="pass"/>
        <transition target="fail"/>
      </state>
      <final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
      <final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
    </scxml>"#;
    assert_passes(
        "xpath value-less delete",
        source,
        &artifacts().xpath,
        &[("Var1", Some(DataValue::Xml("<a/><b/>".into())))],
    );
}

/// SCXML 5.4 and B.2: an `<assign location>` is an `XPath` location
/// expression that must select a node. A plain name is a relative path
/// from the `<datamodel>` context node, not a variable: it selects nothing,
/// so the assignment raises `error.execution` and creates no variable.
#[test]
fn xpath_plain_name_assign_location_passes() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"
        initial="s0" datamodel="xpath">
      <datamodel><data id="Var1"/></datamodel>
      <state id="s0">
        <onentry>
          <assign location="Var2" expr="'x'"/>
          <raise event="foo"/>
        </onentry>
        <transition event="error.execution" target="pass"/>
        <transition event="*" target="fail"/>
      </state>
      <final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
      <final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
    </scxml>"#;
    assert_passes(
        "xpath plain-name assign",
        source,
        &artifacts().xpath,
        &[("Var1", None)],
    );
}

/// SCXML 1.0 §5.9.4: an ill-formed expression MAY wait and raise
/// `error.execution` "at runtime when the expressions are evaluated". An
/// empty required expression attribute (`<if cond>`, `<foreach array/item>`,
/// `<assign location>`) is compiled (the `SCXML066` warning), and each one
/// raises `error.execution` when it runs.
#[test]
fn empty_required_expression_attributes_raise_error_execution() {
    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"
        initial="s0" datamodel="ecmascript">
      <datamodel><data id="errors" expr="0"/></datamodel>
      <state id="s0">
        <onentry><if cond=""><log label="Outcome" expr="'fail'"/></if></onentry>
        <onentry><foreach array="" item="x"/></onentry>
        <onentry><foreach array="[1]" item=""/></onentry>
        <onentry><assign location="" expr="1"/></onentry>
        <transition event="error.execution" cond="errors &lt; 3">
          <assign location="errors" expr="errors + 1"/>
        </transition>
        <transition event="error.execution" target="pass"/>
        <transition event="*" target="fail"/>
      </state>
      <final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
      <final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
    </scxml>"#;
    assert_passes(
        "empty required expression attributes",
        source,
        &artifacts().ecmascript,
        &[("errors", Some(number(0.0)))],
    );
}

// ---------------------------------------------------------------------------
// Operation-level semantics: each data-model component's WIT `assign` export.

/// Declared `<data>` ids in document order, each with its initial value
/// (`None` leaves the provider's placeholder).
type Data<'a> = &'a [(&'a str, Option<DataValue>)];

/// A data-model component that declares every id (`initialize`) and then
/// values each valued one (`initialize-data`), as a machine binds early data.
fn component(data_model: &AuthenticatedComponent, data: Data<'_>) -> ComponentDataModel {
    let declarations = data
        .iter()
        .map(|(id, _)| (*id).to_owned())
        .collect::<Vec<_>>();
    let mut model = ComponentDataModel::instantiate(
        data_model,
        &declarations,
        &DataModelSystemValues {
            session_id: "machine-session".into(),
            machine_name: Some("semantics".into()),
            io_processors: DataValue::canonical_json_object("{}")
                .expect("empty I/O processor object is canonical"),
        },
        &WasmtimeHostConfig::default(),
        &TelemetryProviders::new(),
    )
    .expect("the data-model component initializes");
    for (id, value) in data {
        if let Some(value) = value {
            model
                .initialize_data(id, &DataInitializer::Value(value.clone()))
                .expect("the data-model component values the declared data");
        }
    }
    model
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("finite"))
}

/// One assignment and the result the component must produce.
type Operation = (
    &'static str,
    AssignSource,
    AssignPlacement,
    Result<(), DataModelFailure>,
);

/// SCXML 4.6 and 5.4 in the ECMAScript data model: only a `<foreach>`
/// binding declares its variable; an `<assign>` to an undeclared root is
/// not a valid location (strict ECMAScript assignment: `error.execution`)
/// and declares nothing.
#[test]
fn ecmascript_assign_export_declares_only_foreach_bindings() {
    let data: Data<'_> = &[("x", Some(number(1.0)))];
    let mut wasm = component(&artifacts().ecmascript, data);
    let event = Event::external(EventName::new("go").expect("valid event name"));
    let operations: [Operation; 7] = [
        (
            "item",
            AssignSource::ForeachValue(number(4.0)),
            AssignPlacement::ReplaceChildren,
            Ok(()),
        ),
        (
            "fresh",
            AssignSource::Value(number(1.0)),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::Execution),
        ),
        (
            "fresh",
            AssignSource::Expression("1".into()),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::Execution),
        ),
        (
            "'continue'",
            AssignSource::ForeachValue(number(1.0)),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::InvalidInput),
        ),
        (
            "missing.member",
            AssignSource::ForeachValue(number(1.0)),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::InvalidInput),
        ),
        (
            "x",
            AssignSource::Expression("x + item + _event.name.length".into()),
            AssignPlacement::ReplaceChildren,
            Ok(()),
        ),
        (
            "x",
            AssignSource::Value(number(2.0)),
            AssignPlacement::FirstChild,
            Err(DataModelFailure::Execution),
        ),
    ];
    for (location, source, placement, expected) in operations {
        let result = wasm.assign(location, &source, &placement, Some(&event));
        assert_eq!(result, expected, "component {location} {source:?}");
    }
    assert_eq!(wasm.evaluate_value("item", None), Ok(number(4.0)));
    assert_eq!(wasm.evaluate_value("x", None), Ok(number(7.0)));
    assert_eq!(
        wasm.evaluate_value("typeof fresh", None),
        Ok(DataValue::String("undefined".into()))
    );
}

/// SCXML 4.6, 5.4, and 1.0 CR 2014-03-13 §C.3 in the `XPath` data model: a `<foreach>` binding
/// declares its variable by name; an `<assign>` location is always a
/// location expression, so a plain name selects nothing and declares no
/// variable.
#[test]
fn xpath_assign_export_declares_only_foreach_bindings() {
    let data: Data<'_> = &[
        ("Var1", Some(DataValue::String("1".into()))),
        ("Var2", Some(DataValue::Xml("<item>a</item>".into()))),
    ];
    let mut wasm = component(&artifacts().xpath, data);
    let event = Event::external(EventName::new("go").expect("valid event name"));
    let operations: [Operation; 7] = [
        (
            "Var3",
            AssignSource::Value(DataValue::String("x".into())),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::Execution),
        ),
        (
            "Var3",
            AssignSource::Expression("'x'".into()),
            AssignPlacement::ReplaceChildren,
            Err(DataModelFailure::Execution),
        ),
        (
            "Var4",
            AssignSource::ForeachValue(DataValue::Xml("<item>b</item>".into())),
            AssignPlacement::ReplaceChildren,
            Ok(()),
        ),
        (
            "Var5",
            AssignSource::ForeachValue(number(0.0)),
            AssignPlacement::ReplaceChildren,
            Ok(()),
        ),
        (
            "Var6",
            AssignSource::ForeachValue(number(1.0)),
            AssignPlacement::FirstChild,
            Err(DataModelFailure::InvalidInput),
        ),
        (
            "$Var1",
            AssignSource::Expression("concat($Var1, $_event/name)".into()),
            AssignPlacement::ReplaceChildren,
            Ok(()),
        ),
        (
            "$Var2/item",
            AssignSource::Value(DataValue::Absent),
            AssignPlacement::Delete,
            Ok(()),
        ),
    ];
    for (location, source, placement, expected) in operations {
        let result = wasm.assign(location, &source, &placement, Some(&event));
        assert_eq!(result, expected, "component {location} {source:?}");
    }
    assert_eq!(
        wasm.evaluate_value("string($Var4)", None),
        Ok(DataValue::String("b".into()))
    );
    assert_eq!(
        wasm.evaluate_value("$Var5 = 0", None),
        Ok(DataValue::Bool(true))
    );
    assert_eq!(
        wasm.evaluate_value("count($Var2/item)", None),
        Ok(number(0.0))
    );
    assert_eq!(
        wasm.evaluate_value("string($Var1)", None),
        Ok(DataValue::String("1go".into()))
    );
    assert!(
        wasm.evaluate_value("$Var3", None).is_err(),
        "a plain-name <assign> declares no variable"
    );
}
