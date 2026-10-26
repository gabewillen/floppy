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

/// Whether two values are the same tree: the same prototypes, own keys in
/// the same order, the same property attributes and `Object.is` leaves.
const SAME_TREE: &str = "(function same(a, b) { \
     if (a === null || typeof a !== 'object' || b === null || typeof b !== 'object') return Object.is(a, b); \
     if (Array.isArray(a) !== Array.isArray(b) || Object.getPrototypeOf(a) !== Object.getPrototypeOf(b) \
         || Object.isExtensible(a) !== Object.isExtensible(b)) return false; \
     const keys = Reflect.ownKeys(a), other = Reflect.ownKeys(b); \
     if (keys.length !== other.length) return false; \
     return keys.every((key, i) => { \
         if (key !== other[i]) return false; \
         const x = Object.getOwnPropertyDescriptor(a, key), y = Object.getOwnPropertyDescriptor(b, key); \
         return x.writable === y.writable && x.enumerable === y.enumerable \
             && x.configurable === y.configurable && same(x.value, y.value); \
     }); \
 })";

/// `json` built directly, and whether that is the tree its literal
/// evaluates to (`None` when the direct build declines it).
fn built(json: &'static str) -> Option<bool> {
    let engine = Engine::default();
    engine
        .start(&system(), Vec::new())
        .expect("the realm starts");
    engine
        .run(&mut NoHost, None, move |session: &mut Session| {
            let Some(direct) = session.json_to_value(json) else {
                return Ok(None);
            };
            let literal = session
                .eval(&format!("({json})"))
                .map_err(|_| crate::DataModelError::Execution)?;
            let same = session
                .call_source(SAME_TREE, &[literal, direct])
                .map_err(|_| crate::DataModelError::Execution)?;
            Ok(same.as_boolean())
        })
        .expect("the operation runs")
}

#[test]
fn json_builds_the_tree_its_literal_evaluates_to() {
    for json in [
        "{}",
        "[]",
        r#"{"a":1,"b":[true,null,"x"],"c":{"d":-0,"e":1e21,"f":0.1,"g":-0.0}}"#,
        r#"[1,[2,[3]],{"k":"v\u0000\n\"😀"}]"#,
        r#"{"z":"last","2":"two","1":"one","a":"first"}"#,
        r#"{"a":1,"b":2,"a":3}"#,
        "[9007199254740993,9999999999999999999,-9223372036854775808,0]",
        "[0.25,123456789.123456,1e-7,-1E+2,1.5e22,0.000000000000000000001]",
        r#"{"text":"1.7976931348623157e308 \" 0.30000000000000004"}"#,
        r#"{"constructor":1,"toString":"x","hasOwnProperty":null}"#,
        "  { \"spaced\" : [ 1 , 2 ] }  ",
    ] {
        assert_eq!(built(json), Some(true), "{json}");
    }
}

#[test]
fn json_a_literal_reads_differently_is_left_to_the_literal() {
    let deep = format!("{}{}", "[".repeat(40), "]".repeat(40));
    let deep: &'static str = Box::leak(deep.into_boxed_str());
    for json in [
        r#"{"__proto__":{"polluted":true}}"#,
        r#"[{"a":{"__proto__":null}}]"#,
        "[1e400]",
        "[0.30000000000000004]",
        "[2.2250738585072011e-308]",
        "[5e-324]",
        "[1.7976931348623157e308]",
        "[123456789012345678901]",
        "[1e23]",
        r#"["\ud800"]"#,
        deep,
    ] {
        assert_eq!(built(json), None, "{json}");
    }
}

#[test]
fn an_event_is_built_as_its_source_evaluates() {
    use scxml_types::{Event, EventName, InvokeId, SendId};
    let mut full = Event::external(EventName::new("done.invoke.x").expect("name"));
    full.send_id = Some(SendId::new("send-1").expect("send id"));
    full.origin = Some("#_scxml_s1".into());
    full.origin_type = Some("http://www.w3.org/TR/scxml/#SCXMLEventProcessor".into());
    full.invoke_id = Some(InvokeId::new("x").expect("invoke id"));
    full.data =
        DataValue::canonical_json_object(r#"{"text":"hi","n":[1,2.5]}"#).expect("canonical");
    full.raw = Some("raw text".into());
    let mut events = vec![full, Event::external(EventName::new("bare").expect("name"))];
    for data in [
        DataValue::String("words".into()),
        DataValue::Null,
        DataValue::canonical_json_array("[]").expect("canonical"),
    ] {
        let mut event = Event::external(EventName::new("data").expect("name"));
        event.data = data;
        events.push(event);
    }
    for event in events {
        let engine = Engine::default();
        engine
            .start(&system(), Vec::new())
            .expect("the realm starts");
        let source = crate::event_to_source(&event).expect("source");
        let same = engine
            .run(&mut NoHost, None, move |session: &mut Session| {
                let direct = session.event_value(&event).expect("built directly");
                let literal = session
                    .eval(&source)
                    .map_err(|_| crate::DataModelError::Execution)?;
                session
                    .call_source(SAME_TREE, &[literal, direct])
                    .map_err(|_| crate::DataModelError::Execution)
                    .map(|same| same.as_boolean())
            })
            .expect("the operation runs");
        assert_eq!(same, Some(true));
    }
}
