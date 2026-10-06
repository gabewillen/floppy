//! The removed ECMAScript globals are enforced by the realm, not by the
//! program text.
//!
//! `Promise`, `Proxy`, `WebAssembly`, `Atomics`, and `SharedArrayBuffer`
//! (none of them part of the ECMAScript 3rd edition that B.2 requires) are
//! unavailable: promise jobs, proxy traps, and shared memory have no
//! deterministic place in the data model. That is a
//! property of the evaluation realm: a real reference fails at evaluation
//! (`error.execution`), while program text that merely *contains* those
//! letters — an identifier such as `endDate`, a string such as `'evaluate'`,
//! a property name, or a comment — is ordinary ECMAScript and evaluates.
//! `eval` and `Function` are standard globals (`dynamic_code.rs`).

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataValue, Event, EventName, FiniteNumber};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize(
            r#"{"endDate":5,"myFunction":1,"obj":{"Date":2,"evaluate":3}}"#,
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("the data model initializes");
    model
}

fn event(name: &str) -> Event {
    Event::external(EventName::new(name.to_owned()).expect("static event name is valid"))
}

fn condition(expression: &str) -> Result<bool, DataModelError> {
    model().evaluate_condition(expression, &event("evaluate"), &mut NoStates)
}

fn value(expression: &str) -> Result<DataValue, DataModelError> {
    model().evaluate_value(expression, &event("evaluate"), &mut NoStates)
}

fn text(value: &str) -> DataValue {
    DataValue::String(value.into())
}

#[test]
fn guards_that_only_spell_a_removed_name_are_faithful_predicates() {
    // PAT-GUARD-001: each of these is a valid guard whose value is `true`.
    for guard in [
        "endDate > 0",
        "_event.name == 'evaluate'",
        "myFunction === 1",
        "obj.Date === 2 && obj.evaluate === 3",
        "/* not eval, not Date */ true",
        "'Math.random' !== 'Promise'",
        "({ Function: 1, WebAssembly: 2 }).Function === 1",
    ] {
        assert_eq!(condition(guard), Ok(true), "{guard}");
    }
    assert_eq!(
        value("['Date', 'eval', 'import('].join(' ')"),
        Ok(text("Date eval import("))
    );
    let mut model = model();
    model
        .assign_expression("endDate", "endDate + 1", &event("e"), &mut NoStates)
        .expect("an assignment naming endDate evaluates");
    model
        .execute("var evaluated = 'Function';", &event("e"), &mut NoStates)
        .expect("a script spelling a removed name executes");
    assert_eq!(
        model.evaluate_value("[endDate, evaluated]", &event("e"), &mut NoStates),
        Ok(DataValue::JsonArray("[6,\"Function\"]".into()))
    );
}

#[test]
fn removed_globals_are_undefined_in_the_realm() {
    for name in [
        "Promise",
        "Proxy",
        "WebAssembly",
        "Atomics",
        "SharedArrayBuffer",
    ] {
        assert_eq!(
            value(&format!("typeof {name}")),
            Ok(text("undefined")),
            "{name}"
        );
        assert_eq!(
            value(&format!("typeof globalThis[{name:?}]")),
            Ok(text("undefined")),
            "{name}"
        );
    }
}

#[test]
fn using_a_removed_global_is_an_execution_error() {
    for expression in [
        "Promise.resolve(1)",
        "new SharedArrayBuffer(8)",
        "Atomics.add(new Int32Array(1), 0, 1)",
    ] {
        assert_eq!(
            value(expression),
            Err(DataModelError::Execution),
            "{expression}"
        );
    }
    // A guard using a removed global is false with `error.execution`.
    assert_eq!(
        condition("Promise.resolve(1) !== 0"),
        Err(DataModelError::Execution)
    );
    // The realm is unchanged for later evaluations.
    assert_eq!(
        value("endDate"),
        Ok(DataValue::Number(FiniteNumber::new(5.0).expect("finite")))
    );
}
