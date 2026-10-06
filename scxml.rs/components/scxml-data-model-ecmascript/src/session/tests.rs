//! The native portable conversion (`Session::portable_value`) reads values
//! without running user code and writes exactly the text `JSON.stringify`
//! writes for every JSON tree.

use scxml_types::DataValue;

use super::{Engine, Session};
use crate::{NoHost, SystemContext};

fn system() -> SystemContext {
    SystemContext {
        session_id: "s1".into(),
        machine_name: Some("m".into()),
        io_processors: DataValue::canonical_json_object("{}").expect("canonical"),
    }
}

/// `expression`'s portable value natively, and the canonical form of its
/// `JSON.stringify` text (`None` when it has none or is not a tree).
fn both(expression: &'static str) -> (Result<DataValue, crate::DataModelError>, Option<String>) {
    let engine = Engine::default();
    engine
        .start(&system(), Vec::new())
        .expect("the realm starts");
    engine
        .run(&mut NoHost, None, move |session: &mut Session| {
            let value = session
                .eval(&format!("({expression})"))
                .map_err(|_| crate::DataModelError::Execution)?;
            let native = session.portable_value(&value);
            let text = session
                .eval(&format!("JSON.stringify({expression})"))
                .ok()
                .and_then(|text| text.as_string())
                .map(|text| text.to_std_string_escaped());
            Ok((native, text))
        })
        .expect("the operation runs")
}

#[test]
fn json_trees_convert_exactly_as_json_stringify_writes_them() {
    for expression in [
        "({})",
        "[]",
        "({ a: 1, b: [true, null, 'x'], c: { d: -0, e: 1e21, f: 0.1 } })",
        "[1, [2, [3]], { k: 'v\\u0000\\n\"' }]",
        "({ 2: 'two', 1: 'one', z: 'last', a: 'first' })",
        "(() => { const o = { shown: 1 }; Object.defineProperty(o, 'hidden', { value: 2 }); return o; })()",
        "(() => { const shared = { k: 1 }; return [shared, shared]; })()",
    ] {
        let (native, text) = both(expression);
        let text = text.unwrap_or_else(|| panic!("{expression}: JSON has text"));
        let expected = if text.starts_with('[') {
            DataValue::canonical_json_array(&text)
        } else {
            DataValue::canonical_json_object(&text)
        }
        .expect("canonical");
        assert_eq!(native, Ok(expected), "{expression}");
    }
}

#[test]
fn values_outside_json_trees_have_no_portable_value() {
    for expression in [
        "({ u: undefined })",
        "[1, , 3]",
        "[undefined]",
        "({ f() {} })",
        "({ get g() { return 1; } })",
        "({ [Symbol.iterator]: 1 })",
        "(() => { const o = {}; o.self = o; return o; })()",
        "new Date(0)",
        "new Map()",
        "Object.create(null)",
        "({ n: NaN })",
        "(() => { const a = [1]; a.extra = 2; return a; })()",
        "(function () {})",
    ] {
        let (native, _) = both(expression);
        assert_eq!(
            native,
            Err(crate::DataModelError::UnsupportedValue),
            "{expression}"
        );
    }
}

#[test]
fn reading_a_value_never_runs_user_code() {
    let engine = Engine::default();
    engine
        .start(&system(), Vec::new())
        .expect("the realm starts");
    let calls = engine
        .run(&mut NoHost, None, |session: &mut Session| {
            session
                .eval(
                    "var calls = 0; \
                     Object.prototype.toJSON = function () { calls += 1; return 'hijacked'; }; \
                     JSON.stringify = function () { calls += 1; return '0'; }; \
                     Object.defineProperty(Object.prototype, 'late', { get() { calls += 1; }, enumerable: false }); \
                     var tree = { a: [1, { b: 2 }] };",
                )
                .map_err(|_| crate::DataModelError::Execution)?;
            let tree = session
                .eval("tree")
                .map_err(|_| crate::DataModelError::Execution)?;
            let portable = session.portable_value(&tree)?;
            let calls = session
                .eval("calls")
                .map_err(|_| crate::DataModelError::Execution)?;
            Ok((portable, calls.as_number()))
        })
        .expect("the operation runs");
    assert_eq!(
        calls,
        (
            DataValue::canonical_json_object(r#"{"a":[1,{"b":2}]}"#).expect("canonical"),
            Some(0.0)
        )
    );
}
