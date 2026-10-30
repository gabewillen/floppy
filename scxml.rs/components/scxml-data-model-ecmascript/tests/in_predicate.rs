//! B.2.3: "The SCXML processor MUST define an ECMAScript function named
//! '`In()`' that takes a stateID as its argument and returns 'true' if and
//! only if that state is in the current state configuration, as described
//! in 5.9.1 Conditional Expressions."
//!
//! `In` is a real function of the realm: its argument may be any
//! expression, it can be referenced, passed around, and called from a
//! function a `<script>` defined, and every call asks the execution host
//! about the configuration current at that operation.

use std::collections::BTreeSet;

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, SystemContext,
};
use scxml_types::{DataValue, Event, EventName};

/// A configuration of exactly `active`, recording every queried id.
struct Configuration {
    active: BTreeSet<String>,
    queried: Vec<String>,
}

impl Configuration {
    fn of(states: &[&str]) -> Self {
        Self {
            active: states.iter().map(|state| (*state).to_owned()).collect(),
            queried: Vec::new(),
        }
    }
}

impl ExecutionHost for Configuration {
    fn in_state(&mut self, state_id: &str) -> Result<bool, DataModelError> {
        self.queried.push(state_id.to_owned());
        Ok(self.active.contains(state_id))
    }
}

struct Failing;

impl ExecutionHost for Failing {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Err(DataModelError::Host)
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize_declarations(
            ["target"],
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("declarations initialize");
    model
}

fn event() -> Event {
    Event::external(EventName::new("e").expect("valid event name"))
}

#[test]
fn in_is_an_ecmascript_function() {
    let model = model();
    assert_eq!(
        model.evaluate_condition(
            "typeof In === 'function' && In.length === 1 && In.name === 'In'",
            &event(),
            &mut Configuration::of(&[])
        ),
        Ok(true)
    );
}

#[test]
fn in_takes_any_argument_expression() {
    let mut model = model();
    let host = &mut Configuration::of(&["ready"]);
    for (guard, expected) in [
        ("In('ready')", true),
        ("In(['rea', 'dy'].join(''))", true),
        ("In('re' + 'ady') && !In('idle')", true),
        ("(function () { var f = In; return f('ready'); })()", true),
        ("['ready', 'idle'].map(In).join() === 'true,false'", true),
        ("[{ toString: () => 'ready' }].map((id) => In(id))[0]", true),
    ] {
        assert_eq!(
            model.evaluate_condition(guard, &event(), host),
            Ok(expected),
            "{guard}"
        );
    }
    model
        .assign_expression("target", "In('idle') ? 'idle' : 'ready'", &event(), host)
        .expect("In() works in an assignment");
    assert_eq!(
        model.evaluate_value("target", &event(), host),
        Ok(DataValue::String("ready".into()))
    );
}

#[test]
fn a_script_function_asks_about_the_configuration_when_it_is_called() {
    let mut model = model();
    model
        .execute(
            "function active(id) { return In(id); }",
            &event(),
            &mut Configuration::of(&["first"]),
        )
        .expect("the script defines a function");
    for (states, expected) in [(&["first"][..], true), (&["second"][..], false)] {
        let host = &mut Configuration::of(states);
        assert_eq!(
            model.evaluate_condition("active('first')", &event(), host),
            Ok(expected),
            "{states:?}"
        );
        assert!(
            host.queried.iter().all(|id| id == "first"),
            "{:?}",
            host.queried
        );
    }
}

#[test]
fn a_failing_host_fails_the_operation() {
    let model = model();
    assert_eq!(
        model.evaluate_condition("In('a')", &event(), &mut Failing),
        Err(DataModelError::Host)
    );
    assert_eq!(
        model.evaluate_condition("In(String.fromCharCode(97))", &event(), &mut Failing),
        Err(DataModelError::Host)
    );
}

/// One operation may ask `In()` about any number of distinct state ids.
#[test]
fn distinct_in_queries_per_operation_are_unbounded() {
    let model = model();
    let host = &mut Configuration::of(&["s1999"]);
    let many = "(function () { for (var i = 0; i < 2000; i++) if (In('s' + i)) return i; return -1; })() === 1999";
    assert_eq!(model.evaluate_condition(many, &event(), host), Ok(true));
}
