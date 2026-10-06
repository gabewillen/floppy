//! Hostile-input proof that high-amplification ECMAScript builtins reject
//! over-bound work before the native builtin allocates or iterates, and that
//! the bounded replacements keep standard semantics inside the bound.

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

fn assert_rejected(expressions: &[&str]) {
    let model = model();
    for expression in expressions {
        assert_eq!(
            outcome(&model, expression),
            "rejected",
            "over-bound builtin must reject before allocation: {expression}"
        );
    }
}

#[test]
fn value_bound_used_by_guards_matches_public_constant() {
    let model = model();
    assert_eq!(
        outcome(&model, &format!("'x'.repeat({MAX_VALUE_BYTES})")),
        "allowed"
    );
    assert_eq!(
        outcome(&model, &format!("'x'.repeat({})", MAX_VALUE_BYTES + 1)),
        "rejected"
    );
}

#[test]
fn string_amplifiers_reject_before_allocation() {
    assert_rejected(&[
        "'x'.padStart(1e6)",
        "'x'.padEnd(1e6, 'ab')",
        "'a'.repeat(40000).concat('b'.repeat(40000))",
        "'a'.repeat(4000).replace(/a/g, '$`')",
        "'a'.repeat(4000).replace(/a/g, \"$'\")",
        "'a'.repeat(4000).replace(/a/g, '$&$&$&$&$&$&$&$&$&$&$&$&$&$&$&$&$&$&')",
        "'a'.repeat(4000).replaceAll('a', '$`')",
        "'a'.repeat(4000).replace(/a/g, () => 'b'.repeat(20))",
        "/a/g[Symbol.replace]('a'.repeat(4000), '$`')",
        "RegExp.prototype[Symbol.replace].call(/a/g, 'a'.repeat(4000), '$`')",
        // Tampering with globals the guards use must not disable them.
        "(() => { Math.trunc = () => 0; Number.isFinite = () => true; \
          Reflect.apply = () => ''; return 'ab'.repeat(40000); })()",
        "(() => { String.prototype.padStart = null; return 'x'.padStart(1e6); })()",
    ]);
}

#[test]
fn array_length_amplifiers_reject_before_native_iteration() {
    assert_rejected(&[
        "(a => { a.length = 1e7; return a.fill(0); })([])",
        "(a => { a.length = 1e7; return a.join('ab'); })([])",
        "Array(1e7).join('ab')",
        "Array(1e7).indexOf(1)",
        "Array(1e7).map(x => x)",
        "Array(64).fill('x'.repeat(2048)).join('')",
        "String(Array(64).fill('x'.repeat(2048)))",
        "Array(64).fill('x'.repeat(2048)).toLocaleString()",
        "Array.from({length: 1e7})",
        "Math.max.apply(null, {length: 1e7})",
        "Reflect.apply(Math.max, null, {length: 1e7})",
        "Reflect.construct(Array, {length: 1e7})",
        "[...Array(1e6)]",
        "(a => [].concat(a, a, a))(Array(40000).fill(0))",
        "Array(300).fill(Array(300).fill(0)).flat()",
        "Array(300).fill(0).flatMap(() => Array(300).fill(0))",
        "Array.prototype.fill.call({length: 1e7}, 0)",
        "Array.prototype.fill.call({get length() { return 1e7; }}, 0)",
        "Array.prototype.fill.call({length: {valueOf() { return 1e7; }}}, 0)",
    ]);
}

#[test]
fn json_raw_and_iterator_amplifiers_reject_before_allocation() {
    assert_rejected(&[
        "JSON.stringify(Array(64).fill('x'.repeat(2048)))",
        "JSON.stringify(Array(64).fill(new String('x'.repeat(2048))))",
        "JSON.stringify(Array(64).fill({ toJSON() { return 'x'.repeat(2048); } }))",
        "JSON.stringify(Array(12000).fill(0), null, 10)",
        "String.raw({raw: {length: 1e7}})",
        "String.raw({raw: ['a', 'b', 'c']}, 'x'.repeat(40000), 'y'.repeat(40000))",
        // A getter grows the array while the native iterator reads its length
        // live on every step.
        "(() => { const a = [0]; Object.defineProperty(a, 0, { get() { a.length = 1e6; return 0; } }); \
          return [...a]; })()",
    ]);
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
