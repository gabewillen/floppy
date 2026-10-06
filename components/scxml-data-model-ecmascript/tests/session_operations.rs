//! The session-level `data-model-api` operations (late binding, invoke
//! parameters, locations, string-typed attributes, `BasicHTTP` form values,
//! `<foreach>` collections, and the pure payload helpers).
//!
//! Every expectation is the exact result these operations must produce
//! for the component to reproduce the checked-in IRP baselines (they were
//! captured from the session adapter the operations replaced). The
//! evaluating operations are also compared against the equivalent
//! `evaluate_value` composition.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, ForeachItems, InlineContent, SystemContext,
    event_payload, external_data, inline_content, namelist_key,
};
use scxml_types::{DataInitializer, DataValue, Event, EventName, FiniteNumber, FormValueSource};

struct Active(&'static [&'static str]);

impl ExecutionHost for Active {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(self.0.contains(&state_id))
    }
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("test numbers are finite"))
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["Var1", "Var2", "arr", "doc", "obj"],
            SystemContext {
                session_id: "s1".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    let mut host = Active(&[]);
    for (id, expression) in [
        ("Var1", "1"),
        ("Var2", "'two'"),
        ("arr", "[1, 'b', [2], {k: 3}, null]"),
        ("obj", "({a: 1, b: 'x'})"),
    ] {
        model
            .initialize_data(
                id,
                &DataInitializer::Expression(expression.into()),
                &mut host,
            )
            .expect("test data initializes");
    }
    model
        .initialize_data(
            "doc",
            &DataInitializer::Value(DataValue::Xml("<root><a>1</a></root>".into())),
            &mut host,
        )
        .expect("an XML value initializes");
    model
}

fn event() -> Event {
    let mut event = Event::external(EventName::new("ev.one").expect("valid event name"));
    event.data = DataValue::JsonObject(r#"{"p":"payload"}"#.into());
    event
}

#[test]
fn namelist_key_accepts_exactly_plain_ascii_identifiers() {
    for legal in ["Var1", "_x", "$", "a$1_"] {
        assert_eq!(namelist_key(legal), Some(legal), "{legal}");
    }
    for illegal in ["", "1a", "a.b", "a b", "é", "a-b", "foo.bar.baz ", "[0]"] {
        assert_eq!(namelist_key(illegal), None, "{illegal:?}");
    }
}

#[test]
fn event_payload_builds_a_canonical_object_and_rejects_unportable_values() {
    let entries = [
        ("b".to_owned(), DataValue::String("x".into())),
        ("a".to_owned(), number(1.0)),
        ("n".to_owned(), DataValue::Null),
        (
            "o".to_owned(),
            DataValue::JsonObject(r#"{"k":[1,2]}"#.into()),
        ),
        ("l".to_owned(), DataValue::JsonArray("[true]".into())),
        ("a".to_owned(), DataValue::Bool(true)),
    ];
    assert_eq!(
        event_payload(&entries),
        Some(DataValue::JsonObject(
            r#"{"a":true,"b":"x","l":[true],"n":null,"o":{"k":[1,2]}}"#.into()
        ))
    );
    assert_eq!(event_payload(&[]), Some(DataValue::JsonObject("{}".into())));
    for unportable in [
        DataValue::Undefined,
        DataValue::Absent,
        DataValue::Xml("<a/>".into()),
    ] {
        assert_eq!(event_payload(&[("x".to_owned(), unportable)]), None);
    }
}

#[test]
fn inline_content_reads_markup_as_xml_and_normalized_text_as_json_or_string() {
    assert_eq!(
        inline_content(InlineContent::Markup("<a xmlns=\"\">1</a>")),
        Some(DataValue::Xml("<a xmlns=\"\">1</a>".into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("\n  { \"b\": [1, 2],\n \"a\": 1 }  ")),
        Some(DataValue::JsonObject(r#"{"a":1,"b":[1,2]}"#.into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text(" [1,  2] ")),
        Some(DataValue::JsonArray("[1,2]".into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("  123 ")),
        Some(number(123.0))
    );
    assert_eq!(
        inline_content(InlineContent::Text(" hello \n\t world ")),
        Some(DataValue::String("hello world".into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("")),
        Some(DataValue::String(String::new()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("'single'")),
        Some(DataValue::String("'single'".into()))
    );
}

#[test]
fn external_data_is_decided_only_for_xml_documents() {
    assert_eq!(
        external_data("<?xml version=\"1.0\"?>\n<root><a/></root>\n"),
        Some(DataValue::Xml(
            "<?xml version=\"1.0\"?>\n<root><a/></root>\n".into()
        ))
    );
    for other in ["{\"a\":1}", "plain text", "<a/><b/>", ""] {
        assert_eq!(external_data(other), None, "{other:?}");
    }
}

#[test]
fn evaluate_text_is_some_only_for_strings_with_and_without_an_event() {
    let model = model();
    let mut host = Active(&[]);
    let event = event();
    for event in [None, Some(&event)] {
        assert_eq!(
            model.evaluate_text("'a' + Var2", event, &mut host),
            Ok(Some("atwo".into()))
        );
        for non_string in ["Var1", "null", "undefined", "obj", "arr", "true"] {
            assert_eq!(
                model.evaluate_text(non_string, event, &mut host),
                Ok(None),
                "{non_string}"
            );
        }
        assert_eq!(
            model.evaluate_text("nope", event, &mut host),
            Err(DataModelError::Execution)
        );
    }
    assert_eq!(
        model.evaluate_text("_event.name", Some(&event), &mut host),
        Ok(Some("ev.one".into()))
    );
}

#[test]
fn locations_and_form_values_equal_the_native_value_composition() {
    let model = model();
    let mut host = Active(&["s"]);
    let event = event();
    for source in ["Var1", "Var2", "obj", "arr", "doc", "obj.b", "In('s')"] {
        let native = model.evaluate_value_without_event(source, &mut host);
        assert_eq!(model.evaluate_location(source, None, &mut host), native);
        assert_eq!(
            model.evaluate_form_value(source, FormValueSource::Location, None, &mut host),
            native
        );
        assert_eq!(
            model.evaluate_form_value(source, FormValueSource::Expression, None, &mut host),
            native
        );
        let native = model.evaluate_value(source, &event, &mut host);
        assert_eq!(
            model.evaluate_location(source, Some(&event), &mut host),
            native
        );
    }
    assert_eq!(
        model.evaluate_location("Var1", None, &mut host),
        Ok(number(1.0))
    );
    assert_eq!(
        model.evaluate_location("_event.data.p", Some(&event), &mut host),
        Ok(DataValue::String("payload".into()))
    );
    // A missing location is a ReferenceError (error.execution); an illegal
    // expression is a syntax error (error.execution).
    assert_eq!(
        model.evaluate_location("undeclared", None, &mut host),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        model.evaluate_location("foo.bar.baz ", None, &mut host),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        model.evaluate_form_value("return", FormValueSource::Expression, None, &mut host),
        Err(DataModelError::Execution)
    );
}

#[test]
fn foreach_items_snapshot_arrays_and_classify_other_values() {
    let model = model();
    let mut host = Active(&[]);
    assert_eq!(
        model.foreach_items("arr", None, &mut host),
        Ok(ForeachItems::Items(vec![
            number(1.0),
            DataValue::String("b".into()),
            DataValue::JsonArray("[2]".into()),
            DataValue::JsonObject(r#"{"k":3}"#.into()),
            DataValue::Null,
        ]))
    );
    assert_eq!(
        model.foreach_items("[]", Some(&event()), &mut host),
        Ok(ForeachItems::Items(Vec::new()))
    );
    for illegal in ["Var1", "Var2", "obj", "null", "undefined", "doc"] {
        assert_eq!(
            model.foreach_items(illegal, None, &mut host),
            Ok(ForeachItems::Illegal),
            "{illegal}"
        );
    }
    assert_eq!(
        model.foreach_items("undeclared", None, &mut host),
        Err(DataModelError::Execution)
    );
}

/// A location's leading identifier is an ECMAScript `IdentifierName`
/// (ECMA-262 §12.7: an `IdentifierStart` then `IdentifierPart`s, Unicode
/// `ID_Start` and `ID_Continue`): combining marks and connector punctuation
/// continue it, so a child declaring such a name receives its parameter.
#[test]
fn set_parameter_reads_unicode_identifier_roots() {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["cafe\u{301}", "a\u{203f}b"],
            SystemContext {
                session_id: "s1".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    let mut host = Active(&[]);
    for name in ["cafe\u{301}", "a\u{203f}b"] {
        assert_eq!(model.set_parameter(name, &number(3.0)), Ok(true), "{name}");
        assert_eq!(
            model.evaluate_value_without_event(name, &mut host),
            Ok(number(3.0)),
            "{name}"
        );
    }
    // `²` is alphanumeric but not ID_Continue: no identifier ends there.
    assert_eq!(model.set_parameter("cafe\u{b2}", &number(1.0)), Ok(false));
}

#[test]
fn set_parameter_sets_declared_locations_and_skips_undeclared_ones() {
    let mut model = model();
    let mut host = Active(&[]);
    assert_eq!(model.set_parameter("Var1", &number(7.0)), Ok(true));
    assert_eq!(
        model.evaluate_value_without_event("Var1", &mut host),
        Ok(number(7.0))
    );
    assert_eq!(
        model.set_parameter("obj.b", &DataValue::String("y".into())),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_value_without_event("obj.b", &mut host),
        Ok(DataValue::String("y".into()))
    );
    for undeclared in ["Var9", "nope.x", "1bad", ""] {
        assert_eq!(
            model.set_parameter(undeclared, &number(1.0)),
            Ok(false),
            "{undeclared:?}"
        );
    }
    assert_eq!(
        model.evaluate_value_without_event("typeof Var9 + typeof nope", &mut host),
        Ok(DataValue::String("undefinedundefined".into())),
        "a skipped parameter changes nothing"
    );
    assert_eq!(
        EcmaScriptDataModel::new().set_parameter("Var1", &number(1.0)),
        Err(DataModelError::NotInitialized)
    );
}

#[test]
fn declare_rebinds_to_undefined_and_creates_new_variables() {
    let mut model = model();
    let mut host = Active(&[]);
    model
        .declare("Var1")
        .expect("a declared variable redeclares");
    assert_eq!(
        model.evaluate_value_without_event("Var1", &mut host),
        Ok(DataValue::Undefined)
    );
    model.declare("late").expect("a new variable declares");
    assert_eq!(
        model.evaluate_value_without_event("typeof late", &mut host),
        Ok(DataValue::String("undefined".into()))
    );
    assert_eq!(model.set_parameter("late", &number(2.0)), Ok(true));
    // 5.10 reserves only a leading `_`; any other id is declared (B.2.1).
    for illegal in ["_event", "_late", ""] {
        assert_eq!(
            model.declare(illegal),
            Err(DataModelError::InvalidInput),
            "{illegal:?}"
        );
    }
    assert_eq!(
        EcmaScriptDataModel::new().declare("x"),
        Err(DataModelError::NotInitialized)
    );
}
