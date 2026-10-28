//! Hostile-input proof for the ECMAScript built-in guards: a built-in's
//! result has no size bound (SCXML sets none), while native recursion past the
//! nesting bound and array-likes whose length is not a plain data property
//! still reject before the native built-in runs, and the guarded replacements
//! keep standard semantics.

use scxml_data_model_ecmascript::{
    DataModelError, EcmaScriptDataModel, ExecutionHost, MAX_VALUE_BYTES, SystemContext,
};
use scxml_types::{DataValue, Event, EventName};

struct NoStates;

impl ExecutionHost for NoStates {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, DataModelError> {
        Ok(false)
    }
}

fn model() -> EcmaScriptDataModel {
    let mut model = EcmaScriptDataModel::new();
    model
        .initialize(
            "{}",
            SystemContext {
                session_id: "s".into(),
                machine_name: Some("m".into()),
                io_processors: DataValue::JsonObject("{}".into()),
            },
        )
        .expect("empty data model initializes");
    model
}

fn event() -> Event {
    Event::external(EventName::new("e").expect("static event name is valid"))
}

fn value(model: &EcmaScriptDataModel, expression: &str) -> Result<DataValue, DataModelError> {
    model.evaluate_value(expression, &event(), &mut NoStates)
}

/// Runs `expression` inside a try/catch and reports whether the builtin was
/// allowed to complete, or rejected with the guard's `RangeError`.
fn outcome(model: &EcmaScriptDataModel, expression: &str) -> String {
    let probe = format!(
        "(() => {{ try {{ ({expression}); return 'allowed'; }} catch (error) {{ \
         return error instanceof RangeError ? 'rejected' : 'other:' + error.name; }} }})()"
    );
    match value(model, &probe) {
        Ok(DataValue::String(outcome)) => outcome,
        other => format!("evaluation failed: {other:?}"),
    }
}

fn assert_outcome(expected: &str, expressions: &[&str]) {
    let model = model();
    for expression in expressions {
        assert_eq!(outcome(&model, expression), expected, "{expression}");
    }
}

#[test]
fn a_builtin_result_has_no_size_bound() {
    let model = model();
    assert_eq!(
        value(
            &model,
            &format!("'x'.repeat({}).length", MAX_VALUE_BYTES + 1)
        ),
        Ok(DataValue::Number(
            scxml_types::FiniteNumber::new(f64::from(
                u32::try_from(MAX_VALUE_BYTES + 1).expect("fits")
            ))
            .expect("finite")
        ))
    );
    assert_outcome(
        "allowed",
        &[
            "'x'.padStart(1e6)",
            "'x'.padEnd(1e6, 'ab')",
            "'a'.repeat(40000).concat('b'.repeat(40000))",
            "'a'.repeat(4000).replace(/a/g, '$`')",
            "'a'.repeat(4000).replaceAll('a', '$`')",
            "'a'.repeat(4000).replace(/a/g, () => 'b'.repeat(20))",
            "String.raw({raw: ['a', 'b', 'c']}, 'x'.repeat(40000), 'y'.repeat(40000))",
            "Array(64).fill('x'.repeat(2048)).join('')",
            "String(Array(64).fill('x'.repeat(2048)))",
            "Array(64).fill('x'.repeat(2048)).toLocaleString()",
            "(a => [].concat(a, a, a))(Array(40000).fill(0))",
            "Array(300).fill(Array(300).fill(0)).flat()",
            "Array(300).fill(0).flatMap(() => Array(300).fill(0))",
            "Array.from({length: 100000})",
            "JSON.stringify(Array(64).fill('x'.repeat(2048)))",
            "JSON.stringify(Array(12000).fill(0), null, 10)",
        ],
    );
}

#[test]
fn deep_nesting_rejects_before_native_recursion() {
    assert_outcome(
        "rejected",
        &[
            "(() => { let a = []; for (let i = 0; i < 100000; i++) a = [a]; return a.flat(Infinity); })()",
            "(() => { let a = []; for (let i = 0; i < 100000; i++) a = [a]; return a.join(); })()",
            "(() => { let a = []; for (let i = 0; i < 100000; i++) a = [a]; return JSON.stringify(a); })()",
            "JSON.parse('['.repeat(100000) + ']'.repeat(100000))",
        ],
    );
}

#[test]
fn array_likes_without_a_data_length_reject_before_native_iteration() {
    assert_outcome(
        "rejected",
        &[
            "Array.prototype.fill.call({get length() { return 1e7; }}, 0)",
            "Array.prototype.fill.call({length: {valueOf() { return 1e7; }}}, 0)",
        ],
    );
}

#[test]
fn bounded_builtins_preserve_standard_semantics_within_bounds() {
    let model = model();
    let cases = [
        ("'ab'.padStart(5, 'x')", "\"xxxab\""),
        ("'ab'.padEnd(4)", "\"ab  \""),
        ("'a'.concat(1, null)", "\"a1null\""),
        ("'x'.repeat(3)", "\"xxx\""),
        ("'abc'.replace('b', \"[$`|$'|$&|$$]\")", "\"a[a|c|b|$]c\""),
        (
            "'2020-01'.replace(/(?<y>\\d+)-(?<m>\\d+)/, '$<m>/$<y>')",
            "\"01/2020\"",
        ),
        (
            "'abc'.replace(/(a)(b)?(x)?/, '[$1|$2|$3|$4|$01|$10|$0]')",
            "\"[a|b||$4|a|a0|$0]c\"",
        ),
        ("'aaa'.replaceAll('a', (m, i) => i)", "\"012\""),
        (
            "'x-y'.replace(/-/, (m, off, s) => s.length + ':' + off)",
            "\"x3:1y\"",
        ),
        (
            "'ab'.replace(/(?<n>a)/, (...args) => typeof args[args.length - 1])",
            "\"objectb\"",
        ),
        ("'ab'.replace(/(?<n>z)?a/, '[$<n>]')", "\"[]b\""),
        ("'ab'.replace(/a/, '$<n>')", "\"$<n>b\""),
        ("'a.b.c'.replaceAll('.', '$&$&')", "\"a..b..c\""),
        ("'aXbX'.replace(/X/g, '-')", "\"a-b-\""),
        ("[1, [2, [3, [4]]]].flat(Infinity)", "[1,2,3,4]"),
        ("[1, [2, [3]]].flat()", "[1,2,[3]]"),
        ("[1, , 3].flat()", "[1,3]"),
        ("[1, 2].flatMap(x => [x, x * 2])", "[1,2,2,4]"),
        ("[1, 2].concat([3], 4, [[5]])", "[1,2,3,4,[5]]"),
        ("[null, undefined, 1].join('-')", "\"--1\""),
        ("String([1, [2, 3]])", "\"1,2,3\""),
        // Boa uses ", " as the implementation-defined list separator.
        ("[1, 2].toLocaleString()", "\"1, 2\""),
        (
            "Array.prototype.join.call({length: 2, 0: 'a', 1: 'b'})",
            "\"a,b\"",
        ),
        ("Array.from({length: 3}, (v, i) => i * 2)", "[0,2,4]"),
        ("Array.from('ab')", "[\"a\",\"b\"]"),
        ("Array.from(new Set([1, 1, 2]))", "[1,2]"),
        ("Math.max.apply(null, [1, 5, 2])", "5"),
        ("Reflect.apply(Math.min, null, [3, 1])", "1"),
        ("Reflect.construct(Array, [1, 2]).length", "2"),
        ("[...[1, 2]]", "[1,2]"),
        ("[3, 1, 2].sort()", "[1,2,3]"),
        ("[1, 2, 3].indexOf(2)", "1"),
        ("Array(3).length", "3"),
        (
            "Array.prototype.values === Array.prototype[Symbol.iterator]",
            "true",
        ),
        (
            "JSON.stringify({a: [1, {b: 'x'}]}, null, 2)",
            "\"{\\n  \\\"a\\\": [\\n    1,\\n    {\\n      \\\"b\\\": \\\"x\\\"\\n    }\\n  ]\\n}\"",
        ),
        (
            "JSON.stringify({a: 1, b: 2, c: {a: 3, d: 4}}, ['a', 'c'])",
            "\"{\\\"a\\\":1,\\\"c\\\":{\\\"a\\\":3}}\"",
        ),
        (
            "JSON.stringify({a: 1, b: 'x'}, (k, v) => typeof v === 'number' ? v * 2 : v)",
            "\"{\\\"a\\\":2,\\\"b\\\":\\\"x\\\"}\"",
        ),
        ("JSON.stringify({toJSON() { return [1]; }})", "\"[1]\""),
        ("JSON.stringify('a\\nb')", "\"\\\"a\\\\nb\\\"\""),
        (
            "JSON.stringify([1, 'a', true, null], null, '--')",
            "\"[\\n--1,\\n--\\\"a\\\",\\n--true,\\n--null\\n]\"",
        ),
        ("JSON.stringify(undefined) === undefined", "true"),
    ];
    let mut mismatches = Vec::new();
    for (expression, expected_json) in cases {
        let expected = match expected_json.as_bytes().first() {
            Some(b'"') => DataValue::String(
                serde_json::from_str(expected_json).expect("expected string literal is JSON"),
            ),
            Some(b'[') => DataValue::JsonArray(expected_json.into()),
            Some(b't') => DataValue::Bool(true),
            _ => DataValue::Number(
                scxml_types::FiniteNumber::new(
                    expected_json.parse().expect("expected number literal"),
                )
                .expect("finite"),
            ),
        };
        let actual = value(&model, expression);
        if actual != Ok(expected.clone()) {
            mismatches.push(format!(
                "{expression}: expected {expected:?}, got {actual:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
