//! `<foreach>` over ECMAScript arrays (SCXML 1.0 4.6 and B.2.11).
//!
//! * 4.6: "The SCXML processor MUST act as if it has made a shallow copy of
//!   the collection produced by the evaluation of 'array'."
//! * B.2.11: "In the ECMAScript data model, the SCXML Processor MUST support
//!   iteration over objects that satisfy instanceof(Array) in ECMAScript."
//!   Its note: the loop "is equivalent to `item = array_name[index]`" on that
//!   copy, and 4.6: "the assigned value MAY be null or undefined if the
//!   collection contains a null or undefined item."
//!
//! These tests drive the protocol the machine follows: `foreach_items`,
//! then per item the `item` binding, the `index` binding, and the body.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, ForeachItems, SystemContext,
};
use scxml_types::{AssignPlacement, AssignSource, DataValue, FiniteNumber};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn model(script: &str) -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["arr", "log"],
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
        .execute_without_event(script, &mut NoStates)
        .expect("the fixture script runs");
    model
}

fn items(model: &mut EcmaScriptDataModel, expression: &str) -> Vec<DataValue> {
    match model.foreach_items(expression, None, &mut NoStates) {
        Ok(ForeachItems::Items(items)) => items,
        other => panic!("{expression}: {other:?}"),
    }
}

fn bind(model: &mut EcmaScriptDataModel, name: &str, value: &DataValue) {
    model
        .apply_assign(
            name,
            &AssignSource::ForeachValue(value.clone()),
            &AssignPlacement::ReplaceChildren,
            None,
            &mut NoStates,
        )
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
}

fn index(position: usize) -> DataValue {
    DataValue::Number(
        FiniteNumber::new(f64::from(u32::try_from(position).expect("small"))).expect("finite"),
    )
}

fn check(model: &EcmaScriptDataModel, guard: &str) {
    assert_eq!(
        model.evaluate_condition_without_event(guard, &mut NoStates),
        Ok(true),
        "{guard}"
    );
}

#[test]
fn each_item_is_the_element_itself() {
    let mut model = model("arr = [{ k: 1 }, { k: 2 }];");
    let images = items(&mut model, "arr");
    assert_eq!(
        images,
        vec![
            DataValue::JsonObject(r#"{"k":1}"#.into()),
            DataValue::JsonObject(r#"{"k":2}"#.into())
        ]
    );
    for (position, image) in images.iter().enumerate() {
        bind(&mut model, "item", image);
        bind(&mut model, "index", &index(position));
        check(&model, "item === arr[index]");
        // After a body script the item is still the element.
        model
            .execute_without_event("item.k = item.k * 10;", &mut NoStates)
            .expect("the body runs");
        check(&model, "item === arr[index]");
    }
    check(&model, "arr[0].k === 10 && arr[1].k === 20");
}

#[test]
fn the_loop_runs_over_a_shallow_copy() {
    let mut model = model("arr = [{ k: 1 }, { k: 2 }]; var second = arr[1];");
    let images = items(&mut model, "arr");
    bind(&mut model, "item", &images[0]);
    model
        .execute_without_event("arr[1] = 'replaced'; arr.push(3);", &mut NoStates)
        .expect("the body changes the array");
    bind(&mut model, "item", &images[1]);
    check(
        &model,
        "item === second && item.k === 2 && arr.length === 3",
    );
}

#[test]
fn any_element_is_an_item() {
    let mut model = model(
        "arr = [function () { return 1; }, undefined, , NaN, 5n, Symbol.iterator, null, -0, \
                { u: undefined }, [1, , 3]];",
    );
    let images = items(&mut model, "arr");
    assert_eq!(images.len(), 10);
    let guards = [
        "typeof item === 'function' && item() === 1",
        "item === undefined",
        "item === undefined",
        "Number.isNaN(item)",
        "item === 5n",
        "item === Symbol.iterator",
        "item === null",
        "Object.is(item, -0)",
        "item === arr[8] && 'u' in item",
        "item === arr[9] && !(1 in item)",
    ];
    for (position, image) in images.iter().enumerate() {
        bind(&mut model, "item", image);
        check(&model, guards[position]);
    }
}

#[test]
fn nested_loops_bind_their_own_items() {
    let mut model = model("arr = [{ n: 'a' }, { n: 'b' }]; log = [];");
    let outer = items(&mut model, "arr");
    for image in &outer {
        bind(&mut model, "item", image);
        let inner = items(&mut model, "[{ m: 1 }, { m: 2 }]");
        for inner_image in &inner {
            bind(&mut model, "inner", inner_image);
            model
                .execute_without_event("log.push(item.n + inner.m);", &mut NoStates)
                .expect("the inner body runs");
        }
        check(&model, "arr.indexOf(item) >= 0");
    }
    check(&model, "log.join() === 'a1,a2,b1,b2'");
}

/// An array's item count has no bound of the data model's own.
#[test]
fn large_arrays_are_collections() {
    let mut model = model("arr = [1];");
    assert_eq!(items(&mut model, "new Array(10001)").len(), 10_001);
}

#[test]
fn only_arrays_are_collections() {
    let mut model = model("arr = [1];");
    for (expression, expected) in [
        ("({ length: 1, 0: 'x' })", ForeachItems::Illegal),
        ("'text'", ForeachItems::Illegal),
    ] {
        assert_eq!(
            model.foreach_items(expression, None, &mut NoStates),
            Ok(expected),
            "{expression}"
        );
    }
    assert_eq!(
        items(
            &mut model,
            "Object.setPrototypeOf({ length: 2, 0: 'a', 1: 'b' }, Array.prototype)"
        ),
        vec![DataValue::String("a".into()), DataValue::String("b".into())],
        "an object that satisfies instanceof(Array) is iterated"
    );
    assert_eq!(items(&mut model, "arr"), vec![index(1)]);
}

/// B.2.8.1: "Otherwise, the Processor MUST treat the content as a
/// space-normalized string literal and assign it as the value of
/// _event.data." Inline `<content>` text is space-normalized over XML white
/// space (`XPath` 1.0 `normalize-space()`), keeping other characters such as
/// U+00A0 NO-BREAK SPACE.
#[test]
fn inline_content_text_is_space_normalized_over_xml_white_space() {
    assert_eq!(
        scxml_data_model_ecmascript::inline_content(
            scxml_data_model_ecmascript::InlineContent::Text("\t this\u{a0} is \r\n  a  string \n")
        ),
        Some(scxml_types::DataValue::String(
            "this\u{a0} is a string".into()
        ))
    );
}

/// Loops nest as deep as the document nests them: with 40 loops in
/// progress the outermost still binds each element itself. The data model
/// tracks loops without a bound of its own.
#[test]
fn deeply_nested_loops_keep_binding_the_elements_themselves() {
    let mut model = model("arr = [{ n: 'a' }, { n: 'b' }];");
    let outer = items(&mut model, "arr");
    bind(&mut model, "item", &outer[0]);
    for level in 0..40 {
        let inner = items(
            &mut model,
            &format!("[{{ m: {level}, i: 0 }}, {{ m: {level}, i: 1 }}]"),
        );
        bind(&mut model, "inner", &inner[0]);
    }
    bind(&mut model, "item", &outer[1]);
    check(&model, "item === arr[1]");
}

/// A loop an error ended early never binds its last item, so it stays in
/// progress until the machine call that ran it returns and the machine
/// releases every loop in progress (WIT `data-model-api.release-foreach`):
/// no count bounds them, and 1,000 such loops leave none behind, and the next loop binds the elements themselves.
#[test]
fn loops_an_error_ended_are_released_when_the_machine_call_returns() {
    let mut model = model("arr = [{ k: 1 }, { k: 2 }];");
    for _ in 0..1_000 {
        let first = items(&mut model, "arr")
            .into_iter()
            .next()
            .expect("two items");
        bind(&mut model, "item", &first);
    }
    assert_eq!(model.foreach_in_progress(), Ok(1_000));
    model.release_foreach().expect("the loops are released");
    assert_eq!(model.foreach_in_progress(), Ok(0));
    for (position, item) in items(&mut model, "arr").iter().enumerate() {
        bind(&mut model, "item", item);
        bind(&mut model, "index", &index(position));
        check(&model, "item === arr[index]");
    }
    assert_eq!(model.foreach_in_progress(), Ok(0));
}
