//! Unit tests for the `XPath` data model: accepted inputs, rejected inputs,
//! boundaries, expected failures, and representative SCXML 1.0 CR 2014-03-13 Appendix C.3
//! semantics.

use scxml_types::{
    AssignPlacement, AssignSource, DataInitializer, DataValue, Event, EventName, EventType,
    FiniteNumber, SendId,
};

use crate::{
    DataModelError, ExecutionHost, MAX_VALUE_BYTES, MAX_VARIABLES, SyntaxKind, SystemContext,
    VariableMarkup, XPathDataModel, accepts_event_data, check_syntax, event_data, markup_value,
    namelist_name,
};

/// Host whose configuration is a fixed list; `fail` makes every query fail.
#[derive(Default)]
struct Host {
    active: Vec<&'static str>,
    fail: bool,
    queries: usize,
}

impl ExecutionHost for Host {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        self.queries += 1;
        if self.fail {
            return Err(DataModelError::Host);
        }
        Ok(self.active.contains(&state_id))
    }
}

fn system() -> SystemContext {
    SystemContext {
        session_id: "session-1".into(),
        machine_name: Some("machineName".into()),
        io_processors: DataValue::JsonObject(
            r##"{"http://www.w3.org/TR/scxml/#SCXMLEventProcessor":{"location":"#_scxml_session-1"},"scxml":{"location":"#_scxml_session-1"}}"##
                .into(),
        ),
    }
}

fn model(source: &str) -> XPathDataModel {
    let mut model = XPathDataModel::new();
    model
        .initialize(source, system())
        .expect("test source must initialize");
    model
}

fn cond(model: &mut XPathDataModel, expression: &str) -> Result<bool, DataModelError> {
    model.evaluate_condition(expression, None, &mut Host::default())
}

fn truthy(model: &mut XPathDataModel, expression: &str) -> bool {
    cond(model, expression).unwrap_or_else(|error| panic!("{expression} failed: {error}"))
}

fn assign(
    model: &mut XPathDataModel,
    location: &str,
    kind: &AssignPlacement,
    value: &DataValue,
) -> Result<(), DataModelError> {
    model.assign(location, kind, value, None, &mut Host::default())
}

fn value(model: &mut XPathDataModel, expression: &str) -> DataValue {
    model
        .evaluate_value(expression, None, &mut Host::default())
        .unwrap_or_else(|error| panic!("{expression} failed: {error}"))
}

fn number(value: f64) -> DataValue {
    DataValue::Number(FiniteNumber::new(value).expect("test numbers are finite"))
}

fn event(name: &str) -> Event {
    Event::external(EventName::new(name).expect("test event names are valid"))
}

const THREE_NODES: &str =
    r#"<data id="Var1">1</data><data id="Var3"><node>1</node><node>2</node><node>3</node></data>"#;

/// SCXML 5.10: "The SCXML processor MUST bind the variable _name at load
/// time to the value of the 'name' attribute of the <scxml> element." A
/// document without that attribute supplies no value: `$_name` exists and is
/// an empty `<_name/>`; the processor invents no name.
#[test]
fn name_is_empty_when_the_document_has_no_name() {
    let mut model = XPathDataModel::new();
    model
        .initialize(
            THREE_NODES,
            SystemContext {
                machine_name: None,
                ..system()
            },
        )
        .expect("a nameless document initializes");
    assert!(truthy(&mut model, "count($_name) = 1"));
    assert!(truthy(&mut model, "count($_name/node()) = 0"));
    assert!(truthy(&mut model, "string($_name) = ''"));
}

#[test]
fn declared_data_and_system_variables_are_node_sets() {
    let mut model = model(THREE_NODES);
    assert!(truthy(&mut model, "$Var1/text() = 1"));
    assert!(truthy(&mut model, "$Var1 + 1 = 2"));
    assert!(truthy(&mut model, "count($Var3/*) = 3"));
    assert!(truthy(&mut model, "$_sessionid = 'session-1'"));
    assert!(truthy(&mut model, "$_name/text() = 'machineName'"));
    assert!(truthy(
        &mut model,
        "$_ioprocessors/processor[@name='scxml']/location/text() = '#_scxml_session-1'"
    ));
    assert_eq!(
        cond(&mut model, "$Undeclared"),
        Err(DataModelError::Execution),
        "an undeclared variable is an execution error"
    );
    assert_eq!(
        cond(&mut model, "==!*"),
        Err(DataModelError::InvalidInput),
        "a malformed expression is invalid input"
    );
}

#[test]
fn values_map_node_sets_scalars_and_non_finite_numbers() {
    let mut model = model(THREE_NODES);
    assert_eq!(
        value(&mut model, "$Var3/*[2]"),
        DataValue::Xml("<node>2</node>".into())
    );
    assert_eq!(
        value(&mut model, "$Var1/text()"),
        DataValue::Xml("1".into())
    );
    assert_eq!(value(&mut model, "$Var1 + 1"), number(2.0));
    assert_eq!(value(&mut model, "'a<b'"), DataValue::String("a<b".into()));
    assert_eq!(value(&mut model, "1 = 1"), DataValue::Bool(true));
    assert_eq!(
        value(&mut model, "0 div 0"),
        DataValue::String("NaN".into())
    );
    assert_eq!(
        model.evaluate_string("$Var3/*", None, &mut Host::default()),
        Ok("1".into()),
        "string() of a node-set is the first node's string-value"
    );
}

#[test]
fn event_is_unbound_without_an_event_and_complete_with_one() {
    let mut model = model(THREE_NODES);
    assert_eq!(
        cond(&mut model, "$_event/text()"),
        Err(DataModelError::Execution),
        "_event is unbound before the first event (SCXML 5.10)"
    );
    let mut received = event("event1");
    received.send_id = Some(SendId::new("send-7").expect("valid send id"));
    received.origin = Some("#_scxml_session-1".into());
    received.data = event_data(&[
        ("param1".into(), number(2.0)),
        (
            "Var1".into(),
            DataValue::Xml(r#"<data id="Var1">1</data>"#.into()),
        ),
    ])
    .expect("payload builds");
    let mut host = Host::default();
    for (expression, expected) in [
        ("$_event/name = 'event1'", true),
        ("$_event/type = 'external'", true),
        ("$_event/sendid = 'send-7'", true),
        ("$_event/origin = '#_scxml_session-1'", true),
        ("not(string($_event/invokeid))", true),
        ("$_event/data/data[@id='param1']/text() = 2", true),
        ("$_event/data/data[@id='Var1']/data/text() = 1", true),
        (
            "$_event/name and $_event/type and $_event/sendid and $_event/origin and $_event/origintype and $_event/invokeid and $_event/data",
            true,
        ),
        // Relative paths resolve against <datamodel> (official case 354).
        ("_event/name = 'event1'", true),
    ] {
        assert_eq!(
            model.evaluate_condition(expression, Some(&received), &mut host),
            Ok(expected),
            "{expression}"
        );
    }
    let mut content = event("event2");
    content.data = DataValue::String("123".into());
    assert_eq!(
        model.evaluate_condition("_event/data = 123", Some(&content), &mut host),
        Ok(true)
    );
    assert_eq!(
        cond(&mut model, "$_event"),
        Err(DataModelError::Execution),
        "_event unbinds again when no event is current"
    );
    let mut form = event("form");
    form.data = DataValue::JsonObject(r#"{"flag":true,"nested":{"n":1.5},"param1":"2"}"#.into());
    assert_eq!(
        model.evaluate_condition(
            "$_event/data/data[@id='param1'] = 2 and $_event/data/data[@id='nested']/data[@id='n'] = 1.5 and $_event/data/data[@id='flag'] = 'true'",
            Some(&form),
            &mut host
        ),
        Ok(true),
        "JSON object payloads (other I/O processors) map onto <data id> elements"
    );
}

#[test]
fn event_data_follows_the_cr_interpretation_order() {
    const LARGE: usize = 4 * 65_536;
    // SCXML 1.0 CR 2014-03-13 §C.3.7.1: key-value pairs become `<data id>`
    // children; "Otherwise (i.e., if the content does not consist of
    // key-value pairs), if the Processor can interpret the content is a
    // valid XML document, the it MUST insert the XML structure as the
    // content of _event's <data> child. Otherwise, the Processor MUST treat
    // the content as a space-normalized string literal and insert it as the
    // content of _event's <data> child."
    let mut model = model(THREE_NODES);
    let mut host = Host::default();
    let mut check = |data: DataValue, expression: &str| {
        let mut received = event("e");
        received.data = data;
        assert_eq!(
            model.evaluate_condition(expression, Some(&received), &mut host),
            Ok(true),
            "{expression}"
        );
    };
    check(
        DataValue::String("\n this  is\t a \r\n string \n".into()),
        "$_event/data = 'this is a string' and count($_event/data/node()) = 1",
    );
    check(
        DataValue::String(" \n ".into()),
        "count($_event/data/node()) = 0",
    );
    check(
        DataValue::String("<!-- c --> <books><book title='t2'/></books>\n".into()),
        "$_event/data/books/book/@title = 't2'",
    );
    // Text is XML only when it is a valid XML document: one root element.
    for text in ["<a/><b/>", "x <a/>", "<a/> x"] {
        check(
            DataValue::String(text.into()),
            &format!("$_event/data = '{text}' and count($_event/data/*) = 0"),
        );
    }
    // An XML value is already a node-set: the sender's indication is kept.
    check(
        DataValue::Xml("<a>1</a><b>2</b>".into()),
        "count($_event/data/*) = 2",
    );
    check(
        DataValue::Xml("<open>  x".into()),
        "$_event/data = '<open> x' and count($_event/data/*) = 0",
    );
    check(
        DataValue::JsonArray("[1,\"a  b\"]".into()),
        "$_event/data = '[1,\"a b\"]'",
    );
    check(
        DataValue::JsonObject(r#"{"k":"  a  b ","list":[1,2],"n":null}"#.into()),
        "$_event/data/data[@id='k'] = '  a  b ' and $_event/data/data[@id='list'] = '[1,2]' and count($_event/data/data[@id='n']/node()) = 0",
    );
    // Content is inserted as text nodes, never re-parsed as markup, and has
    // no byte bound (SCXML sets none), so a large payload whose escaped
    // markup is larger still binds.
    check(
        DataValue::String("<".repeat(LARGE)),
        &format!("string-length($_event/data) = {LARGE}"),
    );
    check(
        DataValue::JsonObject(format!(r#"{{"k":"{}"}}"#, "&".repeat(LARGE - 8))),
        &format!("string-length($_event/data/data[@id='k']) = {}", LARGE - 8),
    );
}

#[test]
fn every_event_data_format_is_handled() {
    const LARGE: usize = 4 * 65_536;
    // SCXML 1.0 C.1: error.communication is for data "the receiving
    // Processor cannot handle". SCXML 1.0 CR 2014-03-13 §C.3.7.1 ends its
    // interpretation order in "Otherwise, the Processor MUST treat the
    // content as a space-normalized string literal", so an XPath session
    // handles every format another session can send, JSON arrays included.
    for data in [
        DataValue::Absent,
        DataValue::Undefined,
        DataValue::Null,
        DataValue::Bool(true),
        DataValue::String("  a  b ".into()),
        DataValue::String("<a/><b/>".into()),
        DataValue::Xml("<open>  x".into()),
        DataValue::JsonArray("[1,[2,{\"k\":3}]]".into()),
        DataValue::JsonObject(r#"{"k":[1,2],"o":{"p":null}}"#.into()),
        DataValue::String("<".repeat(LARGE)),
    ] {
        assert!(accepts_event_data(&data), "{data:?}");
    }
}

#[test]
fn assign_types_place_values_per_appendix_b2() {
    let mut model = model(THREE_NODES);
    assign(
        &mut model,
        "$Var1",
        &AssignPlacement::ReplaceChildren,
        &number(5.0),
    )
    .expect("replace");
    assert!(truthy(&mut model, "$Var1/text() = 5"));

    // Official case 525: appending the node-set to itself snapshots first.
    let children = value(&mut model, "$Var3/*");
    assign(&mut model, "$Var3", &AssignPlacement::LastChild, &children).expect("lastchild");
    assert!(truthy(&mut model, "count($Var3/*) = 6"));

    assign(
        &mut model,
        "$Var3",
        &AssignPlacement::FirstChild,
        &DataValue::Xml("<first/>".into()),
    )
    .expect("firstchild");
    assert!(truthy(&mut model, "name($Var3/*[1]) = 'first'"));
    assign(
        &mut model,
        "$Var3/first",
        &AssignPlacement::NextSibling,
        &DataValue::Xml("<second/>".into()),
    )
    .expect("nextsibling");
    assign(
        &mut model,
        "$Var3/first",
        &AssignPlacement::PreviousSibling,
        &DataValue::Xml("<zeroth/>".into()),
    )
    .expect("previoussibling");
    assert!(truthy(
        &mut model,
        "name($Var3/*[1]) = 'zeroth' and name($Var3/*[3]) = 'second'"
    ));
    assign(
        &mut model,
        "$Var3/second",
        &AssignPlacement::Replace,
        &DataValue::Xml("<replaced>r</replaced>".into()),
    )
    .expect("replace");
    assert!(truthy(
        &mut model,
        "$Var3/replaced = 'r' and not($Var3/second)"
    ));
    assign(
        &mut model,
        "$Var3/zeroth",
        &AssignPlacement::Delete,
        &DataValue::Undefined,
    )
    .expect("delete");
    assert!(truthy(&mut model, "not($Var3/zeroth)"));
    assign(
        &mut model,
        "$Var3/first",
        &AssignPlacement::AddAttribute("mark".into()),
        &DataValue::String("m".into()),
    )
    .expect("addattribute");
    assert!(truthy(&mut model, "$Var3/first/@mark = 'm'"));

    // Every selected node receives the value, all-or-nothing.
    assign(
        &mut model,
        "$Var3/node",
        &AssignPlacement::AddAttribute("seen".into()),
        &DataValue::Bool(true),
    )
    .expect("multi-target addattribute");
    assert!(truthy(&mut model, "count($Var3/node[@seen='true']) = 6"));
}

#[test]
fn illegal_locations_are_execution_errors_and_change_nothing() {
    let mut model = model(THREE_NODES);
    let before = model.snapshot().expect("snapshot");
    for location in [
        "$_sessionid",
        "$_name",
        "$_ioprocessors/processor",
        "$Var1/nothing",
        "..",
        ".",
        "'text'",
        "$Undeclared",
    ] {
        assert_eq!(
            assign(
                &mut model,
                location,
                &AssignPlacement::ReplaceChildren,
                &number(1.0)
            ),
            Err(DataModelError::Execution),
            "{location}"
        );
    }
    let mut received = event("e");
    received.data = DataValue::String("d".into());
    assert_eq!(
        model.assign(
            "$_event/data",
            &AssignPlacement::ReplaceChildren,
            &number(1.0),
            Some(&received),
            &mut Host::default()
        ),
        Err(DataModelError::Execution),
        "_event is read-only"
    );
    for kind in [AssignPlacement::Replace, AssignPlacement::Delete] {
        assert_eq!(
            assign(&mut model, "$Var1", &kind, &number(1.0)),
            Err(DataModelError::Execution),
            "a variable's own element cannot be replaced or deleted"
        );
    }
    assert_eq!(
        assign(
            &mut model,
            "$Var1",
            &AssignPlacement::AddAttribute("not a name".into()),
            &number(1.0)
        ),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        assign(
            &mut model,
            "$Var1",
            &AssignPlacement::ReplaceChildren,
            &DataValue::Xml("<unclosed>".into())
        ),
        Err(DataModelError::Execution)
    );
    assert_eq!(
        assign(
            &mut model,
            "$Var1",
            &AssignPlacement::ReplaceChildren,
            &DataValue::JsonArray("[]".into())
        ),
        Err(DataModelError::UnsupportedValue)
    );
    assert_eq!(
        model.snapshot().expect("snapshot"),
        before,
        "failed assignments commit nothing"
    );
}

#[test]
fn foreach_items_bind_their_nodes() {
    let mut model = model(
        r#"<data id="Var1">0</data><data id="Var2"/><data id="Var3"><node>1</node><node>2</node><node>3</node></data><data id="Empty"/>"#,
    );
    let items = model
        .evaluate_items("$Var3/*", None, &mut Host::default())
        .expect("items");
    assert_eq!(items.len(), 3);
    for (index, item) in items.iter().enumerate() {
        model
            .bind("Var2", item, None, &mut Host::default())
            .expect("item binds");
        model
            .bind(
                "Index",
                &number(f64::from(u8::try_from(index).expect("small"))),
                None,
                &mut Host::default(),
            )
            .expect("index binds and declares");
        // Official cases 153/155: the item variable is the node itself.
        assert!(truthy(&mut model, "$Var1/text() < $Var2/text()"));
        let current = value(&mut model, "$Var2/text()");
        assign(
            &mut model,
            "$Var1",
            &AssignPlacement::ReplaceChildren,
            &current,
        )
        .expect("assign");
    }
    assert!(truthy(&mut model, "$Var1/text() = 3 and $Index = 2"));
    assert_eq!(
        model.evaluate_items("$Empty/text()", None, &mut Host::default()),
        Ok(Vec::new()),
        "an empty node-set is a legal collection (CR 2014-03-13 §C.3.10)"
    );
    for array in ["1", "'x'"] {
        assert_eq!(
            model.evaluate_items(array, None, &mut Host::default()),
            Err(DataModelError::Execution),
            "{array}: a scalar is not a legal array (official case 152)"
        );
    }
    assert_eq!(
        model.bind("..", &number(1.0), None, &mut Host::default()),
        Err(DataModelError::Execution),
        "`..` is not a variable name and not a writable location"
    );
    assert_eq!(
        model.bind("_event", &number(1.0), None, &mut Host::default()),
        Err(DataModelError::Execution),
        "system names cannot be bound"
    );
    model
        .bind(
            "$Var1",
            &DataValue::String("id-1".into()),
            None,
            &mut Host::default(),
        )
        .expect("a location binds by replacechildren");
    assert!(truthy(&mut model, "$Var1 = 'id-1'"));
}

/// Loops nest as deep as the document nests them: with 40 loops in
/// progress the outermost still binds the collection's node itself. The
/// data model tracks loops without a bound of its own on native builds.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn deeply_nested_loops_keep_binding_the_collection_nodes() {
    let mut model = model(
        r#"<data id="Var2"/><data id="Var3"><node>1</node><node>2</node></data><data id="Inner"><i>a</i><i>b</i></data><data id="Item"/>"#,
    );
    let outer = model
        .evaluate_items("$Var3/*", None, &mut Host::default())
        .expect("items");
    model
        .bind("Var2", &outer[0], None, &mut Host::default())
        .expect("item binds");
    for _ in 0..40 {
        let inner = model
            .evaluate_items("$Inner/*", None, &mut Host::default())
            .expect("inner items");
        model
            .bind("Item", &inner[0], None, &mut Host::default())
            .expect("inner item binds");
    }
    model
        .bind("Var2", &outer[1], None, &mut Host::default())
        .expect("item binds");
    assert!(truthy(&mut model, "count($Var2 | $Var3/*[2]) = 1"));
}

/// SCXML 4.6: "The SCXML processor MUST act as if it has made a shallow
/// copy of the collection produced by the evaluation of 'array'" and "For
/// each item in turn, the processor MUST assign it to the item variable".
/// A shallow copy of a node-set holds the same nodes, so `$item` is the
/// collection's node itself, not a copy of it. CR 2014-03-13 C.3.1: "The
/// Processor MUST also bind an `XPath` variable of the same name to that
/// datamodel <data> element": binding a foreach item to a declared
/// variable never removes that `<data id>` element from `<datamodel>`.
#[test]
fn foreach_items_are_the_collection_nodes_and_keep_declared_data() {
    let mut model =
        model(r#"<data id="list"><item>a</item><item>b</item></data><data id="it">kept</data>"#);
    for name in ["it", "fresh"] {
        let items = model
            .evaluate_items("$list/item", None, &mut Host::default())
            .expect("items");
        for (index, item) in items.iter().enumerate() {
            model
                .apply_assign(
                    name,
                    &AssignSource::ForeachValue(item.clone()),
                    &AssignPlacement::ReplaceChildren,
                    None,
                    &mut Host::default(),
                )
                .expect("the item binds");
            model
                .bind(
                    "i",
                    &number(f64::from(u8::try_from(index).expect("small"))),
                    None,
                    &mut Host::default(),
                )
                .expect("the index binds");
            let it = format!("${name}");
            assert!(
                truthy(&mut model, &format!("count({it}/.. | $list) = 1")),
                "inside the list"
            );
            assert!(
                truthy(&mut model, &format!("count({it} | $list/item) = 2")),
                "the same node"
            );
            assign(
                &mut model,
                &it,
                &AssignPlacement::AddAttribute(name.into()),
                &DataValue::String("1".into()),
            )
            .expect("an assignment through the item reaches the collection");
        }
    }
    assert!(truthy(&mut model, "count($list/item[@it][@fresh]) = 2"));
    assert!(truthy(&mut model, "count(/datamodel/data[@id='it']) = 1"));
    assert!(truthy(&mut model, "/datamodel/data[@id='it'] = 'kept'"));
    assert!(truthy(
        &mut model,
        "count(/datamodel/data[@id='fresh']) = 1"
    ));
    assert!(
        truthy(&mut model, "count(/datamodel/item) = 0"),
        "no node moved"
    );
    assert!(truthy(
        &mut model,
        "$it/text() = 'b' and $fresh/text() = 'b'"
    ));
    assert!(truthy(&mut model, "$i = 1"));
    // Outside a loop a plain value is assigned to the declared element.
    model
        .bind("it", &number(7.0), None, &mut Host::default())
        .expect("a later binding");
    assert!(truthy(&mut model, "$it = 7 and count($list/item) = 2"));
    assert!(truthy(&mut model, "count(/datamodel/data[@id='it']) = 1"));
}

/// Reclaiming garbage between iterations keeps the loop's items and the
/// item variable on the collection's own nodes.
#[test]
fn foreach_items_survive_store_compaction() {
    let mut model = model(r#"<data id="list"/><data id="junk"/>"#);
    // Each assignment replaces 4000 nodes. The items are created after the
    // first garbage, so compaction (past half the store's slot bound,
    // `MAX_STORE_NODES`) moves them to new slots.
    let garbage = DataValue::Xml("<g/>".repeat(4_000));
    let churn = |model: &mut XPathDataModel| {
        for _ in 0..25 {
            assign(model, "$junk", &AssignPlacement::ReplaceChildren, &garbage).expect("garbage");
        }
    };
    churn(&mut model);
    assign(
        &mut model,
        "$list",
        &AssignPlacement::ReplaceChildren,
        &DataValue::Xml("<item>a</item><item>b</item>".into()),
    )
    .expect("the list");
    let items = model
        .evaluate_items("$list/item", None, &mut Host::default())
        .expect("items");
    let bind = |model: &mut XPathDataModel, item: &DataValue| {
        model
            .apply_assign(
                "it",
                &AssignSource::ForeachValue(item.clone()),
                &AssignPlacement::ReplaceChildren,
                None,
                &mut Host::default(),
            )
            .expect("the item binds");
    };
    bind(&mut model, &items[0]);
    churn(&mut model);
    assert!(
        truthy(&mut model, "count($it | $list/item[1]) = 1"),
        "item 1"
    );
    bind(&mut model, &items[1]);
    assert!(
        truthy(&mut model, "count($it | $list/item[2]) = 1"),
        "item 2"
    );
    assert!(truthy(&mut model, "count(/datamodel/data[@id='it']) = 1"));
}

#[test]
fn apply_assign_declares_only_foreach_bindings() {
    let mut model = model(r#"<data id="Var1">0</data>"#);
    let apply = |model: &mut XPathDataModel, location: &str, source: AssignSource, placement| {
        model.apply_assign(location, &source, &placement, None, &mut Host::default())
    };
    let before = model.snapshot().expect("snapshot");
    // SCXML 5.4: an `<assign>` location is a location expression; a plain
    // name is a relative path from `<datamodel>` that selects nothing.
    for source in [
        AssignSource::Value(DataValue::String("x".into())),
        AssignSource::Expression("'x'".into()),
    ] {
        assert_eq!(
            apply(&mut model, "Var2", source, AssignPlacement::ReplaceChildren),
            Err(DataModelError::Execution)
        );
    }
    assert_eq!(
        apply(
            &mut model,
            "Var2",
            AssignSource::ForeachValue(number(1.0)),
            AssignPlacement::FirstChild,
        ),
        Err(DataModelError::InvalidInput),
        "a foreach binding is always replacechildren"
    );
    assert_eq!(model.snapshot(), Ok(before), "no variable was declared");
    // SCXML 4.6: a `<foreach>` item or index declares its variable.
    apply(
        &mut model,
        "Var2",
        AssignSource::ForeachValue(number(1.0)),
        AssignPlacement::ReplaceChildren,
    )
    .expect("a foreach binding declares its variable");
    apply(
        &mut model,
        "$Var1",
        AssignSource::Expression("$Var2 + 1".into()),
        AssignPlacement::ReplaceChildren,
    )
    .expect("an expression source evaluates and assigns");
    assert!(truthy(&mut model, "$Var1 = 2 and $Var2 = 1"));
}

#[test]
fn declare_and_set_declared_follow_late_binding_and_invoke_rules() {
    let mut model = model("");
    model.declare("Late").expect("declare");
    model
        .declare("Late")
        .expect("redeclaring keeps the binding");
    assert!(truthy(&mut model, "not($Late/node())"));
    assert_eq!(model.set_declared("Late", &number(4.0)), Ok(true));
    assert!(truthy(&mut model, "$Late = 4"));
    assert_eq!(
        model.set_declared("Absent", &number(4.0)),
        Ok(false),
        "undeclared invoke parameters change nothing"
    );
    assert_eq!(model.declare("_name"), Err(DataModelError::InvalidInput));
    let mut full = XPathDataModel::new();
    let mut source = String::new();
    for index in 0..MAX_VARIABLES {
        source.push_str("<data id=\"v");
        source.push_str(&index.to_string());
        source.push_str("\"/>");
    }
    full.initialize(&source, system())
        .expect("the bound is inclusive");
    assert_eq!(full.declare("overflow"), Err(DataModelError::InvalidInput));
}

#[test]
fn in_predicate_queries_the_host() {
    let mut model = model("");
    let mut host = Host {
        active: vec!["s1"],
        ..Host::default()
    };
    assert_eq!(
        model.evaluate_condition("In('s1')", None, &mut host),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_condition("In('s2')", None, &mut host),
        Ok(false)
    );
    assert_eq!(host.queries, 2);
    let mut failing = Host {
        fail: true,
        ..Host::default()
    };
    assert_eq!(
        model.evaluate_condition("In('s1')", None, &mut failing),
        Err(DataModelError::Host)
    );
}

#[test]
fn initialize_rejects_malformed_sources_and_system_values() {
    let mut model = XPathDataModel::new();
    for source in [
        "<data/>",
        r#"<data id="_event"/>"#,
        r#"<data id="a"/><data id="a"/>"#,
        r#"<other id="a"/>"#,
        "text",
        r#"<data id="a">"#,
        r#"<data id="not a name"/>"#,
    ] {
        assert_eq!(
            model.initialize(source, system()),
            Err(DataModelError::InvalidInput),
            "{source}"
        );
    }
    let mut empty_session = system();
    empty_session.session_id.clear();
    assert_eq!(
        model.initialize("", empty_session),
        Err(DataModelError::InvalidInput)
    );
    let mut noncanonical = system();
    noncanonical.io_processors = DataValue::JsonObject(r#"{"b":{},"a":{}}"#.into());
    assert_eq!(
        model.initialize("", noncanonical),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(
        model.system_context(),
        Err(DataModelError::NotInitialized),
        "failed initialization commits nothing"
    );
    assert_eq!(
        model.evaluate_condition("true()", None, &mut Host::default()),
        Err(DataModelError::NotInitialized)
    );
}

#[test]
fn script_is_empty_only() {
    let mut model = model("");
    assert_eq!(model.execute("  \n"), Ok(()));
    assert_eq!(model.execute("x = 1"), Err(DataModelError::Execution));
}

#[test]
fn a_script_of_other_unicode_white_space_is_not_empty() {
    // XPath white space is XML `S` (#x20 | #x9 | #xD | #xA); U+1680 OGHAM
    // SPACE MARK is script content, which the XPath data model rejects.
    let mut model = model("");
    assert_eq!(model.execute(" \t\r\n"), Ok(()));
    assert_eq!(model.execute("\u{1680}"), Err(DataModelError::Execution));
    assert_eq!(check_syntax(SyntaxKind::Script, " \t\r\n"), Ok(Vec::new()));
    assert_eq!(
        check_syntax(SyntaxKind::Script, "\u{1680}").map(|issues| issues.len()),
        Ok(1)
    );
}

#[test]
fn repeated_event_rebinding_stays_bounded_and_correct() {
    let mut model = model(r#"<data id="Count">0</data>"#);
    let mut host = Host::default();
    for index in 0..20_000_u32 {
        let mut received = event("tick");
        received.data = DataValue::Xml(format!("<n>{index}</n>"));
        received.event_type = EventType::Internal;
        assert_eq!(
            model.evaluate_condition(
                &format!("$_event/data/n = {index}"),
                Some(&received),
                &mut host
            ),
            Ok(true)
        );
        if index % 1_000 == 0 {
            let next = value(&mut model, "$Count + 1");
            assign(
                &mut model,
                "$Count",
                &AssignPlacement::ReplaceChildren,
                &next,
            )
            .expect("count");
        }
    }
    assert!(truthy(&mut model, "$Count = 20"));
}

#[test]
fn payload_helpers_build_scxml_xpath_event_data() {
    assert_eq!(
        event_data(&[
            ("a\"b".into(), DataValue::String("x<y".into())),
            ("n".into(), number(2.0)),
            ("u".into(), DataValue::Undefined),
            ("x".into(), DataValue::Xml("<v>1</v>".into())),
        ]),
        Ok(DataValue::Xml(
            "<data id=\"a&quot;b\">x&lt;y</data><data id=\"n\">2</data><data id=\"u\"></data><data id=\"x\"><v>1</v></data>"
                .into()
        ))
    );
    assert_eq!(
        event_data(&[("j".into(), DataValue::JsonArray("[]".into()))]),
        Err(DataModelError::UnsupportedValue)
    );
    assert_eq!(
        event_data(&[("x".into(), DataValue::Xml("<open>".into()))]),
        Err(DataModelError::InvalidInput)
    );
    assert_eq!(namelist_name("$Var1"), Some("Var1"));
    assert_eq!(namelist_name("Var1"), None);
    assert_eq!(namelist_name("$a/b"), None);
    assert_eq!(
        markup_value("<book title=\"t\"/>"),
        DataValue::Xml("<book title=\"t\"/>".into())
    );
    assert_eq!(markup_value("123"), DataValue::String("123".into()));
    assert_eq!(
        markup_value("<!-- c -->"),
        DataValue::String("<!-- c -->".into())
    );
}

#[test]
fn check_syntax_reports_exact_spans() {
    assert_eq!(
        check_syntax(SyntaxKind::Condition, "$Var1/text() = 1"),
        Ok(Vec::new())
    );
    assert_eq!(
        check_syntax(SyntaxKind::Condition, "In('s1')"),
        Ok(Vec::new())
    );
    let issues = check_syntax(SyntaxKind::Value, "1 +").expect("bounded input checks");
    assert_eq!(issues.len(), 1);
    assert!(issues[0].start <= issues[0].end && issues[0].end <= 3);
    assert_eq!(
        check_syntax(SyntaxKind::Location, "$Var1/a"),
        Ok(Vec::new())
    );
    assert_eq!(
        check_syntax(SyntaxKind::Location, "1 + 1").map(|issues| issues.len()),
        Ok(1),
        "a location must select nodes"
    );
    assert_eq!(check_syntax(SyntaxKind::Script, "  "), Ok(Vec::new()));
    assert_eq!(
        check_syntax(SyntaxKind::Script, "x").map(|issues| issues.len()),
        Ok(1)
    );
    // One byte past the wasm expression bound (16 KiB) is checked natively,
    // where the bound does not apply; the host's validator tests check that
    // the shipped data model declines it (LIMIT003).
    let long = "1".repeat(16_385);
    assert_eq!(check_syntax(SyntaxKind::Value, &long), Ok(Vec::new()));
    // A parse over the nesting bound is declined (the validator's LIMIT003),
    // never reported as a syntax issue.
    let nested = format!("{}1{}", "(".repeat(200), ")".repeat(200));
    assert_eq!(
        check_syntax(SyntaxKind::Value, &nested),
        Err(DataModelError::InvalidInput)
    );
}

#[test]
fn initialize_declarations_binds_the_template_processor_alias_and_values_data() {
    let mut model = XPathDataModel::new();
    model
        .initialize_declarations(["Var1", "Var2"], system())
        .expect("unique declarations initialize");
    // W3C XPath IRP template name (official 500/501) mirrors the canonical
    // SCXML Event I/O Processor entry.
    assert!(truthy(
        &mut model,
        "$_ioprocessors/processor[@name='http://www.w3.org/TR/scxml/#SCXMLProcessor']/location = '#_scxml_session-1'"
    ));
    assert!(truthy(&mut model, "count($Var1/node()) = 0"));
    model
        .initialize_data(
            "Var1",
            &DataInitializer::Expression("1 + 1".into()),
            &mut Host::default(),
        )
        .expect("an expression initializer values a declared variable");
    model
        .initialize_data(
            "Var2",
            &DataInitializer::Value(DataValue::Xml("<item>a</item>".into())),
            &mut Host::default(),
        )
        .expect("a materialized value initializes");
    assert!(truthy(&mut model, "$Var1 = 2 and $Var2/item = 'a'"));
    assert_eq!(
        model.initialize_data(
            "Var3",
            &DataInitializer::Value(DataValue::Null),
            &mut Host::default()
        ),
        Err(DataModelError::InvalidInput),
        "an undeclared id is not a location"
    );
    assert_eq!(
        XPathDataModel::new().initialize_declarations(["a", "a"], system()),
        Err(DataModelError::InvalidInput),
        "duplicate declarations are rejected"
    );
}

#[test]
fn snapshot_serializes_every_variable_in_document_order_and_changes_nothing() {
    assert_eq!(
        XPathDataModel::new().snapshot(),
        Err(DataModelError::NotInitialized)
    );
    let mut model = model(THREE_NODES);
    model
        .bind(
            "Item",
            &DataValue::Xml("<node>9</node>".into()),
            None,
            &mut Host::default(),
        )
        .expect("bind");
    let image = model.snapshot().expect("snapshot");
    assert_eq!(model.snapshot(), Ok(image.clone()), "deterministic");
    assert_eq!(image.system, system());
    let variables: Vec<(&str, &str)> = image
        .variables
        .iter()
        .map(|variable| match &variable.markup {
            VariableMarkup::Markup(markup) => (variable.name.as_str(), markup.as_str()),
            VariableMarkup::Oversized(_) => panic!("small markup is carried"),
        })
        .collect();
    assert_eq!(
        variables,
        [
            ("Var1", r#"<data id="Var1">1</data>"#),
            (
                "Var3",
                r#"<data id="Var3"><node>1</node><node>2</node><node>3</node></data>"#
            ),
            ("Item", r#"<data id="Item"><node>9</node></data>"#),
        ]
    );
    assert!(truthy(
        &mut model,
        "$Item/node/text() = 9 and count($Var3/*) = 3 and $Var1 = 1"
    ));
}

#[test]
fn a_variable_name_is_bounded_like_an_scxml_name() {
    let at_bound = "n".repeat(scxml_types::MAX_NAME_BYTES);
    let over = "n".repeat(scxml_types::MAX_NAME_BYTES + 1);
    let mut model = XPathDataModel::new();
    model
        .initialize_declarations([at_bound.as_str()], system())
        .expect("a name of exactly MAX_NAME_BYTES declares");
    assert_eq!(
        XPathDataModel::new().initialize_declarations([over.as_str()], system()),
        Err(DataModelError::InvalidInput),
        "one byte over the bound is rejected"
    );
    let image = model.snapshot().expect("snapshot");
    assert_eq!(image.variables[0].name, at_bound);
}

#[test]
fn markup_longer_than_one_data_value_is_reported_oversized() {
    // `<data id="Var1">` + text + `</data>`: pad the text so the element is
    // exactly MAX_VALUE_BYTES, then grow it by one byte.
    let wrapper = r#"<data id="Var1"></data>"#.len();
    let mut model = model(r#"<data id="Var1"/>"#);
    let text = |bytes: usize| DataValue::String("x".repeat(bytes));
    assign(
        &mut model,
        "$Var1",
        &AssignPlacement::ReplaceChildren,
        &text(MAX_VALUE_BYTES - wrapper),
    )
    .expect("fill to the bound");
    let markup = |model: &XPathDataModel| {
        model.snapshot().expect("snapshot").variables[0]
            .markup
            .clone()
    };
    match markup(&model) {
        VariableMarkup::Markup(markup) => assert_eq!(markup.len(), MAX_VALUE_BYTES),
        VariableMarkup::Oversized(bytes) => panic!("at the bound is carried, got {bytes}"),
    }
    assign(
        &mut model,
        "$Var1",
        &AssignPlacement::ReplaceChildren,
        &text(MAX_VALUE_BYTES - wrapper + 1),
    )
    .expect("one byte more commits");
    assert_eq!(
        markup(&model),
        VariableMarkup::Oversized(u64::try_from(MAX_VALUE_BYTES + 1).expect("fits"))
    );
    // Growth past any one value's bound through repeated appends is
    // reported the same way, never truncated.
    let big = DataValue::Xml(format!("<n>{}</n>", "x".repeat(60_000)));
    assign(&mut model, "$Var1", &AssignPlacement::LastChild, &big).expect("append");
    assign(&mut model, "$Var1", &AssignPlacement::LastChild, &big).expect("append");
    assert!(matches!(
        markup(&model),
        VariableMarkup::Oversized(bytes) if bytes > 2 * 60_000
    ));
}

#[test]
fn data_string_content_is_a_space_normalized_literal() {
    // SCXML 1.0 CR 2014-03-13 §C.3.1: "Otherwise the Processor MUST treat
    // the content as a space-normalized string literal and insert it as the
    // child of the datamodel <data> element."
    let mut model = XPathDataModel::new();
    model
        .initialize_declarations(["Text", "Blank", "Markup"], system())
        .expect("declarations initialize");
    let mut host = Host::default();
    for (id, content) in [
        (
            "Text",
            DataValue::String("\n  this  is \t\r\n a string \n".into()),
        ),
        ("Blank", DataValue::String(" \n\t ".into())),
        ("Markup", DataValue::Xml("<p>  keep \n  me  </p>".into())),
    ] {
        model
            .initialize_data(id, &DataInitializer::Value(content), &mut host)
            .expect("inline or src content initializes");
    }
    assert!(truthy(&mut model, "$Text = 'this is a string'"));
    assert!(
        truthy(&mut model, "count($Blank/node()) = 0"),
        "whitespace-only content normalizes to the empty string: the <data> stays empty"
    );
    assert!(
        truthy(&mut model, "$Markup/p = '  keep \n  me  '"),
        "XML content is inserted as is, never normalized"
    );
    // Only content is normalized: an expression result keeps its exact text.
    model
        .initialize_data(
            "Text",
            &DataInitializer::Expression("' a  b '".into()),
            &mut host,
        )
        .expect("expression initializes");
    assert!(truthy(&mut model, "$Text = ' a  b '"));
}

/// A loop an error ended early never binds its last item, so it stays in
/// progress until the machine call that ran it returns and the machine
/// releases every loop in progress (WIT `data-model-api.release-foreach`):
/// natively, where no count bounds them, 1,000 such loops leave none
/// behind, and the next loop binds the collection's nodes.
#[test]
fn loops_an_error_ended_are_released_when_the_machine_call_returns() {
    let mut model =
        model(r#"<data id="Var1"/><data id="Var3"><node>1</node><node>2</node></data>"#);
    for _ in 0..1_000 {
        let items = model
            .evaluate_items("$Var3/*", None, &mut Host::default())
            .expect("items");
        model
            .bind("Var1", &items[0], None, &mut Host::default())
            .expect("the first item binds");
    }
    if !cfg!(target_arch = "wasm32") {
        assert_eq!(model.foreach_in_progress(), 1_000);
    }
    model.release_foreach();
    assert_eq!(model.foreach_in_progress(), 0);
    let items = model
        .evaluate_items("$Var3/*", None, &mut Host::default())
        .expect("items");
    for item in &items {
        model
            .bind("Var1", item, None, &mut Host::default())
            .expect("item binds");
    }
    assert!(truthy(&mut model, "$Var1 = 2"), "the last item is bound");
    assert_eq!(model.foreach_in_progress(), 0);
}
