//! Which names the ECMAScript data model accepts as variables.
//!
//! * B.2.10: "The SCXML Processor MUST accept any ECMAScript program as
//!   defined in Section 14 of [ECMASCRIPT-262] as the content of a
//!   <script> element." A program may create globals under any name the
//!   language allows, including names that begin with `_`, and they become
//!   part of the data model (5.8).
//! * B.2.11: "The legal values for the 'item' attribute are legal
//!   ECMAScript variable names."
//! * 5.10: "A conformant SCXML document MUST NOT contain ids beginning with
//!   '_' in the <data> element." Only `<data>` ids carry that restriction.
//! * WIT `data-model-snapshot.variables` lists "Every non-`_` location".

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext, VariableValue,
};
use scxml_types::{AssignPlacement, AssignSource, DataValue, FiniteNumber};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn system() -> SystemContext {
    SystemContext {
        session_id: "s".into(),
        machine_name: Some("m".into()),
        io_processors: DataValue::JsonObject("{}".into()),
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(["declared"], system())
        .expect("declarations initialize");
    model
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("finite"))
}

fn foreach(model: &mut EcmaScriptDataModel, item: &str) -> Result<(), DataModelError> {
    model.apply_assign(
        item,
        &AssignSource::ForeachValue(number(1.0)),
        &AssignPlacement::ReplaceChildren,
        None,
        &mut NoStates,
    )
}

#[test]
fn a_script_may_create_globals_under_any_name_the_language_allows() {
    let mut model = model();
    let host = &mut NoStates;
    model
        .execute_without_event(
            "_private = 1; var __scxml_like = 2; \u{f1} = 3; globalThis['a b'] = 4;",
            host,
        )
        .expect("the program is accepted");
    // Globals persist across operations.
    model
        .execute_without_event("declared = 5;", host)
        .expect("a second script runs");
    assert_eq!(
        model.evaluate_value_without_event(
            "[_private, __scxml_like, \u{f1}, globalThis['a b'], declared]",
            host
        ),
        Ok(DataValue::JsonArray("[1,2,3,4,5]".into()))
    );
    let names = model
        .snapshot()
        .expect("snapshot")
        .variables
        .into_iter()
        .map(|variable| (variable.name, variable.value))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![("declared".to_owned(), VariableValue::Value(number(5.0)))],
        "the snapshot lists the declared <data> ids"
    );
}

#[test]
fn foreach_items_are_exactly_the_legal_variable_names() {
    let mut model = model();
    for legal in ["_item", "$", "\u{f1}", "let", "item2"] {
        assert_eq!(foreach(&mut model, legal), Ok(()), "{legal}");
    }
    assert_eq!(
        model.evaluate_value_without_event("[_item, $, \u{f1}, item2]", &mut NoStates),
        Ok(DataValue::JsonArray("[1,1,1,1]".into()))
    );
    model
        .execute_without_event("declared = {}", &mut NoStates)
        .expect("declared holds an object");
    for illegal in [
        "declared.x",
        "declared['x']",
        "(declared)",
        "a b",
        "1a",
        "class",
        "",
        "_event",
        "_sessionid",
    ] {
        assert!(foreach(&mut model, illegal).is_err(), "{illegal:?}");
    }
    assert_eq!(
        model.evaluate_value_without_event("declared", &mut NoStates),
        Ok(DataValue::JsonObject("{}".into())),
        "an illegal item name assigns nothing"
    );
}

#[test]
fn every_data_id_a_document_may_declare_becomes_a_global_variable() {
    // B.2.1: "For each <data> element in the document, the SCXML Processor
    // MUST create an ECMAScript variable object whose name is the value of
    // the 'id' attribute of <data>." The id is an XML ID; 5.10 reserves only
    // a leading `_`. A name that is not an ECMAScript identifier is still a
    // property of the global (variable) object, reachable as
    // `globalThis['retry-count']`.
    let ids = [
        "\u{4ef7}\u{683c}",
        "\u{e9}",
        "retry-count",
        "max.tries",
        "class",
    ];
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(ids, system())
        .expect("every conformant data id is declared");
    let host = &mut NoStates;
    for (index, id) in ids.iter().enumerate() {
        let initializer = if index % 2 == 0 {
            scxml_types::DataInitializer::Expression(format!("{index} + 1"))
        } else {
            scxml_types::DataInitializer::Value(number(f64::from(
                u32::try_from(index).expect("small index") + 1,
            )))
        };
        model
            .initialize_data(id, &initializer, host)
            .unwrap_or_else(|error| panic!("{id}: {error:?}"));
    }
    assert_eq!(
        model.evaluate_value_without_event(
            "[\u{4ef7}\u{683c}, \u{e9}, globalThis['retry-count'], globalThis['max.tries'], globalThis['class']]",
            host
        ),
        Ok(DataValue::JsonArray("[1,2,3,4,5]".into()))
    );
    let mut late = model_with(&["declared"]);
    late.declare("late-bound")
        .expect("a late-bound id is declared");
    assert_eq!(
        late.evaluate_value_without_event("typeof globalThis['late-bound']", &mut NoStates),
        Ok(DataValue::String("undefined".into()))
    );
    assert_eq!(
        model
            .snapshot()
            .expect("snapshot")
            .variables
            .into_iter()
            .map(|variable| variable.name)
            .collect::<Vec<_>>(),
        ids.map(str::to_owned)
    );
}

fn model_with(ids: &[&str]) -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(ids.iter().copied(), system())
        .expect("declarations initialize");
    model
}

#[test]
fn data_ids_beginning_with_an_underscore_are_rejected() {
    for ids in [&["_x"][..], &["ok", "_private"][..]] {
        assert_eq!(
            EcmaScriptDataModel::new().initialize_declarations(ids.iter().copied(), system()),
            Err(DataModelError::InvalidInput),
            "{ids:?}"
        );
    }
    let mut model = model();
    assert_eq!(model.declare("_late"), Err(DataModelError::InvalidInput));
}
