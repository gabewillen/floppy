//! The session-level `data-model-api` operations (invoke parameters,
//! locations, string-typed attributes, `BasicHTTP` form values, `<foreach>`
//! node-sets, and the pure payload helpers) of the `XPath` data model.
//!
//! Every expectation is the exact result these operations must produce
//! for the component to reproduce the checked-in IRP baselines (they were
//! captured from the session adapter the operations replaced). The
//! evaluating operations are also compared against the equivalent
//! `evaluate_string` / `evaluate_location` / `evaluate_items`
//! composition.

use scxml_data_model_xpath::{
    DataModelError, ExecutionHost, ForeachItems, InlineContent, SystemContext, XPathDataModel,
    event_payload, external_data, inline_content, namelist_key,
};
use scxml_types::{DataValue, Event, EventName, FiniteNumber, FormValueSource};

struct Active(&'static [&'static str]);

impl ExecutionHost for Active {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        Ok(self.0.contains(&state_id))
    }
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("test numbers are finite"))
}

fn model() -> XPathDataModel {
    let mut model = XPathDataModel::new();
    model
        .initialize(
            r#"<data id="Var1">1</data><data id="Var2">two</data><data id="nodes"><a>1</a><a x="y">2</a></data><data id="empty"/>"#,
            SystemContext {
                session_id: "s1".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("test source initializes");
    model
}

fn event() -> Event {
    let mut event = Event::external(EventName::new("ev.one").expect("valid event name"));
    event.data = DataValue::Xml(r#"<data id="p">payload</data>"#.into());
    event
}

#[test]
fn namelist_key_is_the_variable_name_of_a_plain_reference() {
    assert_eq!(namelist_key("$Var1"), Some("Var1"));
    assert_eq!(namelist_key("$a.b-c"), Some("a.b-c"));
    for illegal in [
        "Var1",
        "$",
        "$1a",
        "$a/b",
        "$a:b",
        "",
        "$Var1 ",
        "foo.bar.baz ",
    ] {
        assert_eq!(namelist_key(illegal), None, "{illegal:?}");
    }
}

#[test]
fn event_payload_builds_data_elements_in_order_with_duplicates() {
    let entries = [
        ("b".to_owned(), DataValue::String("x<y".into())),
        ("a".to_owned(), number(1.5)),
        ("n".to_owned(), DataValue::Null),
        ("x".to_owned(), DataValue::Xml("<v>1</v>".into())),
        ("b".to_owned(), DataValue::Bool(true)),
    ];
    assert_eq!(
        event_payload(&entries),
        Some(DataValue::Xml(
            r#"<data id="b">x&lt;y</data><data id="a">1.5</data><data id="n"></data><data id="x"><v>1</v></data><data id="b">true</data>"#
                .into()
        ))
    );
    assert_eq!(event_payload(&[]), Some(DataValue::Xml(String::new())));
    assert_eq!(
        event_payload(&[("l".to_owned(), DataValue::JsonArray("[1]".into()))]),
        None
    );
    assert_eq!(
        event_payload(&[("x".to_owned(), DataValue::Xml("<unclosed>".into()))]),
        None
    );
}

#[test]
fn inline_content_keeps_markup_as_xml_and_text_verbatim() {
    assert_eq!(
        inline_content(InlineContent::Markup("<a xmlns=\"\">1</a>")),
        Some(DataValue::Xml("<a xmlns=\"\">1</a>".into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("  {\"a\": 1} \n")),
        Some(DataValue::String("  {\"a\": 1} \n".into()))
    );
    assert_eq!(
        inline_content(InlineContent::Text("")),
        Some(DataValue::String(String::new()))
    );
}

#[test]
fn external_data_is_markup_when_it_holds_an_element_else_text() {
    assert_eq!(
        external_data("<a>1</a><b/>"),
        Some(DataValue::Xml("<a>1</a><b/>".into()))
    );
    for text in ["plain", "{\"a\":1}", "", "<unclosed>"] {
        assert_eq!(
            external_data(text),
            Some(DataValue::String(text.into())),
            "{text:?}"
        );
    }
}

#[test]
fn evaluate_text_is_the_string_value_with_and_without_an_event() {
    let mut model = model();
    let mut host = Active(&[]);
    let event = event();
    for event in [None, Some(&event)] {
        for (expression, text) in [
            ("$Var2", "two"),
            ("concat('a', $Var1)", "a1"),
            ("$nodes", "12"),
            ("1 div 0", "Infinity"),
            ("true()", "true"),
            ("$empty", ""),
        ] {
            let native = model
                .evaluate_string(expression, event, &mut host)
                .map(Some);
            assert_eq!(native, Ok(Some(text.into())), "{expression}");
            assert_eq!(
                model.evaluate_text(expression, event, &mut host),
                native,
                "{expression}"
            );
        }
        assert_eq!(
            model.evaluate_text("$undeclared", event, &mut host),
            Err(DataModelError::Execution)
        );
        assert_eq!(
            model.evaluate_text("(", event, &mut host),
            Err(DataModelError::InvalidInput)
        );
    }
    assert_eq!(
        model.evaluate_text("$_event/name", Some(&event), &mut host),
        Ok(Some("ev.one".into()))
    );
}

#[test]
fn form_values_are_string_values_and_locations_must_select_nodes() {
    let mut model = model();
    let mut host = Active(&["s"]);
    let event = event();
    assert_eq!(
        model.evaluate_form_value("$Var1", FormValueSource::Location, None, &mut host),
        Ok(DataValue::String("1".into()))
    );
    assert_eq!(
        model.evaluate_form_value("$nodes/a[2]/@x", FormValueSource::Location, None, &mut host),
        Ok(DataValue::String("y".into()))
    );
    assert_eq!(
        model.evaluate_form_value(
            "$_event/data/data[@id='p']",
            FormValueSource::Location,
            Some(&event),
            &mut host
        ),
        Ok(DataValue::String("payload".into()))
    );
    assert_eq!(
        model.evaluate_form_value("1 + 1", FormValueSource::Expression, None, &mut host),
        Ok(DataValue::String("2".into()))
    );
    assert_eq!(
        model.evaluate_form_value("In('s')", FormValueSource::Expression, None, &mut host),
        Ok(DataValue::String("true".into()))
    );
    // A location must select nodes; a value expression need not.
    assert_eq!(
        model.evaluate_form_value("1 + 1", FormValueSource::Location, None, &mut host),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        model.evaluate_form_value("$nodes/missing", FormValueSource::Location, None, &mut host),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        model.evaluate_form_value(
            "$nodes/missing",
            FormValueSource::Expression,
            None,
            &mut host
        ),
        Ok(DataValue::String(String::new()))
    );
}

#[test]
fn evaluate_location_requires_a_non_empty_node_set() {
    let mut model = model();
    let mut host = Active(&[]);
    assert_eq!(
        model.evaluate_location("$nodes/a", None, &mut host),
        Ok(DataValue::Xml("<a>1</a><a x=\"y\">2</a>".into()))
    );
    for missing in ["foo.bar.baz ", "$empty/*", "'text'", "$undeclared"] {
        assert_eq!(
            model.evaluate_location(missing, None, &mut host),
            Err(DataModelError::Execution),
            "{missing:?}"
        );
    }
}

#[test]
fn foreach_items_iterate_node_sets_and_reject_everything_else() {
    let mut model = model();
    let mut host = Active(&[]);
    assert_eq!(
        model.foreach_items("$nodes/a", Some(&event()), &mut host),
        Ok(ForeachItems::Items(vec![
            DataValue::Xml("<a>1</a>".into()),
            DataValue::Xml("<a x=\"y\">2</a>".into()),
        ]))
    );
    // SCXML 1.0 CR 2014-03-13 §C.3.10: "the legal iterable collections are
    // Node-Sets"; an empty node-set is one, iterated zero times.
    assert_eq!(
        model.foreach_items("$empty/*", None, &mut host),
        Ok(ForeachItems::Items(Vec::new()))
    );
    for illegal in ["1", "'a'", "true()", "$undeclared"] {
        assert_eq!(
            model.foreach_items(illegal, None, &mut host),
            Ok(ForeachItems::Illegal),
            "{illegal}"
        );
    }
    assert_eq!(
        model.foreach_items("(", None, &mut host),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        XPathDataModel::new().foreach_items("1", None, &mut host),
        Err(DataModelError::NotInitialized)
    );
}

#[test]
fn set_parameter_replaces_declared_values_and_skips_undeclared_names() {
    let mut model = model();
    let mut host = Active(&[]);
    assert_eq!(model.set_parameter("Var1", &number(7.0)), Ok(true));
    assert_eq!(
        model.evaluate_string("$Var1", None, &mut host),
        Ok("7".into())
    );
    assert_eq!(
        model.set_parameter("Var2", &DataValue::Xml("<b>3</b>".into())),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_string("$Var2/b", None, &mut host),
        Ok("3".into())
    );
    assert_eq!(model.set_parameter("Var9", &number(1.0)), Ok(false));
    assert_eq!(
        model.set_parameter("Var1", &DataValue::JsonArray("[1]".into())),
        Err(DataModelError::UnsupportedValue)
    );
}
