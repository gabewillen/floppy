//! Location expressions (SCXML 1.0 B.2.4, B.2.7, 5.4).
//!
//! * B.2.4: "The SCXML Processor MUST accept any ECMAScript left-hand-side
//!   expression as a location expression."
//! * B.2.7: "When evaluating an <assign> element in the ECMAScript data
//!   model, the SCXML Processor MUST replace the existing value at
//!   'location' with the value produced by evaluating 'expr'. If it is
//!   unable to do so (for example, if the <assign> element attempts to
//!   assign to a read-only attribute), it MUST place the error
//!   error.execution on the internal event queue."
//! * 5.4: "If the location expression does not denote a valid location in
//!   the data model ... the SCXML Processor MUST place the error
//!   'error.execution' in the internal event queue."

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{AssignPlacement, AssignSource, DataValue, FiniteNumber};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["a", "i", "obj", "key", "s"],
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
        .execute_without_event(
            "a = [0, 0, 0]; i = 0; obj = { inner: { k1: { leaf: 0 } } }; key = 'k1'; s = 'abc'; \
             \u{f1} = { n: 0 };",
            &mut NoStates,
        )
        .expect("the fixture script runs");
    model
}

fn assign(
    model: &mut EcmaScriptDataModel,
    location: &str,
    expression: &str,
) -> Result<(), DataModelError> {
    model.apply_assign(
        location,
        &AssignSource::Expression(expression.into()),
        &AssignPlacement::ReplaceChildren,
        None,
        &mut NoStates,
    )
}

fn value(model: &EcmaScriptDataModel, expression: &str) -> Result<DataValue, DataModelError> {
    model.evaluate_value_without_event(expression, &mut NoStates)
}

#[test]
fn any_left_hand_side_expression_is_a_location() {
    let mut model = model();
    for (location, expression) in [
        ("a[i+1]", "1"),
        ("a[ 2 ]", "2"),
        ("(a)[0]", "3"),
        ("obj.inner[key].leaf", "4"),
        ("obj['x y']", "5"),
        ("obj.inner['k' + (i + 2)]", "6"),
        ("\u{f1}.n", "7"),
        ("a[a.length - 3]", "a[0] + 10"),
    ] {
        assert_eq!(
            assign(&mut model, location, expression),
            Ok(()),
            "{location}"
        );
    }
    assert_eq!(
        value(
            &model,
            "[a, obj.inner.k1.leaf, obj['x y'], obj.inner.k2, \u{f1}.n]"
        ),
        Ok(DataValue::JsonArray("[[13,1,2],4,5,6,7]".into()))
    );
}

#[test]
fn a_location_that_cannot_be_assigned_is_an_execution_error_and_changes_nothing() {
    let mut model = model();
    for location in [
        "obj.missing.deep",
        "s.length",
        "f()",
        "a + 1",
        "nope",
        "nope.x",
        "_event",
        "_sessionid.x",
        "a[",
    ] {
        assert!(
            assign(&mut model, location, "9").is_err(),
            "{location} must fail"
        );
        assert!(
            model
                .apply_assign(
                    location,
                    &AssignSource::Value(DataValue::Number(
                        FiniteNumber::new(9.0).expect("finite")
                    )),
                    &AssignPlacement::ReplaceChildren,
                    None,
                    &mut NoStates,
                )
                .is_err(),
            "{location} must fail for a materialized value"
        );
    }
    assert_eq!(
        value(&model, "[a, obj, s, typeof nope]"),
        Ok(DataValue::JsonArray(
            r#"[[0,0,0],{"inner":{"k1":{"leaf":0}}},"abc","undefined"]"#.into()
        ))
    );
}
