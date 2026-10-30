//! The SCXML 1.0 system variables in the ECMAScript data model (sections
//! 5.10, 5.10.1, and B.2.8).

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{BaggagePolicy, DataValue, Event, EventName, TraceContext};

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
            ["kept"],
            SystemContext {
                session_id: "s1".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject(
                    r##"{"scxml":{"location":"#_scxml_s1"}}"##.into(),
                ),
            },
        )
        .expect("declarations initialize");
    model
}

fn event(name: &str) -> Event {
    Event::external(EventName::new(name.to_owned()).expect("valid event name"))
}

fn payload_event() -> Event {
    let mut tick = event("tick");
    tick.data = DataValue::JsonObject(r#"{"p":1,"q":[1]}"#.into());
    tick
}

/// 5.10: "The Processor MUST cause any attempt to change the value of a
/// system variable to fail and MUST place the error 'error.execution' on
/// the internal event queue when such an attempt is made." B.2.8: "The
/// SCXML Processor MUST define an ECMAScript read-only variable for each
/// system variable defined in 5.10 System Variables." A script that writes
/// a system variable fails (`error.execution`); the variable keeps its
/// value, and what the script did before the write stays done, as
/// ECMAScript defines for an abrupt completion.
#[test]
fn a_script_that_writes_a_system_variable_fails_and_leaves_it_unchanged() {
    let mut model = model();
    let host = &mut NoStates;
    let tick = payload_event();
    for script in [
        "kept = 1; _sessionid = 'x'",
        "kept = 1; _name = 'x'",
        "kept = 1; _ioprocessors = {}",
        "kept = 1; _event = 1",
        "kept = 1; var _event = 1",
        "kept = 1; function _name() {}",
        "kept = 1; this._sessionid = 'x'",
        "kept = 1; Object.defineProperty(globalThis, '_event', { value: 1 })",
        "kept = 1; (function () { 'use strict'; _ioprocessors = null; })()",
    ] {
        assert_eq!(
            model.execute(script, &tick, host),
            Err(DataModelError::Execution),
            "{script}"
        );
        assert_eq!(
            model.execute_without_event(script, host),
            Err(DataModelError::Execution),
            "{script} without an event"
        );
    }
    assert_eq!(
        model.evaluate_value(
            "[kept, _sessionid, _name, _ioprocessors.scxml.location, _event.name, _event.data.p]",
            &tick,
            host
        ),
        Ok(DataValue::JsonArray(
            r##"[1,"s1","m","#_scxml_s1","tick",1]"##.into()
        )),
        "every system variable keeps its value"
    );
}

/// 5.10: "The SCXML Processor MUST maintain a protected portion of the
/// data model containing information that can be useful to applications."
/// The objects bound to `_event` and `_ioprocessors` never change: a write
/// to one of their members changes nothing.
#[test]
fn the_objects_of_system_variables_never_change() {
    let mut model = model();
    let host = &mut NoStates;
    let tick = payload_event();
    model
        .execute(
            "_event.name = 'x'; _event.data.p = 9; _event.data.q.push(2); \
             _ioprocessors.scxml.location = 'x'; _ioprocessors.added = 1; \
             kept = [_event.name, _event.data.p, _event.data.q.length, \
                     _ioprocessors.scxml.location, _ioprocessors.added];",
            &tick,
            host,
        )
        .ok();
    assert_eq!(
        model.evaluate_value(
            "[_event.name, _event.data.p, _event.data.q.length, _ioprocessors.scxml.location, typeof _ioprocessors.added]",
            &tick,
            host
        ),
        Ok(DataValue::JsonArray(r##"["tick",1,1,"#_scxml_s1","undefined"]"##.into()))
    );
    for guard in [
        "Object.isFrozen(_event) && Object.isFrozen(_event.data) && Object.isFrozen(_event.data.q)",
        "Object.isFrozen(_ioprocessors) && Object.isFrozen(_ioprocessors.scxml)",
    ] {
        assert_eq!(
            model.evaluate_condition(guard, &tick, host),
            Ok(true),
            "{guard}"
        );
    }
}

/// B.2.8: "In cases where this specification does not specify a value for
/// one of these fields or states that the field is empty or has no value,
/// the Processor MUST set the value to ECMAScript undefined."
#[test]
fn an_event_without_a_payload_has_undefined_data() {
    let mut model = model();
    let host = &mut NoStates;
    let plain = event("plain");
    assert_eq!(
        plain.data,
        DataValue::Absent,
        "the event carries no payload"
    );
    for guard in [
        "_event.data === undefined",
        "typeof _event.data === 'undefined'",
        "'data' in _event",
        "_event.sendid === undefined && _event.origin === undefined",
    ] {
        assert_eq!(
            model.evaluate_condition(guard, &plain, host),
            Ok(true),
            "{guard}"
        );
    }
    model
        .assign_expression("kept", "_event.data", &plain, host)
        .expect("the empty data field is assignable");
    assert_eq!(
        model.evaluate_value_without_event("kept", host),
        Ok(DataValue::Undefined)
    );
    assert_eq!(
        model.evaluate_log("_event.data", &plain, host),
        Ok("undefined".into())
    );
}

/// 5.10: "The Processor MUST cause any attempt to change the value of a
/// system variable to fail and MUST place the error 'error.execution' on
/// the internal event queue when such an attempt is made." A member write
/// to the object of `_event` or `_ioprocessors`, at any depth and in
/// non-strict code as in strict code, is such an attempt: it fails with
/// `error.execution` and changes nothing.
#[test]
fn a_member_write_to_a_system_variable_fails_in_every_mode() {
    let mut model = model();
    let host = &mut NoStates;
    let tick = payload_event();
    for script in [
        "_event.name = 'x'",
        "_event.added = 1",
        "_event.data.p = 9",
        "_event.data.q.push(2)",
        "_event.data.q[0] = 5",
        "delete _event.name",
        "delete _event.data.p",
        "Object.setPrototypeOf(_event.data, null)",
        "Reflect.set(_event, 'name', 'x')",
        "Reflect.deleteProperty(_event.data, 'p')",
        "_event.__proto__ = {}",
        "_ioprocessors.scxml.location = 'x'",
        "_ioprocessors.added = 1",
        "delete _ioprocessors.scxml",
        "(function () { 'use strict'; _event.data.p = 9; })()",
    ] {
        assert_eq!(
            model.execute(&format!("kept = 1; {script}"), &tick, host),
            Err(DataModelError::Execution),
            "{script}"
        );
    }
    assert_eq!(
        model.evaluate_value(
            "[_event.name, _event.data.p, _event.data.q.length, _event.data.q[0], _ioprocessors.scxml.location, 'added' in _event]",
            &tick,
            host
        ),
        Ok(DataValue::JsonArray(
            r##"["tick",1,1,1,"#_scxml_s1",false]"##.into()
        ))
    );
}

/// The read-only objects still read as the values they are: frozen,
/// arrays that are arrays, JSON text, identity within one event, and
/// portable values when copied into the data model; an inherited write
/// through one (an object whose prototype is `_event.data`) is ECMAScript's
/// ordinary failed assignment, not a write to the system variable.
#[test]
fn read_only_system_objects_read_as_their_values() {
    let mut model = model();
    let host = &mut NoStates;
    let tick = payload_event();
    for guard in [
        "Object.isFrozen(_event) && Object.isFrozen(_event.data.q)",
        "Array.isArray(_event.data.q) && _event.data.q instanceof Array",
        "JSON.stringify(_event.data) === '{\"p\":1,\"q\":[1]}'",
        "_event === _event && _event.data === _event.data",
        "Object.keys(_event).join() === 'name,type,sendid,origin,origintype,invokeid,data,raw'",
        "(function () { var o = Object.create(_event.data); o.p = 2; return o.p === 1; })()",
        "Object.freeze(_event) === _event",
    ] {
        assert_eq!(
            model.evaluate_condition(guard, &tick, host),
            Ok(true),
            "{guard}"
        );
    }
    model
        .assign_expression("kept", "_event.data", &tick, host)
        .expect("the event data is assignable");
    assert_eq!(
        model.evaluate_value_without_event("kept", host),
        Ok(DataValue::JsonObject(r#"{"p":1,"q":[1]}"#.into()))
    );
    assert_eq!(
        model.evaluate_log("_event.data.q", &tick, host),
        Ok("[1]".into())
    );
}

/// An event bound again is the same `_event` object, whichever operation
/// binds it, and so is an event that differs only in its trace context
/// (which `_event` does not expose); another event is a new object.
#[test]
fn an_event_bound_again_is_the_same_event_object() {
    let mut model = model();
    let host = &mut NoStates;
    let tick = payload_event();
    model
        .assign_expression("kept", "_event", &tick, host)
        .expect("the event is assignable");
    assert_eq!(
        model.evaluate_condition("kept === _event", &tick, host),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_value("kept === _event", &tick, host),
        Ok(DataValue::Bool(true))
    );
    let mut traced = tick.clone();
    traced.trace_context = Some(TraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
        tracestate: None,
        baggage: None,
        baggage_policy: BaggagePolicy::Drop,
    });
    assert_eq!(
        model.evaluate_condition("kept === _event", &traced, host),
        Ok(true)
    );
    let mut other = tick.clone();
    other.data = DataValue::JsonObject(r#"{"p":2,"q":[1]}"#.into());
    assert_eq!(
        model.evaluate_condition("kept !== _event && _event.data.p === 2", &other, host),
        Ok(true)
    );
    assert_eq!(
        model.evaluate_condition("kept !== _event", &tick, host),
        Ok(true)
    );
}

/// An XML payload is a read-only DOM (B.2.8.1) whose nodes reject writes as
/// the other system objects do, and still read, convert, and log as XML.
#[test]
fn an_xml_event_payload_is_read_only_and_still_a_document() {
    let mut model = model();
    let host = &mut NoStates;
    let xml = r#"<books><book title="t1"/><book title="t2"/></books>"#;
    let mut books = event("books");
    books.data = DataValue::Xml(xml.into());
    assert_eq!(
        model.evaluate_condition(
            "_event.data.getElementsByTagName('book')[1].getAttribute('title') == 't2' && _event.data.documentElement.firstChild.nodeName == 'book'",
            &books,
            host
        ),
        Ok(true)
    );
    assert_eq!(
        model.execute("_event.data.documentElement.tagName = 'x'", &books, host),
        Err(DataModelError::Execution)
    );
    model
        .assign_expression("kept", "_event.data", &books, host)
        .expect("the document is assignable");
    assert_eq!(
        model.evaluate_value_without_event("kept", host),
        Ok(DataValue::Xml(xml.into()))
    );
    assert_eq!(
        model.evaluate_log("_event.data", &books, host),
        Ok(xml.into())
    );
}

/// 5.10: "The SCXML processor MUST bind the variable _name at load time to
/// the value of the 'name' attribute of the <scxml> element." A document
/// without that attribute supplies no value, so `_name` is declared and
/// read-only but `undefined`; the processor invents no name.
#[test]
fn name_is_undefined_and_still_read_only_when_the_document_has_no_name() {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["kept"],
            SystemContext {
                session_id: "s1".into(),
                machine_name: None,
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize without a name");
    let host = &mut NoStates;
    assert_eq!(
        model.evaluate_condition_without_event(
            "'_name' in globalThis && typeof _name === 'undefined'",
            host
        ),
        Ok(true)
    );
    assert_eq!(
        model.execute_without_event("_name = 'scxml'", host),
        Err(DataModelError::Execution),
        "_name stays a read-only system variable"
    );
    assert_eq!(
        model.evaluate_condition_without_event("_name === undefined", host),
        Ok(true)
    );
}
